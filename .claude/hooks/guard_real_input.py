#!/usr/bin/env python3
"""PreToolUse(Bash|PowerShell|Monitor) guard: ai-cu の実入力サブコマンドを、テスト以外から直接起動させない。

なぜ hook なのか
---------------
ai-cu は本物のマウスとキーボードを動かす。開発中に AI が誤って実機で `ai-cu run ...` を叩くと、
ユーザーが作業中のウィンドウへ文字が入ったりクリックが飛んだりする。CLAUDE.md の文章 (助言) では
反射的な誤爆を止められないので、コマンドを組み立てた瞬間に発火する PreToolUse で deny する。

何を止めるか
------------
- `ai-cu` (または `cargo run ...`) の起動のうち、読み取り専用と分かっているサブコマンド
  (READ_ONLY) 以外。未知のサブコマンドは「実入力かもしれない」として止める (fail-safe)
- `--dry-run` / `--help` / `--version` が付いていれば通す
- 引数なしの起動は通す (端末が無ければ usage を出して終了する設計。CLAUDE.md 参照)
- `config` は下位コマンドが読み取り (show / path / get / validate / list) のときだけ通す。
  設定ファイルを書き換える下位コマンドは、実機の設定を壊しうるので止める
- `--config` の次の語は、読み取りの語と同じ字 (`--config shot click ...`) でも常に設定のパスの値として読む
  (clap と同じ読み。D-164)

通すもの (D-051)
----------------
検証 (fmt、clippy、test、release ビルド、フィクスチャの実機テスト) を通した release ビルド
`target/release/ai-cu.exe` の起動は、実入力のサブコマンドでも通す。開発中の AI が利用者の代わりに
手動確認を行うため。`cargo run` とデバッグビルド (`target/debug/...`) は、検証前のコードを実機に
向けうるので止めたまま。起動先が文字列から分からない形 (`& $exe run ...`、`Start-Process`) も
止めたまま。実機に触れる一連の確認の前には、利用者の GO を 1 回もらう (CLAUDE.md)。
利用者の実際の設定ファイルが床になり、AI 側から allow を広げる経路は無い (D-010、D-018)。

通すのは、利用者が Claude Code を起動したシェルに環境変数 AI_CU_MANUAL_CHECK (値は空でなければ何でも)
を付けたセッションだけ。この hook は自分のプロセスの環境を見るので、コマンド文字列の中の
`AI_CU_MANUAL_CHECK=1 target/release/ai-cu.exe ...` では通らない。settings.json の env には書かない
(書くと AI が自分で opt-in できてしまう。テストで固定している)。

どこまで見るか
--------------
コマンド文字列に現れる直接の起動 (`out=$(ai-cu ...)`、`time ai-cu ...`、`cargo -q run -- ...`、
`bash -c "..."`、サブシェル、ループの本体) と、python のスクリプトの引数に書かれた ai-cu のパス
(`python docs/manual-check/mcp_client.py target/release/ai-cu.exe mcp -- ...`。D-055 の手順 19) を見る。
python の引数のうち、ai-cu という名前のフォルダ (リポジトリの根など) の絶対パスは、繋ぎの無い 1 段のコマンドで、
実在するフォルダで、隣に起動できるファイル (`<パス>.exe` など) が無いときだけ起動と見ない (フォルダは起動できない。
相対のパス、繋ぎのあるコマンド、シェルの中の段では今までどおり止める。D-165)。
同じ条件 (1 段、絶対のパス) で、`cargo run --manifest-path <M>` の M が ai-cu でない別のクレート (名前・[[bin]]・
src/bin/ の項目のどれも ai-cu らしくなく、workspace でない) を指し、`-p` / `--bin` が無ければ、ai-cu の起動と見ない (D-165)。
スクリプトの中身や、コピーした実行ファイルの起動は文字列に現れないので見えない。MCP のクライアント
(Claude Code の設定) からの起動も見えない (debug ビルドの `ai-cu mcp` は実入力の道具をバイナリ側で断る。D-055)。偶発の誤爆を止めるためのシートベルトであって、意図的な回避を止める
境界ではない。バイナリ側の実行時ガード (環境変数 AI_CU_DEV_FORBID_REAL_INPUT。D-022) は残っているが、
このリポジトリの settings.json では設定しない (D-051)。

実入力の検証は `cargo test` (モック) と、フィクスチャウィンドウ限定の `cargo test -- --ignored` で行い、
その後に release ビルドで手動確認の手順 (docs/manual-check/CHECKLIST.md) を行う。
この規則を緩めるときは docs/decisions.md に判断を追記すること。

設計の約束 (ユーザーのグローバル hook と同じ)
-------------------------------------------
1. 何があっても exit 0。deny は JSON (permissionDecision) でだけ出す
2. 当たっていない command では黙る
3. stdout は bytes で書き、reason は ASCII のみ (この PC のコンソールは CP932)
"""

import json
import os
import re
import shlex
import sys

# 実入力を行わないと分かっているサブコマンド。実キャプチャを行うのは `shot`、`pixel`、`--shot` を付けた `wait-for`
# で、撮影の許可と守りは本体が受け持つ (D-162)。足すときは docs/decisions.md に理由を書く。
# `shot` は画面を撮る (入力は送らない)。出力先の制限は無いので、撮った画像は個人情報を含みうる前提で扱う。
# `guide` は埋め込んだ手引きの文字列を出すだけ (D-048)
# `find` と `text` は UI Automation で要素の名前、矩形、文字列を読むだけで、入力は送らない。要素の名前と文字列は
# 画面上の情報だが、`windows` の題名と同じ扱い。開発シェルでは最下層 (D-022) が拒否する (D-054 の追記)
# `wait-for` は find / text と同じ読み取り (ウィンドウ、要素、文字列が出る / 消えるまで繰り返し読む) で、入力は送らない (D-068 の追記)
# `tree` は find と同じ UI Automation の読み取り (要素の木を深さで切って返す) で、入力は送らない (D-084)
# `pixel` は画面座標の点の色を読むだけで、入力は送らない。撮影と同じ許可と守りを点ごとの 1x1 に掛ける (D-146、D-162)
READ_ONLY = {
    "shot", "validate", "schema", "info", "help", "version",
    "check", "doctor", "list", "windows", "explain", "completions", "guide",
    "find", "text", "wait-for", "tree", "pixel",
}
# `config` は下位コマンドで判定する。設定を書き換えるもの (set / init / edit / reset など) は通さない
CONFIG_READ = {"show", "path", "get", "validate", "list"}
# 実入力を行う既知のサブコマンド名。オプションの値とサブコマンドを見分けるためだけに使う。
# `mcp` は常駐の MCP サーバーで、道具として実入力を送る (D-055)。`focus` は前面化 (D-050)
# `resize` は許可された窓の大きさを変える (入力は送らないが、実デスクトップの窓を変える。D-150)
# `clipboard` は利用者のクリップボードに文字を置く (入力は送らないが、元の中身は戻らない。D-157)
ACTIONS = {"run", "click", "dblclick", "double-click", "drag", "move", "type", "key", "scroll", "wait", "focus", "mcp", "close", "resize", "clipboard"}
SAFE_FLAGS = {"--help", "-h", "--version", "-V", "--dry-run"}
# サブコマンドの前に置ける値つきオプション (今は src/cli.rs の global な `--config` だけ) の写し。
# clap と同じく、次の語は中身によらず (読み取りの語でも、空でも) 値として 1 語消費する (D-164)。
# 手で揃える: src/cli.rs のテスト step_window_hint_targets_match_clap_definition (global な値つきオプションを
# config だけに固定) が落ちたら、ここにも足す。そのテストが数えないもの (global でない最上位の値つきオプション、
# `--config` の短い名前や別名) を足すときも、ここに足す
VALUE_OPTS = {"--config"}

# 次のコマンドをそのまま実行するだけの語。読み飛ばして、実際に始まる語を見る
LAUNCHERS = {
    "env", "time", "timeout", "nohup", "xargs", "exec", "command", "builtin", "call", "sudo", "nice",
    "stdbuf", "setsid", "start", "then", "do", "else", "elif", "if", "while", "until", "!",
}
# launcher の値付きオプション (timeout -s KILL 5 ai-cu ...、xargs -a FILE ai-cu ... など)
LAUNCHER_VALUE_OPTS = {"-s", "-k", "-a", "-i", "-I", "-n", "-P", "-d", "-t", "--signal", "--kill-after"}
# 引数がコマンドラインそのものになるシェル: 中身を再帰して見る
SHELLS = {"cmd", "powershell", "pwsh", "bash", "sh", "zsh", "wsl", "invoke-expression", "iex"}
# 引数を解釈できない包み。本文のどこかに ai-cu があれば止める
TEXT_WRAPPERS = {"start-process", "saps"}
# スクリプトの実行器。引数のどこかにある ai-cu のパス (手動確認の客 docs/manual-check/mcp_client.py が
# release のパスを引数に取る形。D-055) を、その起動として見る。パスが変数なら起動先が分からないので止める
SCRIPT_RUNNERS = {"python", "python3", "py"}

CARGO_GLOBAL_VALUE = {"--color", "-C", "-Z", "--config"}
CARGO_RUN_VALUE = {
    "--manifest-path", "--features", "-F", "--target", "--profile", "--target-dir", "-j", "--jobs",
    "--config", "--color", "-Z", "--message-format",
}

ENV_ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
BARE_VAR = re.compile(r"^\$\{?[A-Za-z_][\w:]*\}?$")
NUMBER = re.compile(r"^\d+(?:\.\d+)?[smhd]?$")
SLASH_OPT = re.compile(r"^/[A-Za-z]{1,3}$")
BIN_NAME = re.compile(r"^ai[-_]cu(?:-[0-9a-f]{8,})?$")
# 検証を通した release ビルドの起動 (D-051)。パスの区切りは / に揃えて小文字で比べる
RELEASE_PATH = re.compile(r"(?:^|/)target/release/ai[-_]cu(?:\.exe)?$")
# release ビルドからの実入力を通す、セッション単位の opt-in。利用者が Claude Code を起動するシェルで設定する
OPT_IN_ENV = "AI_CU_MANUAL_CHECK"
PATHLIKE = re.compile(r"[./\\:\d]")
AI_CU_IN_TEXT = re.compile(r"(?:^|[\s\"'\\/(`{=])ai-cu(?:\.exe)?(?=[\s\"')`};,|&]|$)", re.I)
DYNAMIC_CALL = re.compile(r"(?<!&)&(?!&)\s*\([^)]{0,300}ai-cu", re.I)
LINE_CONT = re.compile(r"(?:`|(?<=\s)\\)\r?\n")  # 行末の PowerShell の ` / bash の \
# ai-cu という名前のフォルダの判定 (D-165)。拡張子の無い名前に OS やシェルが補いうる拡張子 (PATHEXT も足す)
LAUNCH_EXTS = (".exe", ".com", ".bat", ".cmd", ".ps1")
FD_DUP = re.compile(r"[0-9]?>&[0-9]")  # 2>&1 などの記述子の写し (段の繋ぎではない)
REDIRECT_TARGET = re.compile(r">>?\s*(\"[^\"]*\"|'[^']*'|[^\s;&|<>]+)")
DRIVE_ABS = re.compile(r"^[A-Za-z]:[\\/]")
GIT_BASH_DRIVE = re.compile(r"^/([A-Za-z])(?=/|$)")

TOOLS = ("Bash", "PowerShell", "Monitor")

REASON = (
    "ai-cu real-input subcommand is denied here (it moves the real mouse/keyboard). "
    "Real input is allowed only through the verified release build, invoked by its path "
    "target/release/ai-cu.exe after the full /verify sequence, with the user's GO, and only in a "
    "session the user started with the environment variable %s set (D-051); "
    "`cargo run` and target/debug builds are denied. Otherwise use `cargo test` (mock backend), "
    "the fixture-only `cargo test -- --ignored`, or add --dry-run. Read-only subcommands allowed "
    "for any build: %s (config: %s only). See D-006, D-013 and D-051 in docs/decisions.md."
) % (OPT_IN_ENV, ", ".join(sorted(READ_ONLY)), ", ".join(sorted(CONFIG_READ)))


def _is_release_path(tok):
    """`target/release/ai-cu(.exe)` へのパス (相対でも絶対でも) か。"""
    p = _unquote(tok).replace("\\", "/").lower()
    return RELEASE_PATH.search(p) is not None


def manual_check_enabled(environ=None):
    """利用者がセッション単位で opt-in しているか (この hook のプロセスの環境変数で見る)。"""
    env = os.environ if environ is None else environ
    return bool((env.get(OPT_IN_ENV) or "").strip())


def _unquote(tok):
    return tok.strip().strip("\"')")


def _basename(tok):
    t = _unquote(tok).replace("\\", "/").rsplit("/", 1)[-1].lower()
    return t[:-4] if t.endswith(".exe") else t


def _naive_split(command):
    return re.split(r"[;&|\n(){}`]+|\$\(", command)


def split_segments(command):
    """引用符の外で ; & | 改行 ( ) { } ` $( を境に分ける。"..." の中では ` と $( だけで分ける。"""
    segs, cur, q, i, n = [], [], None, 0, len(command)
    while i < n:
        c = command[i]
        nxt = command[i + 1] if i + 1 < n else ""
        if q == "'":
            cur.append(c)
            if c == "'":
                q = None
        elif q == '"':
            if c == '"':
                q = None
                cur.append(c)
            elif c == "`" or (c == "$" and nxt == "("):
                segs.append("".join(cur))
                cur = []
                i += 1 if c == "$" else 0
            else:
                cur.append(c)
        elif c in "'\"":
            q = c
            cur.append(c)
        elif c in ";&|\n(){}`":
            segs.append("".join(cur))
            cur = []
        elif c == "$" and nxt == "(":
            segs.append("".join(cur))
            cur = []
            i += 1
        else:
            cur.append(c)
        i += 1
    if q is not None:  # 引用符が閉じていない: 推測せず、全部で分ける (fail-safe)
        return _naive_split(command)
    segs.append("".join(cur))
    return segs


def _tokens(segment):
    try:
        return shlex.split(segment, posix=False)
    except ValueError:
        return segment.split()


def _skip_prefix(toks):
    """環境変数の代入、launcher、`$x =` を読み飛ばして、実際にコマンドが始まる語の位置を返す。"""
    i, launched = 0, False
    while i < len(toks):
        t = _unquote(toks[i])
        low = t.lower()
        nxt = _unquote(toks[i + 1]) if i + 1 < len(toks) else ""
        if ENV_ASSIGN.match(t):
            i += 1
        elif BARE_VAR.match(t) and nxt in ("=", "+="):  # PowerShell の代入: $r = ai-cu ...
            i += 2
        elif low in LAUNCHERS:
            launched = True
            i += 1
        elif launched and t in LAUNCHER_VALUE_OPTS:
            i += 2
        elif launched and (t == "" or t.startswith("-") or NUMBER.match(t) or SLASH_OPT.match(t)):
            i += 1
        else:
            break
    return i


def split_subcommand(args):
    """最初のサブコマンドとその後ろの引数を返す。オプションの値 (--config t.toml) をサブコマンドと取り違えない。"""
    after_opt = False
    value_next = False
    for idx, a in enumerate(args):
        a = _unquote(a)
        if value_next:  # `--config <値>` の値。空の語もここで 1 語として消費する (D-164)
            value_next = False
            continue
        if a in VALUE_OPTS:
            value_next = True
            after_opt = False
            continue
        if not a:
            continue
        if a.startswith("-"):
            after_opt = "=" not in a and a not in SAFE_FLAGS
            continue
        low = a.lower()
        if low in READ_ONLY or low in ACTIONS or low == "config":
            return low, args[idx + 1:]
        if after_opt and PATHLIKE.search(a):  # パスか数値に見える: 直前のオプションの値
            after_opt = False
            continue
        return low, args[idx + 1:]  # 未知の素の語はサブコマンドとみなす (fail-safe)
    return None, []


def args_are_safe(args):
    """ai-cu に渡される引数が、読み取り専用 (または dry-run / help) と言えるか。"""
    stripped = [_unquote(a) for a in args]
    if any(a in SAFE_FLAGS for a in stripped):
        return True
    sub, rest = split_subcommand(stripped)
    if sub is None or sub in READ_ONLY:
        return True
    if sub == "config":
        nxt, _ = split_subcommand([a for a in rest])
        return nxt is None or nxt in CONFIG_READ
    return False


def release_args_ok(args):
    """検証済みの release ビルドに渡す引数。実入力は通すが、設定を書き換える config の下位コマンドは通さない。"""
    stripped = [_unquote(a) for a in args]
    sub, rest = split_subcommand(stripped)
    if sub == "config":
        nxt, _ = split_subcommand([a for a in rest])
        return nxt is None or nxt in CONFIG_READ
    return True


def _names_ai_cu(name):
    """パッケージや bin の名前が ai-cu らしいか (ai-cu-cli や ai_cu も含める)。"""
    return re.search(r"ai[-_]?cu", (name or "").lower()) is not None


DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def _uses_ai_cu(data):
    """依存の表 (target.<cfg>. の下も) に、名前か `package` が ai-cu らしい依存があるか。ライブラリ経由で実入力を送れる。"""
    tables = [data.get(k) for k in DEP_TABLES]
    for spec in (data.get("target") or {}).values():
        if isinstance(spec, dict):
            tables.extend(spec.get(k) for k in DEP_TABLES)
    for table in tables:
        if not isinstance(table, dict):
            continue
        for key, dep in table.items():
            if _names_ai_cu(key) or (isinstance(dep, dict) and _names_ai_cu(str(dep.get("package")))):
                return True
    return False


def _points_outside(node):
    """manifest のどこかの `path` が、クレートのフォルダの外 (絶対、`..` を含む) を指すか。
    外の ai-cu のソース (`[[bin]] path = "../ai-cu/src/main.rs"`) や依存を取り込めるので、別のクレートと見ない。"""
    if isinstance(node, dict):
        for k, v in node.items():
            if k == "path" and isinstance(v, str):
                p = v.replace("\\", "/")
                if DRIVE_ABS.match(p) or p.startswith("/") or ".." in p.split("/"):
                    return True
            elif _points_outside(v):
                return True
    elif isinstance(node, list):
        return any(_points_outside(x) for x in node)
    return False


def _manifest_is_other_crate(path):
    """絶対パスの Cargo.toml が、ai-cu でない別のクレートと読めるか (D-165)。
    読めない・壊れている・名前が無い・workspace・名前か [[bin]] か src/bin/ の項目が ai-cu らしいなら偽 (止める側)。"""
    try:
        import tomllib  # Python 3.11 から。無ければ例外で偽 (止める側) になり、hook 全体は落ちない

        with open(path, "rb") as f:
            data = tomllib.load(f)
        pkg = data.get("package")
        if "workspace" in data or not isinstance(pkg, dict) or not isinstance(pkg.get("name"), str):
            return False
        if _names_ai_cu(pkg["name"]):
            return False
        for b in data.get("bin") or []:
            if not isinstance(b, dict) or _names_ai_cu(str(b.get("name"))):
                return False
        if _uses_ai_cu(data) or _points_outside(data):
            return False
        bindir = os.path.join(os.path.dirname(path), "src", "bin")
        if os.path.isdir(bindir) and any(_names_ai_cu(os.path.splitext(n)[0]) for n in os.listdir(bindir)):
            return False  # cargo は src/bin/*.rs と src/bin/<名前>/main.rs を [[bin]] 無しで bin にする
        return True
    except Exception:
        return False


def _cargo_run_args(rest, relax=False, tool=None):
    """`cargo [global opts] run ...` のプログラム引数。ai-cu の cargo run でなければ None。
    relax (1 段のコマンド) で `--manifest-path` が別のクレートの絶対パスなら None (D-165)。"""
    i = 0
    while i < len(rest):
        t = _unquote(rest[i])
        if t.startswith("+"):
            i += 1
        elif t.startswith("-"):
            i += 2 if t in CARGO_GLOBAL_VALUE else 1
        else:
            break
    if i >= len(rest) or _unquote(rest[i]) not in ("run", "r"):
        return None
    opts = rest[i + 1:]
    if "--" in opts:
        k = opts.index("--")
        opts, prog = opts[:k], opts[k + 1:]
    else:
        prog = None  # cargo は `--` が無くても残りをプログラムの引数として渡す
    target, manifest, pos, j = None, None, [], 0
    while j < len(opts):
        t = _unquote(opts[j])
        if t == "--example" or t.startswith("--example="):
            return None  # example (フィクスチャウィンドウなど) であって、ai-cu ではない
        if t == "--manifest-path":
            manifest = opts[j + 1] if j + 1 < len(opts) else None
            j += 2
        elif t.startswith("--manifest-path="):
            manifest = opts[j].strip().split("=", 1)[1]
            j += 1
        elif t in ("-p", "--package", "--bin"):
            target = _unquote(opts[j + 1]) if j + 1 < len(opts) else None
            j += 2
        elif t.startswith(("--bin=", "--package=")):
            target = t.split("=", 1)[1]
            j += 1
        elif t.startswith("-p") and len(t) > 2:  # `-pai-cu` と `-p=ai-cu` (cargo はどちらもパッケージと読む)
            target = t[2:].removeprefix("=")
            j += 1
        elif t in CARGO_RUN_VALUE:
            j += 2
        elif t.startswith("-"):
            j += 1
        else:
            # cargo run は最初の位置引数より後の語を、オプションの形でもすべてプログラムに渡す。
            # 後ろの `--manifest-path` や `--bin` を cargo のものと読まない (D-165)
            pos.extend(_unquote(x) for x in opts[j:])
            break
    if target and not _names_ai_cu(target):
        return None  # 別のパッケージや bin (xtask、フィクスチャなど)。ai-cu-cli のような名前は ai-cu 扱いのまま
    if not target and relax and manifest:
        local = _absolute_local(manifest, tool)  # 絶対の確かめは manifest の判定の外で先に行う
        if local is not None and _manifest_is_other_crate(local):
            return None  # 作業領域の使い捨てのクレートなど、ai-cu でない別のクレート (D-165)
    return prog if prog is not None else pos


def invocations(command, depth=0, tool=None):
    """command 中の ai-cu 起動を (kind, args) で列挙する。kind: 'direct' | 'release' | 'wrapped'。
    'release' は検証済みの release ビルドのパスからの起動 (D-051)。tool は Bash / PowerShell などの道具の名前。"""
    found = []
    command = LINE_CONT.sub(" ", command or "")
    if depth == 0 and DYNAMIC_CALL.search(command):
        found.append(("wrapped", []))
    relax = depth == 0 and _single_segment(command)  # シェルの中の段では緩めない (D-165)
    for seg in split_segments(command):
        toks = _tokens(seg)
        i = _skip_prefix(toks)
        if i >= len(toks):
            continue
        head_raw = _unquote(toks[i])
        head = _basename(toks[i])
        rest = toks[i + 1:]
        if BIN_NAME.match(head):
            found.append(("release" if _is_release_path(toks[i]) else "direct", rest))
        elif BARE_VAR.match(head_raw):  # `& $exe run ...` / `$AI run ...`: 起動先が分からない
            sub, _ = split_subcommand([_unquote(a) for a in rest])
            if sub in ACTIONS:  # `$a -join ','` などは通す
                found.append(("wrapped", []))
        elif head == "cargo":
            args = _cargo_run_args(rest, relax, tool)
            if args is not None:
                found.append(("direct", args))
        elif head in SHELLS and depth < 3:
            payload = list(rest)
            while payload and re.match(r"^[-/][A-Za-z]+$", _unquote(payload[0])):
                payload.pop(0)  # -lc、-Command、/c など
            inner = " ".join(_unquote(t) for t in payload)
            found.extend(invocations(inner, depth + 1, tool))
        elif head in TEXT_WRAPPERS and AI_CU_IN_TEXT.search(seg):
            found.append(("wrapped", []))
        elif head in SCRIPT_RUNNERS:
            found.extend(_script_runner_invocations(rest, relax, tool))
    return found


def _single_segment(command):
    """繋ぎ (; && || | 改行 $( など) の無い 1 段のコマンドか (D-165)。`2>&1` のような記述子の写しは数えない。
    1 段でも、リダイレクトの先が ai-cu の名前 (拡張子を除く) なら、起動の前に隣のファイルを作れるので偽。"""
    if "\r" in command:  # PowerShell は単独の CR でも文を分ける (split_segments は分けない)
        return False
    text = FD_DUP.sub(" ", command)
    if len([s for s in split_segments(text) if s.strip()]) != 1:
        return False
    for m in REDIRECT_TARGET.finditer(text):
        if BIN_NAME.match(os.path.splitext(_basename(m.group(1)))[0]):
            return False
    return True


def _absolute_local(tok, tool):
    """ドライブつきの絶対パス (`X:/…` か `X:\\…`) ならそのパス、そうでなければ None (D-165)。
    Git Bash の `/x/…` は PowerShell 以外で `X:/…` に直す。Bash では引用符の無い `\\` はエスケープとして消えるので、
    引用符の無い逆スラッシュの語は見ない。UNC、ドライブ相対 (`X:foo`)、相対は None。"""
    raw = tok.strip()
    quoted = len(raw) >= 2 and raw[0] == raw[-1] and raw[0] in "\"'"
    if tool != "PowerShell" and "\\" in raw and not quoted:
        return None
    p = _unquote(raw)
    if tool != "PowerShell":
        p = GIT_BASH_DRIVE.sub(lambda m: m.group(1).upper() + ":", p, count=1)
    return p if DRIVE_ABS.match(p) else None


def _launch_exts():
    """拡張子の無い名前に OS やシェルが補いうる拡張子: 固定の一覧と、このプロセスの PATHEXT の和 (D-165)。"""
    exts = set(LAUNCH_EXTS)
    for e in (os.environ.get("PATHEXT") or "").split(";"):
        e = e.strip().lower()
        if e.startswith("."):
            exts.add(e)
    return exts


def _is_directory_not_binary(tok, tool):
    """絶対パスが実在するフォルダで、隣に起動できるファイル (`<パス>.exe` など) が無いか (D-165)。
    フォルダは起動できない。読めない・判定できないときは偽 (止める側)。"""
    try:
        p = _absolute_local(tok, tool)
        if p is None or not os.path.isdir(p):
            return False
        base = p.rstrip("/\\")
        return not any(os.path.exists(base + e) for e in _launch_exts())
    except Exception:
        return False


def _script_runner_invocations(rest, relax=False, tool=None):
    """`python client.py <ai-cu のパス> <引数...>` の形: 引数の中の ai-cu のパスを起動として見る。
    パスが変数 (`$cu mcp`) で、その後ろが実入力のサブコマンドなら、起動先が分からないので止める。
    relax (1 段のコマンド) のときは、ai-cu という名前のフォルダの絶対パスを起動と見ずに次の語へ進む (D-165)。"""
    for idx, tok in enumerate(rest):
        raw = _unquote(tok)
        if BIN_NAME.match(_basename(tok)):
            if relax and _is_directory_not_binary(tok, tool):
                continue
            kind = "release" if _is_release_path(tok) else "direct"
            return [(kind, rest[idx + 1:])]
        if BARE_VAR.match(raw):
            sub, _ = split_subcommand([_unquote(a) for a in rest[idx + 1:]])
            if sub in ACTIONS:
                return [("wrapped", [])]
    return []


def violation(command, environ=None, tool=None):
    """deny すべき起動があれば True。release ビルドの実入力は、opt-in したセッションでだけ通す。"""
    opted_in = manual_check_enabled(environ)
    for kind, args in invocations(command, tool=tool):
        if kind == "wrapped":
            return True
        if kind == "release" and opted_in:
            if not release_args_ok(args):
                return True
        elif not args_are_safe(args):
            return True
    return False


def decide(event, environ=None):
    if event.get("hook_event_name") != "PreToolUse":
        return None
    if event.get("tool_name") not in TOOLS:
        return None
    command = (event.get("tool_input") or {}).get("command") or ""
    if not violation(command, environ, event.get("tool_name")):
        return None
    return {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": REASON,
        }
    }


def main():
    raw = sys.stdin.buffer.read().decode("utf-8", "replace")
    # PowerShell 経由や BOM 付き file 経由だと先頭に U+FEFF が付く
    event = json.loads(raw.lstrip("﻿ \r\n\t"))
    payload = decide(event)
    if payload is None:
        return
    sys.stdout.buffer.write(json.dumps(payload, ensure_ascii=True).encode("ascii"))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        pass  # 何が起きても Bash / PowerShell を止めない (約束 1)
    sys.exit(0)
