"""MCP (stdio) の最小の客。手動確認の手順 19 で、ai-cu mcp に JSON-RPC の行を順に送って応答を見る。

使い方:
  python docs/manual-check/mcp_client.py <ai-cu.exe のパス> [ai-cu の引数...] -- <requests.jsonl> [--timeout SEC] [--save-images DIR]

requests.jsonl は 1 行 1 メッセージ (JSON-RPC 2.0)。空行と # で始まる行は飛ばす。各行を送った後、id のある
リクエストならその応答が返るまで (最長 --timeout 秒) 待って表示する。通知 (id 無し) は送るだけ。
最後に標準入力を閉じて (EOF)、サーバーの終了コードを出す。サーバーの標準エラーはそのまま流す。
応答が制限時間内に無い、サーバーが先に終わった、のどちらも表示して終了コード 1 にする。

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


def parse_args(argv):
    if "--" not in argv:
        return None
    sep = argv.index("--")
    cmd = argv[:sep]
    rest = argv[sep + 1 :]
    opts = {"timeout": 60.0, "save_dir": None, "requests": None}
    i = 0
    while i < len(rest):
        if rest[i] == "--timeout":
            opts["timeout"] = float(rest[i + 1])
            i += 2
        elif rest[i] == "--save-images":
            opts["save_dir"] = rest[i + 1]
            i += 2
        else:
            opts["requests"] = rest[i]
            i += 1
    if not cmd or opts["requests"] is None:
        return None
    return cmd, opts


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


def wait_reply(responses, lock, want_id, timeout):
    """want_id の応答を待つ。(reply, closed, timed_out)。別の id の応答は表示して捨てる。"""
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
        reply = json.loads(got)
        if reply.get("id") == want_id:
            return reply, False, False
        print(f"<<< (別の id の応答) {got[:300]}")


def main(argv):
    parsed = parse_args(argv)
    if parsed is None:
        print(__doc__)
        return 2
    cmd, opts = parsed
    with open(opts["requests"], encoding="utf-8") as f:
        lines = [l.rstrip("\r\n") for l in f]
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        bufsize=0,
    )
    responses = []
    lock = threading.Condition()

    def reader():
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

    threading.Thread(target=reader, daemon=True).start()
    counter = [0]
    code = 0
    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        msg = json.loads(line)
        print(f">>> {line[:200]}")
        try:
            proc.stdin.write((line + "\n").encode("utf-8"))
            proc.stdin.flush()
        except OSError as e:
            print(f"<<< (サーバーが終了していて送れない: {e})")
            code = 1
            break
        if "id" not in msg:
            continue
        reply, closed, timed_out = wait_reply(responses, lock, msg["id"], opts["timeout"])
        if closed:
            print("<<< (サーバーが標準出力を閉じた)")
            code = 1
            break
        if timed_out:
            print(f"<<< (応答なし: {opts['timeout']} 秒)")
            code = 1
            continue
        shrink_images(reply, opts["save_dir"], counter)
        print("<<< " + json.dumps(reply, ensure_ascii=False)[:4000])
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
    print(f"server exit={rc}")
    return code


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
