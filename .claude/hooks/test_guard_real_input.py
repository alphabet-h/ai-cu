"""guard_real_input.py のテスト。実行: python .claude/hooks/test_guard_real_input.py"""

import json
import os
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True  # __pycache__ を作らない (git に混ざるのを防ぐ)

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)

import guard_real_input as g  # noqa: E402


def event(cmd, tool="Bash"):
    return {"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd}}


class Denied(unittest.TestCase):
    CASES = [
        # 素直な起動
        "ai-cu run actions.json",
        "ai-cu click 10 20",
        "ai-cu drag 1 2 3 4",
        "target/debug/ai-cu run a.json",
        r"target\debug\ai-cu.exe run a.json",
        "RUST_LOG=debug ai-cu run a.json",
        # release ビルドでも、設定を書き換える config の下位コマンドと、起動先の分からない形は止める (D-051)
        "target/release/ai-cu.exe config set allow.keys x",
        "release/ai-cu.exe run a.json",
        "target/release/tools/ai-cu.exe run a.json",
        "target/release-fast/ai-cu.exe run a.json",
        "cargo run --release -- run a.json",
        "cargo run --release -- shot --out x.png --region 0,0,10,10 && target/debug/ai-cu.exe run a.json",
        "cd /tmp && ai-cu run a.json",
        "echo hi; ai-cu key ctrl+s",
        "ai-cu somethingnew",  # 未知のサブコマンドは fail-safe で止める
        "ai-cu --config x.toml run a.json",  # 値付きオプションを前置しても、実入力は止める
        "ai-cu -v hotkey ctrl+c",
        # MCP サーバーは常駐して実入力を送る (D-055)。debug と cargo run からは止める
        "ai-cu mcp",
        "cargo run -- mcp",
        "target/debug/ai-cu.exe mcp",
        "ai-cu --config t.toml mcp",
        "ai-cu focus notepad.exe",
        # python の客 (docs/manual-check/mcp_client.py) の引数に ai-cu があれば、その起動として見る
        "python docs/manual-check/mcp_client.py target/debug/ai-cu.exe mcp -- docs/manual-check/mcp-requests.jsonl",
        "python docs/manual-check/mcp_client.py ai-cu mcp -- x.jsonl",
        "python -I docs/manual-check/mcp_client.py target/debug/ai-cu.exe mcp -- x.jsonl --timeout 30",
        "py -3 docs/manual-check/mcp_client.py target/debug/ai-cu.exe mcp -- x.jsonl",
        "python docs/manual-check/mcp_client.py $cu mcp -- x.jsonl",  # 起動先が分からない
        # cargo run (グローバルオプション、-- の省略、パッケージ指定)
        "cargo run -- run a.json",
        "cargo run --release -- click 1 2",
        "cargo +nightly run -- type hello",
        "cargo -q run -- run a.json",
        "cargo --quiet run -- run a.json",
        "cargo --locked run -- click 1 2",
        "cargo --color never run -- run a.json",
        "cargo -q r -- click 1 2",
        "cargo run --release click 10 20",  # `--` が無くても cargo は引数を渡す
        "cargo run -p ai-cu run a.json",
        "cargo run --bin ai-cu click 1 2",
        "cargo run --manifest-path Cargo.toml --bin ai-cu -- run a.json",
        # 先頭の語ではない起動
        "out=$(ai-cu run a.json)",
        "echo $(ai-cu click 1 2)",
        "echo `ai-cu run a.json`",
        "$r = ai-cu run a.json | ConvertFrom-Json",
        r"$out = .\target\debug\ai-cu.exe run a.json",
        "time ai-cu run a.json",
        "timeout 5 ai-cu run a.json",
        "timeout -s KILL 5 ai-cu run a.json",
        "env FOO=1 ai-cu run a.json",
        "nohup ai-cu run a.json",
        "echo a.json | xargs ai-cu run",
        "echo a.json | xargs -a list.txt ai-cu run",
        "(ai-cu run a.json)",
        "{ ai-cu run a.json; }",
        "for f in a b; do ai-cu run $f.json; done",
        "if true; then ai-cu click 1 2; fi",
        "Get-ChildItem *.json | ForEach-Object { ai-cu run $_ }",
        "git -C D:/x status && ai-cu run a.json",
        '& (Join-Path target ai-cu.exe) run a.json',
        '& $exe run a.json',
        # シェルや包みの中
        "bash -c 'target/debug/ai-cu run a.json'",
        "bash -lc \"cargo run -- run a.json\"",
        'cmd /c "target\\debug\\ai-cu.exe run a.json"',
        'powershell -NoProfile -Command "ai-cu run a.json"',
        'Start-Process "target\\debug\\ai-cu.exe" -ArgumentList "run a.json"',
        # config: 設定を書き換える下位コマンドは止める
        "ai-cu config set allow.keys x",
        "ai-cu config init",
        "ai-cu config edit",
    ]

    def test_denied(self):
        for cmd in self.CASES:
            with self.subTest(cmd=cmd):
                self.assertTrue(g.violation(cmd), cmd)
                self.assertIsNotNone(g.decide(event(cmd)))

    def test_every_covered_tool_is_denied(self):
        for tool in ("Bash", "PowerShell", "Monitor"):
            with self.subTest(tool=tool):
                self.assertIsNotNone(g.decide(event("ai-cu run a.json", tool=tool)))


class Allowed(unittest.TestCase):
    CASES = [
        # ビルドと検索
        "cargo test",
        "cargo test -- --ignored",
        "cargo test -p ai-cu",
        "cargo build --release",
        "cargo build -p ai-cu",
        "cargo clippy --all-targets -- -D warnings",
        'rg "ai-cu run" docs',
        'rg "shot|ai-cu run" docs',
        "echo ai-cu run a.json",
        "ls target/debug/ai-cu.exe",
        'Select-String ai-cu README.md',
        'bash -lc "rg ai-cu src"',
        'sh -c "cargo build -p ai-cu"',
        'powershell -Command "Select-String ai-cu README.md"',
        r"cmd /c dir target\debug\ai-cu.exe",
        "git -C C:/src/ai-cu status",
        "python .claude/hooks/test_guard_real_input.py",
        "python docs/manual-check/mcp_client.py",
        "python docs/manual-check/mcp_client.py target/release/ai-cu.exe guide -- x.jsonl",
        "python docs/manual-check/mcp_client.py target/release/ai-cu.exe --help",
        "python scripts/report.py --out target/ai-cu-report.txt",
        "bash scripts/build.sh",
        "",
        # 別のバイナリ (フィクスチャウィンドウなど)
        "cargo run --example fixture -- --size 400x300",
        "cargo run --bin other -- gen",
        "cargo run -p xtask -- build",
        "cargo run -p fixture -- --size 400x300",
        # 引数なし、help、version、dry-run
        "cargo run",
        "ai-cu",
        "ai-cu --help",
        "ai-cu --version",
        "ai-cu -V",
        "ai-cu run --help",
        "ai-cu run actions.json --dry-run",
        "cargo run -- --help",
        # 読み取り専用のサブコマンド (設定のパスを前置してもよい)
        "ai-cu shot out.png",
        "ai-cu validate actions.json",
        "ai-cu schema",
        "ai-cu check actions.json",
        "ai-cu doctor",
        "ai-cu list",
        "ai-cu windows",
        "ai-cu guide",
        "target/release/ai-cu.exe guide",
        # find と text は UI Automation で読むだけ (D-054 の追記)
        "ai-cu find --window notepad.exe --type button",
        "cargo run -- text --window notepad.exe --type document",
        "target/debug/ai-cu find --window notepad.exe --name 保存",
        # wait-for は find / text と同じく UI Automation で読むだけで、入力を送らない (D-068 の追記)
        "ai-cu wait-for notepad.exe --name 保存",
        "cargo run -- wait-for notepad.exe --text 一手ずつ --type document --timeout-ms 3000",
        "target/debug/ai-cu wait-for notepad.exe --title zzz --absent --timeout-ms 1000",
        # tree は find と同じく UI Automation で読むだけ (D-084)
        "ai-cu tree --window notepad.exe",
        "cargo run -- tree --window notepad.exe --depth 3",
        "target/debug/ai-cu tree --window notepad.exe --all",
        "ai-cu explain actions.json",
        "ai-cu completions powershell",
        "target/debug/ai-cu validate a.json",
        "ai-cu --config t.toml validate x.json",
        "ai-cu -c t.toml validate x.json",
        "ai-cu --config target/t.toml schema",
        "ai-cu --config=target/t.toml validate x.json",
        "ai-cu validate --config t.toml x.json",
        "AI_CU_CONFIG=target/t.toml ai-cu validate x.json",
        "cargo run -- --config target/t.toml validate a.json",
        "cargo run -- shot out.png",
        "ai-cu config",
        "ai-cu config show",
        "ai-cu config path",
        "ai-cu config get allow.keys",
        "ai-cu --config target/t.toml config show",
        "target/release/ai-cu.exe config show",
        "target/release/ai-cu.exe run a.json --dry-run",
        # 読み取り専用は release のパスでも opt-in に関係なく通る
        "./target/release/ai-cu.exe shot --out target/x.png --region 0,0,800,600",
        "out=$(target/release/ai-cu.exe windows)",
    ]

    def test_allowed(self):
        for cmd in self.CASES:
            with self.subTest(cmd=cmd):
                self.assertFalse(g.violation(cmd, environ={}), cmd)
                self.assertIsNone(g.decide(event(cmd), environ={}))


class ReleaseOptIn(unittest.TestCase):
    """検証済みの release ビルドのパスからの実入力は、利用者が opt-in したセッションでだけ通す (D-051)。"""

    OPTED = {g.OPT_IN_ENV: "1"}
    CASES = [
        "target/release/ai-cu.exe run a.json",
        "target/release/ai-cu run a.json",
        "C:/src/ai-cu/target/release/ai-cu.exe run docs/manual-check/sample.json",
        r'& "C:\src\ai-cu\target\release\ai-cu.exe" run a.json',
        r"& .\target\release\ai-cu.exe run a.json; $LASTEXITCODE",
        "Start-Sleep -Seconds 4; target/release/ai-cu.exe run a.json; echo exit=$?",
        "target/release/ai-cu.exe --config=target/t.toml run a.json",
        "bash -c 'target/release/ai-cu.exe run a.json'",
        "target/release/ai-cu.exe key ctrl+s",
        # MCP サーバー (D-055) と、python の客を通した release の起動 (手順 19)
        "target/release/ai-cu.exe mcp",
        "C:/src/ai-cu/target/release/ai-cu.exe mcp",
        "python docs/manual-check/mcp_client.py target/release/ai-cu.exe mcp -- docs/manual-check/mcp-requests.jsonl --timeout 30",
        r"python C:\src\ai-cu\docs\manual-check\mcp_client.py C:\src\ai-cu\target\release\ai-cu.exe mcp -- C:\src\ai-cu\docs\manual-check\mcp-requests-input.jsonl",
        "python -I docs/manual-check/mcp_client.py target/release/ai-cu.exe --config=target/t.toml mcp -- x.jsonl",
    ]

    def test_allowed_when_opted_in(self):
        for cmd in self.CASES:
            with self.subTest(cmd=cmd):
                self.assertFalse(g.violation(cmd, environ=self.OPTED), cmd)
                self.assertIsNone(g.decide(event(cmd), environ=self.OPTED))

    def test_denied_without_opt_in(self):
        for env in ({}, {g.OPT_IN_ENV: ""}, {g.OPT_IN_ENV: "  "}):
            for cmd in self.CASES:
                with self.subTest(cmd=cmd, env=env):
                    self.assertTrue(g.violation(cmd, environ=env), cmd)
                    self.assertIsNotNone(g.decide(event(cmd), environ=env))

    def test_inline_assignment_in_command_does_not_opt_in(self):
        cmd = "AI_CU_MANUAL_CHECK=1 target/release/ai-cu.exe run a.json"
        self.assertTrue(g.violation(cmd, environ={}), cmd)

    def test_opt_in_does_not_widen_other_builds_or_config_writes(self):
        for cmd in (
            "target/debug/ai-cu.exe run a.json",
            "cargo run --release -- run a.json",
            "ai-cu run a.json",
            "& $exe run a.json",
            "target/release/ai-cu.exe config set allow.keys x",
            "target/debug/ai-cu.exe mcp",
            "cargo run --release -- mcp",
            "python docs/manual-check/mcp_client.py target/debug/ai-cu.exe mcp -- x.jsonl",
            "python docs/manual-check/mcp_client.py $cu mcp -- x.jsonl",
            "python docs/manual-check/mcp_client.py target/release/ai-cu.exe config set a b -- x.jsonl",
        ):
            with self.subTest(cmd=cmd):
                self.assertTrue(g.violation(cmd, environ=self.OPTED), cmd)


class Events(unittest.TestCase):
    def test_other_tools_and_events_are_ignored(self):
        self.assertIsNone(g.decide({"hook_event_name": "PreToolUse", "tool_name": "Edit", "tool_input": {}}))
        self.assertIsNone(g.decide({"hook_event_name": "PostToolUse", "tool_name": "Bash",
                                    "tool_input": {"command": "ai-cu run a.json"}}))
        self.assertIsNone(g.decide({}))

    def test_missing_command_is_ignored(self):
        self.assertIsNone(g.decide({"hook_event_name": "PreToolUse", "tool_name": "Monitor", "tool_input": {}}))

    def test_reason_is_ascii(self):
        payload = g.decide(event("ai-cu run a.json"))
        reason = payload["hookSpecificOutput"]["permissionDecisionReason"]
        reason.encode("ascii")
        self.assertEqual(payload["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_no_pathological_slowdown(self):
        # DYNAMIC_CALL の正規表現が、長い入力で二乗時間にならないこと
        import time
        cmd = "& (" + "x" * 200000
        t0 = time.time()
        g.violation(cmd)
        self.assertLess(time.time() - t0, 5.0)


class Process(unittest.TestCase):
    """実際に stdin から JSON を渡して、終了コードと出力を確かめる。"""

    SCRIPT = os.path.join(HERE, "guard_real_input.py")

    def run_guard(self, raw_bytes, env=None):
        return subprocess.run([sys.executable, "-B", self.SCRIPT], input=raw_bytes, capture_output=True, timeout=20,
                              env=env)

    def test_release_real_input_needs_opt_in_in_process_env(self):
        raw = json.dumps(event("target/release/ai-cu.exe run a.json")).encode("utf-8")
        base = {k: v for k, v in os.environ.items() if k != g.OPT_IN_ENV}
        p = self.run_guard(raw, env=base)
        self.assertEqual(p.returncode, 0)
        self.assertIn(b"deny", p.stdout)
        p = self.run_guard(raw, env={**base, g.OPT_IN_ENV: "1"})
        self.assertEqual((p.returncode, p.stdout), (0, b""))

    def test_deny_json_and_exit_zero(self):
        raw = json.dumps(event("ai-cu run a.json")).encode("utf-8")
        p = self.run_guard(raw)
        self.assertEqual(p.returncode, 0)
        out = json.loads(p.stdout.decode("ascii"))
        self.assertEqual(out["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_silent_when_allowed(self):
        p = self.run_guard(json.dumps(event("cargo test")).encode("utf-8"))
        self.assertEqual((p.returncode, p.stdout), (0, b""))

    def test_bom_is_tolerated(self):
        p = self.run_guard(b"\xef\xbb\xbf" + json.dumps(event("ai-cu run a.json")).encode("utf-8"))
        self.assertEqual(p.returncode, 0)
        self.assertIn(b"deny", p.stdout)

    def test_garbage_input_never_blocks(self):
        for raw in (b"", b"not json", b"\xff\xfe\x00"):
            with self.subTest(raw=raw):
                p = self.run_guard(raw)
                self.assertEqual((p.returncode, p.stdout), (0, b""))


class Wiring(unittest.TestCase):
    """settings.json の hook 設定が、実在するスクリプトを指し、実際に deny を返すこと。"""

    def setUp(self):
        with open(os.path.join(ROOT, ".claude", "settings.json"), encoding="utf-8") as fh:
            self.settings = json.load(fh)

    def test_opt_in_is_not_set_in_settings_env(self):
        # 利用者がシェルで設定する変数。settings.json に書くと AI が自分で opt-in できてしまう (D-051)
        self.assertNotIn(g.OPT_IN_ENV, self.settings.get("env", {}))
        self.assertNotIn("AI_CU_DEV_FORBID_REAL_INPUT", self.settings.get("env", {}), "D-051 で外した")

    def test_pretooluse_hook_points_at_this_script(self):
        entries = self.settings["hooks"]["PreToolUse"]
        guard = [e for e in entries for h in e["hooks"] if "guard_real_input.py" in h["command"]]
        self.assertEqual(len(guard), 1, "guard hook must be registered exactly once")
        matcher = guard[0]["matcher"]
        for tool in g.TOOLS:
            self.assertIn(tool, matcher.split("|"), tool + " must be covered by the hook matcher")
        command = [h["command"] for h in guard[0]["hooks"] if "guard_real_input.py" in h["command"]][0]
        self.assertIn("${CLAUDE_PROJECT_DIR}", command)
        script = command.split('"')[1].replace("${CLAUDE_PROJECT_DIR}", ROOT)
        self.assertTrue(os.path.isfile(script), script)
        p = subprocess.run([sys.executable, "-B", script], input=json.dumps(event("ai-cu run a.json")).encode(),
                           capture_output=True, timeout=20)
        self.assertEqual(p.returncode, 0)
        self.assertIn(b"deny", p.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
