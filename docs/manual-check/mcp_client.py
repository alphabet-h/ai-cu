"""MCP (stdio) の最小の客。手動確認の手順 19 で、ai-cu mcp に JSON-RPC の行を順に送って応答を見る。

使い方:
  python docs/manual-check/mcp_client.py <ai-cu.exe のパス> [ai-cu の引数...] -- <requests.jsonl> [--timeout SEC] [--save-images DIR] [--save-replies DIR]

requests.jsonl は 1 行 1 メッセージ (JSON-RPC 2.0)。空行と # で始まる行は飛ばす。各行を送った後、id のある
リクエストならその応答が返るまで (最長 --timeout 秒) 待って表示する。通知 (id 無し) は送るだけ。
最後に標準入力を閉じて (EOF)、サーバーの終了コードを出す。サーバーの標準エラーはそのまま流す。
応答が制限時間内に無い、サーバーが先に終わった、のどちらも表示して終了コード 1 にする。
サーバーが終わった後に届いていた応答 (最後の要求の後や、時間切れの後に遅れて届いたもの) も読み切って表示する。
JSON として読めない行は落とさずに「(JSON でない行)」として表示して続ける。

表示は長いと切る (待っていた応答 4000 字、別の id の応答 300 字、送った要求 200 字)。切ったときは
「…(全 N 字のうち L 字)」を付ける。応答が tools/list の形 (result.tools が配列) なら、その前に
「--- tools: N 個 / readOnlyHint: a, b, …」の要約の行を出す。

--save-replies DIR を付けると、その回ごとの下位フォルダ DIR/<YYYYMMDD-HHMMSS> (同じ名前が既にあれば -2 などを
付ける) を作って最初にそのパスを出し、id のある応答を切らずに reply-<id>.json に書く (画像の data は縮めた後)。
id が無いか null の応答は表示だけで書かない。同じ名前になる id (数の 2 と文字の "2" など) は後の応答が上書きする。

画像 (content の image) は data を長さだけにして表示し、--save-images DIR を付ければ png として保存する。
このスクリプトは読むだけで、ai-cu の設定ファイルには触れない。実入力を送る道具を requests.jsonl に書くときは、
人の GO をもらってから走らせる (CLAUDE.md、D-051)。
"""

import base64
import json
import os
import subprocess
import sys
import threading
import time

OTHER_LABEL = "(別の id の応答) "
REPLY_LIMIT = 4000
OTHER_LIMIT = 300
REQUEST_LIMIT = 200
BAD_LIMIT = 300
READER_JOIN_SEC = 5


def parse_args(argv):
    if "--" not in argv:
        return None
    sep = argv.index("--")
    cmd = argv[:sep]
    rest = argv[sep + 1 :]
    opts = {"timeout": 60.0, "save_dir": None, "replies_dir": None, "requests": None}
    i = 0
    while i < len(rest):
        if rest[i] == "--timeout":
            opts["timeout"] = float(rest[i + 1])
            i += 2
        elif rest[i] == "--save-images":
            opts["save_dir"] = rest[i + 1]
            i += 2
        elif rest[i] == "--save-replies":
            opts["replies_dir"] = rest[i + 1]
            i += 2
        else:
            opts["requests"] = rest[i]
            i += 1
    if not cmd or opts["requests"] is None:
        return None
    return cmd, opts


def clip(text, limit):
    """limit 字を超えたら切って、切ったことの印を付ける。"""
    if len(text) <= limit:
        return text
    return text[:limit] + f" …(全 {len(text)} 字のうち {limit} 字)"


def tools_summary(reply):
    """tools/list の形 (result.tools が配列) なら要約の文字列、そうでなければ None。"""
    result = reply.get("result") if isinstance(reply, dict) else None
    if not isinstance(result, dict):
        return None
    tools = result.get("tools")
    if not isinstance(tools, list):
        return None
    names = []
    for tool in tools:
        if not isinstance(tool, dict):
            continue
        ann = tool.get("annotations")
        if isinstance(ann, dict) and ann.get("readOnlyHint") is True:
            names.append(str(tool.get("name")))
    hint = ", ".join(names) if names else "(なし)"
    return f"tools: {len(tools)} 個 / readOnlyHint: {hint}"


def reply_path(run_dir, reply_id):
    """応答を書くファイルのパス。id の記号と空白は _ に置き換える。"""
    name = str(reply_id)
    for ch in '/\\:*?"<>|':
        name = name.replace(ch, "_")
    name = "".join("_" if c.isspace() else c for c in name)
    return os.path.join(run_dir, f"reply-{name}.json")


def run_dir_name(now):
    """回ごとの下位フォルダの名前 (time.struct_time を受ける)。"""
    return time.strftime("%Y%m%d-%H%M%S", now)


def make_run_dir(base, name):
    """base/name を新しく作る。既にあれば name-2、name-3 … と作れるまで試す。作ったパスを返す。"""
    os.makedirs(base, exist_ok=True)
    n = 1
    while True:
        path = os.path.join(base, name if n == 1 else f"{name}-{n}")
        try:
            os.mkdir(path)
            return path
        except FileExistsError:
            n += 1


def save_path(run_dir, reply):
    """応答を書くパス。書かない (run_dir が無い、id が無いか null) なら None。保存の判定はここだけ。"""
    if run_dir is None or reply.get("id") is None:
        return None
    return reply_path(run_dir, reply["id"])


def parse_line(text):
    """サーバーの 1 行を JSON のオブジェクトとして読む。読めない、またはオブジェクトでなければ None。"""
    try:
        value = json.loads(text)
    except ValueError:
        return None
    return value if isinstance(value, dict) else None


def show_bad(text, out=print):
    out("<<< (JSON でない行) " + clip(text, BAD_LIMIT))


def shrink_images(reply, save_dir, counter):
    """結果の image の data を長さ (と保存先) に置き換える。"""
    result = reply.get("result")
    if not isinstance(result, dict):
        return
    for item in result.get("content", []):
        if not (isinstance(item, dict) and item.get("type") == "image"):
            continue
        data = item.get("data", "")
        counter[0] += 1
        if save_dir:
            os.makedirs(save_dir, exist_ok=True)
            path = os.path.join(save_dir, f"mcp-{counter[0]}.png")
            with open(path, "wb") as f:
                f.write(base64.b64decode(data))
            item["data"] = f"<{len(data)} chars, saved {path}>"
        else:
            item["data"] = f"<{len(data)} chars>"


def show(reply, label, limit, run_dir, save_dir_images, counter, out=print):
    """応答を表示する: 画像を縮める → 要約の行 → 切った全文 → (保存するなら) 全文を書く。"""
    shrink_images(reply, save_dir_images, counter)
    summary = tools_summary(reply)
    if summary is not None:
        out("--- " + summary)
    out("<<< " + label + clip(json.dumps(reply, ensure_ascii=False), limit))
    path = save_path(run_dir, reply)
    if path is not None:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(reply, f, ensure_ascii=False, indent=2)
        out(f"(saved {path})")


def wait_reply(responses, lock, want_id, timeout, on_other, on_bad):
    """want_id の応答を待つ。(reply, closed, timed_out)。別の id の応答は on_other、読めない行は on_bad に渡す。"""
    deadline = time.monotonic() + timeout
    while True:
        with lock:
            while not responses and time.monotonic() < deadline:
                lock.wait(timeout=0.1)
            if not responses:
                return None, False, True
            got = responses.pop(0)
        if got is None:
            return None, True, False
        reply = parse_line(got)
        if reply is None:
            on_bad(got)
            continue
        if reply.get("id") == want_id:
            return reply, False, False
        on_other(reply)


def drain(responses, lock, on_other, on_bad):
    """残った行を読み切る。None (読み取りの終わり) か空で止まる。読み取りのスレッドの join の後に呼ぶ。"""
    while True:
        with lock:
            if not responses:
                return
            got = responses.pop(0)
        if got is None:
            return
        reply = parse_line(got)
        if reply is None:
            on_bad(got)
        else:
            on_other(reply)


def main(argv):
    parsed = parse_args(argv)
    if parsed is None:
        print(__doc__)
        return 2
    cmd, opts = parsed
    with open(opts["requests"], encoding="utf-8") as f:
        lines = [l.rstrip("\r\n") for l in f]
    run_dir = None
    if opts["replies_dir"]:
        run_dir = make_run_dir(opts["replies_dir"], run_dir_name(time.localtime()))
        print(f"replies: {run_dir}")
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        bufsize=0,
    )
    responses = []
    lock = threading.Condition()

    def reader_loop():
        for raw in proc.stdout:
            text = raw.decode("utf-8", "replace").strip()
            if not text:
                continue
            with lock:
                responses.append(text)
                lock.notify_all()
        with lock:
            responses.append(None)
            lock.notify_all()

    reader = threading.Thread(target=reader_loop, daemon=True)
    reader.start()
    counter = [0]

    def on_other(reply):
        show(reply, OTHER_LABEL, OTHER_LIMIT, run_dir, opts["save_dir"], counter)

    code = 0
    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        msg = json.loads(line)
        print(f">>> {clip(line, REQUEST_LIMIT)}")
        try:
            proc.stdin.write((line + "\n").encode("utf-8"))
            proc.stdin.flush()
        except OSError as e:
            print(f"<<< (サーバーが終了していて送れない: {e})")
            code = 1
            break
        if "id" not in msg:
            continue
        reply, closed, timed_out = wait_reply(
            responses, lock, msg["id"], opts["timeout"], on_other, show_bad
        )
        if closed:
            print("<<< (サーバーが標準出力を閉じた)")
            code = 1
            break
        if timed_out:
            print(f"<<< (応答なし: {opts['timeout']} 秒)")
            code = 1
            continue
        show(reply, "", REPLY_LIMIT, run_dir, opts["save_dir"], counter)
    try:
        proc.stdin.close()
    except OSError:
        pass
    try:
        rc = proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        print("サーバーが 10 秒以内に終わらない。kill する")
        proc.kill()
        rc = proc.wait()
        code = 1
    reader.join(timeout=READER_JOIN_SEC)
    if reader.is_alive():
        print(f"(読み取りが {READER_JOIN_SEC} 秒で終わらない。残りを読まずに終える)")
    else:
        drain(responses, lock, on_other, show_bad)
    print(f"server exit={rc}")
    return code


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
