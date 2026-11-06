"""scripts/verify.py の判定と手順のテスト。一時ディレクトリだけを使い、cargo は呼ばない。

使い方: python scripts/test_verify.py
"""

import contextlib
import io
import json
import re
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path
from unittest import mock

sys.dont_write_bytecode = True  # __pycache__ を作らない (書くのは import する側なので、ここで止める)

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import verify  # noqa: E402

SKILL_MD = ROOT / ".claude" / "skills" / "verify" / "SKILL.md"


def pid_alive(pid: int) -> bool:
    r = subprocess.run(
        ["tasklist", "/FI", f"PID eq {pid}", "/NH"], capture_output=True, timeout=30
    )
    text = r.stdout.decode("utf-8", errors="replace")
    return re.search(rf"\b{pid}\b", text) is not None


class JudgeTest(unittest.TestCase):
    def test_judge_test_ok_with_doctest_zero_line(self):
        log = (
            "test result: ok. 12 passed; 0 failed; 0 ignored\n"
            "test result: ok. 0 passed; 0 failed; 0 ignored\n"
        )
        self.assertEqual(verify.judge("test", 0, log), (True, "ok"))

    def test_judge_test_fails_without_result_lines(self):
        self.assertEqual(verify.judge("test", 0, ""), (False, "test result の行が無い"))

    def test_judge_test_fails_on_zero_total(self):
        log = "test result: ok. 0 passed; 0 failed; 5 ignored\n"
        for kind in ("test", "ignored"):
            self.assertEqual(verify.judge(kind, 0, log), (False, "走った件数が 0"), kind)

    def test_judge_test_fails_on_failed_line(self):
        log = "test foo ... FAILED\ntest result: ok. 3 passed; 0 failed; 0 ignored\n"
        self.assertEqual(verify.judge("test", 0, log), (False, "FAILED か panicked の行がある"))

    def test_judge_test_fails_on_not_ok_result(self):
        log = "test result: FAILED. 3 passed; 1 failed; 0 ignored\n"
        for kind in ("test", "ignored"):
            self.assertEqual(verify.judge(kind, 0, log), (False, "test result が ok でない"), kind)

    def test_judge_nonzero_exit_and_timeout(self):
        self.assertEqual(verify.judge("test", 101, ""), (False, "終了コード 101"))
        self.assertEqual(verify.judge("plain", None, ""), (False, "時間切れ"))

    def test_judge_unittest_needs_ok(self):
        self.assertEqual(verify.judge("unittest", 0, "...\nRan 3 tests\n\nOK\n"), (True, "ok"))
        ok, reason = verify.judge("unittest", 0, "Ran 3 tests\n\nFAILED (failures=1)\n")
        self.assertFalse(ok)
        self.assertEqual(reason, "unittest の OK が無い")


class StepsTest(unittest.TestCase):
    def test_steps_quick_and_ignored(self):
        names = lambda ss: [s.name for s in ss]  # noqa: E731
        self.assertEqual(names(verify.steps("py", quick=True, ignored=False)), ["fmt", "clippy", "test"])
        default = verify.steps("py", quick=False, ignored=False)
        self.assertEqual(names(default), ["fmt", "clippy", "test", "hook", "release"])
        full = verify.steps("py", quick=False, ignored=True)
        self.assertEqual(names(full)[-1], "ignored")
        for s in full:
            self.assertEqual(s.log, f"{s.name}.log")
        self.assertEqual(
            [s.kind for s in full], ["plain", "plain", "test", "unittest", "plain", "ignored"]
        )

    def test_steps_match_skill_md(self):
        skill = SKILL_MD.read_text(encoding="utf-8")
        full = verify.steps("python", quick=False, ignored=True)
        for s in full:
            self.assertIn(" ".join(s.cmd), skill, s.name)
            self.assertIn(f"target/verify/{s.log}", skill, s.name)
            if s.tail is not None:
                self.assertIn(f"tail -n {s.tail} target/verify/{s.log}", skill, s.name)
            if s.pattern is not None:
                self.assertIn(f'grep -E "{s.pattern}" target/verify/{s.log}', skill, s.name)
        in_skill = set(re.findall(r"target/verify/(\w+)\.log", skill)) - {"install"}
        self.assertEqual(in_skill, {Path(s.log).stem for s in full})

    def test_steps_cargo_flag(self):
        full = verify.steps("py", quick=False, ignored=True)
        self.assertEqual(
            {s.name: s.cargo for s in full},
            {
                "fmt": True,
                "clippy": True,
                "test": True,
                "hook": False,
                "release": True,
                "ignored": True,
            },
        )

    def test_excerpt_pattern_and_tail(self):
        log = "a\ntest result: ok. 1 passed\nb\ntest x ... FAILED\nc\ntest result: ok. 2 passed\n"
        step = verify.Step("t", [], "t.log", "test", tail=None, pattern="test result|FAILED")
        self.assertEqual(
            verify.excerpt(step, log),
            ["test result: ok. 1 passed", "test x ... FAILED", "test result: ok. 2 passed"],
        )
        step = verify.Step("p", [], "p.log", "plain", tail=5)
        log8 = "\n".join(f"line{i}" for i in range(1, 9)) + "\n"
        self.assertEqual(verify.excerpt(step, log8), [f"line{i}" for i in range(4, 9)])


class RunAllTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.dir = Path(self.tmp.name)
        self.root = self.dir / "root"
        self.root.mkdir()

    def tearDown(self):
        self.tmp.cleanup()

    def script(self, name: str, body: str) -> list[str]:
        path = self.dir / name
        path.write_text(textwrap.dedent(body), encoding="utf-8")
        return [sys.executable, str(path)]

    def run_all(self, steps, timeout=30.0, out=None, **kw) -> tuple[int, str]:
        out = io.StringIO() if out is None else out
        kw.setdefault("marker", lambda root: None)  # 本物の git を起動しない (plan の全体の制約)
        code = verify.run_all(self.root, steps, timeout, out=out, **kw)
        return code, out.getvalue()

    def test_run_all_stops_at_first_failure_and_writes_logs(self):
        mark = self.dir / "mark.txt"
        steps = [
            verify.Step("one", self.script("one.py", 'print("hello")\n'), "one.log", "plain"),
            verify.Step("two", self.script("two.py", "import sys\nsys.exit(3)\n"), "two.log", "plain"),
            verify.Step(
                "three",
                self.script("three.py", f"open({str(mark)!r}, 'w').write('x')\n"),
                "three.log",
                "plain",
            ),
        ]
        code, out = self.run_all(steps)
        self.assertEqual(code, 1)
        logs = self.root / "target" / "verify"
        self.assertTrue((logs / "one.log").exists())
        self.assertTrue((logs / "two.log").exists())
        self.assertFalse(mark.exists())
        self.assertIn("hello", out)
        self.assertIn("exit=3", out)
        self.assertIn("判定: NG (終了コード 3)", out)
        self.assertIn("未実行", out)
        self.assertIn("three: 未実行", out)

    def test_run_all_reports_timeout_and_kills_grandchild(self):
        pid_file = self.dir / "grandchild.pid"
        body = f"""\
            import subprocess, sys, time
            g = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
            open({str(pid_file)!r}, "w").write(str(g.pid))
            time.sleep(30)
        """
        steps = [verify.Step("slow", self.script("slow.py", body), "slow.log", "plain")]
        code, out = self.run_all(steps, timeout=2)
        self.assertEqual(code, 1)
        self.assertIn("exit=timeout", out)
        self.assertIn("時間切れ", out)
        pid = int(pid_file.read_text())
        deadline = time.monotonic() + 1.0
        alive = pid_alive(pid)
        while alive and time.monotonic() < deadline:
            time.sleep(0.1)
            alive = pid_alive(pid)
        self.assertFalse(alive, f"孫 (pid {pid}) が残っている")

    def test_run_all_returns_even_if_killer_fails(self):
        steps = [verify.Step("slow", self.script("s.py", "import time\ntime.sleep(30)\n"), "s.log", "plain")]
        start = time.monotonic()
        code, out = self.run_all(steps, timeout=1, killer=lambda pid: None)
        self.assertEqual(code, 1)
        # proc.kill() が効けば wait(10) を待たずに戻る。無ければ 1 + 10 秒かかり、止まらない扱いになる
        self.assertLess(time.monotonic() - start, 8)
        self.assertIn("時間切れ", out)
        self.assertNotIn("プロセスが止まらない", out)

    def test_run_all_reports_stuck_when_process_survives_kill(self):
        pids: list[int] = []
        steps = [verify.Step("slow", self.script("st.py", "import time\ntime.sleep(20)\n"), "st.log", "plain")]
        try:
            with mock.patch.object(subprocess.Popen, "kill", lambda self: None):
                code, out = self.run_all(steps, timeout=1, killer=pids.append)
        finally:
            for pid in pids:
                verify.kill_tree(pid)
        self.assertEqual(code, 1)
        self.assertIn("判定: NG (時間切れ、プロセスが止まらない)", out)

    def test_run_all_warns_on_ignored_timeout(self):
        steps = [
            verify.Step(
                "ignored", self.script("i.py", "import time\ntime.sleep(30)\n"), "ignored.log", "ignored"
            )
        ]
        code, out = self.run_all(steps, timeout=1)
        self.assertEqual(code, 1)
        self.assertIn("押されたままのキーとボタンを確かめる", out)

    def test_run_all_child_utf8_output_is_readable(self):
        steps = [verify.Step("u", self.script("u.py", 'print("日本語の失敗の文")\n'), "u.log", "plain")]
        code, out = self.run_all(steps)
        self.assertEqual(code, 0)
        log = (self.root / "target" / "verify" / "u.log").read_bytes().decode("utf-8")
        self.assertIn("日本語の失敗の文", log)
        self.assertIn("日本語の失敗の文", out)
        self.assertNotIn("�", out)

    def env_steps(self) -> list:
        """子が自分の CARGO_BUILD_JOBS を印字する 2 手順 (cargo の手順と、そうでない手順)"""
        body = 'import os\nprint(os.environ.get("CARGO_BUILD_JOBS"))\n'
        return [
            verify.Step("c", self.script("c.py", body), "c.log", "plain", cargo=True),
            verify.Step("h", self.script("h.py", body), "h.log", "plain", cargo=False),
        ]

    def log_text(self, name: str) -> str:
        return (self.root / "target" / "verify" / name).read_text(encoding="utf-8").strip()

    def test_run_all_passes_jobs_only_to_cargo_steps(self):
        with mock.patch.dict(verify.os.environ):
            verify.os.environ.pop("CARGO_BUILD_JOBS", None)
            code, out = self.run_all(self.env_steps(), jobs=3)
        self.assertEqual(code, 0)
        self.assertEqual(self.log_text("c.log"), "3")
        self.assertEqual(self.log_text("h.log"), "None")
        # 出力の頭に 1 回だけ (spec の「出力の頭に 1 行」)
        self.assertEqual(out.splitlines()[0], "jobs=3 (--jobs)")
        self.assertEqual(out.count("jobs="), 1)

    def test_run_all_without_jobs_keeps_env(self):
        with mock.patch.dict(verify.os.environ, {"CARGO_BUILD_JOBS": "5"}):
            code, out = self.run_all(self.env_steps())
        self.assertEqual(code, 0)
        self.assertEqual(self.log_text("c.log"), "5")
        self.assertEqual(self.log_text("h.log"), "5")
        self.assertIn("jobs=5 (環境の CARGO_BUILD_JOBS)", out)

    def test_run_all_without_jobs_or_env_says_default(self):
        with mock.patch.dict(verify.os.environ):
            verify.os.environ.pop("CARGO_BUILD_JOBS", None)
            code, out = self.run_all(self.env_steps())
        self.assertEqual(code, 0)
        self.assertEqual(self.log_text("c.log"), "None")
        self.assertIn("jobs=cargo の既定", out)

    # --- 記録と --continue (D-083、plan ステップ 2) ---

    HEAD = "abc1234def"

    @property
    def state_path(self) -> Path:
        return self.root / "target" / "verify" / "state.json"

    def read_state(self) -> dict:
        return json.loads(self.state_path.read_text(encoding="utf-8"))

    def put_state(self, head: str, entries: list[dict]) -> None:
        self.state_path.parent.mkdir(parents=True, exist_ok=True)
        data = {"version": 1, "head": head, "steps": entries}
        self.state_path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")

    def touch_step(self, name: str, ok: bool = True) -> tuple[verify.Step, Path]:
        """走ると印のファイルを作り、行を 1 つ出す手順。ok でなければ exit 1"""
        mark = self.dir / f"{name}.ran"
        body = f"open({str(mark)!r}, 'w').write('x')\nprint('out of {name}')\n"
        if not ok:
            body += "import sys\nsys.exit(1)\n"
        step = verify.Step(name, self.script(f"{name}.py", body), f"{name}.log", "plain")
        return step, mark

    def old_entry(self, step: verify.Step, finished_at="2000-01-01T00:00:00+09:00", cmd=None) -> dict:
        return {
            "name": step.name,
            "cmd": " ".join(step.cmd) if cmd is None else cmd,
            "exit": 0,
            "excerpt": [f"old excerpt of {step.name}"],
            "finished_at": finished_at,
        }

    def test_run_all_records_passed_steps(self):
        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two")
        code, out = self.run_all([one, two], marker=lambda root: self.HEAD)
        self.assertEqual(code, 0)
        state = self.read_state()
        self.assertEqual(state["head"], self.HEAD)
        self.assertEqual([e["name"] for e in state["steps"]], ["one", "two"])
        for e, step in zip(state["steps"], [one, two]):
            self.assertEqual(e["cmd"], " ".join(step.cmd))
            self.assertEqual(e["exit"], 0)
            self.assertEqual(e["excerpt"], [f"out of {step.name}"])
        first = state["steps"][0]["finished_at"]
        self.assertIn(f"記録: {first}、HEAD abc1234", out.splitlines())

    def test_record_line_is_flushed(self):
        class Recorder:
            def __init__(self):
                self.ops: list[tuple[str, str]] = []
                self.pending = 0

            def write(self, s):
                self.ops.append(("w", s))
                self.pending += 1
                return len(s)

            def flush(self):
                self.ops.append(("f", ""))
                self.pending = 0

            def getvalue(self):
                return "".join(s for op, s in self.ops if op == "w")

        rec = Recorder()
        real_run_step = verify.run_step
        unflushed: list[int] = []

        def checking_run_step(*args, **kwargs):
            unflushed.append(rec.pending)
            return real_run_step(*args, **kwargs)

        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two")
        with mock.patch.object(verify, "run_step", checking_run_step):
            code, _ = self.run_all([one, two], out=rec, marker=lambda root: self.HEAD)
        self.assertEqual(code, 0)
        self.assertEqual(unflushed, [0, 0])  # 子を起動する時点で見出しと `$ ` の行が出ている
        idx = [i for i, (op, s) in enumerate(rec.ops) if op == "w" and "記録:" in s]
        self.assertEqual(len(idx), 2)
        for i in idx:
            rest = rec.ops[i + 1 :]
            first_flush = next(j for j, (op, _) in enumerate(rest) if op == "f")
            self.assertTrue(all(s == "\n" for op, s in rest[:first_flush]), rest[: first_flush + 1])

    def test_continue_skips_recorded_steps_and_prints_excerpt(self):
        one, one_mark = self.touch_step("one")
        two, two_mark = self.touch_step("two", ok=False)
        marker = lambda root: self.HEAD  # noqa: E731
        code, _ = self.run_all([one, two], marker=marker)
        self.assertEqual(code, 1)
        first = self.read_state()
        self.assertEqual([e["name"] for e in first["steps"]], ["one"])
        one_at = first["steps"][0]["finished_at"]
        one_mark.unlink()
        two_mark.unlink()
        two_fixed, _ = self.touch_step("two")  # 同じパスに書き直す (cmd の字面は同じ)
        self.assertEqual(two_fixed.cmd, two.cmd)
        code, out = self.run_all([one, two_fixed], marker=marker, resume=True)
        self.assertEqual(code, 0)
        self.assertFalse(one_mark.exists())
        self.assertTrue(two_mark.exists())
        lines = out.splitlines()
        self.assertIn(f"== one (引き継ぎ: {one_at}、HEAD abc1234) ==", lines)
        self.assertIn("out of one", lines)
        self.assertIn("one: ok (引き継ぎ)", lines)
        self.assertIn("two: ok", lines)
        self.assertNotIn("引き継がない:", out)
        state = self.read_state()
        self.assertEqual([e["name"] for e in state["steps"]], ["one", "two"])
        self.assertEqual(state["steps"][0]["finished_at"], one_at)

    def test_continue_reruns_when_cmd_differs(self):
        one, one_mark = self.touch_step("one")
        self.put_state(self.HEAD, [self.old_entry(one, cmd="python other.py")])
        code, out = self.run_all([one], marker=lambda root: self.HEAD, resume=True)
        self.assertEqual(code, 0)
        self.assertTrue(one_mark.exists())
        self.assertNotIn("(引き継ぎ:", out)
        entries = self.read_state()["steps"]
        self.assertEqual(len(entries), 1)
        self.assertEqual(entries[0]["cmd"], " ".join(one.cmd))

    def test_continue_runs_all_when_head_differs(self):
        one, one_mark = self.touch_step("one")
        two, two_mark = self.touch_step("two")
        self.put_state("old", [self.old_entry(one), self.old_entry(two)])
        code, out = self.run_all([one, two], marker=lambda root: "new", resume=True)
        self.assertEqual(code, 0)
        self.assertIn("引き継がない:", out)
        self.assertTrue(one_mark.exists())
        self.assertTrue(two_mark.exists())
        state = self.read_state()
        self.assertEqual(state["head"], "new")
        self.assertEqual([e["name"] for e in state["steps"]], ["one", "two"])
        self.assertTrue(all(not e["finished_at"].startswith("2000-") for e in state["steps"]))

    def test_continue_runs_all_when_dirty(self):
        one, one_mark = self.touch_step("one")
        two, two_mark = self.touch_step("two")
        self.put_state(self.HEAD, [self.old_entry(one), self.old_entry(two)])
        code, out = self.run_all([one, two], marker=lambda root: None, resume=True)
        self.assertEqual(code, 0)
        self.assertIn("引き継がない:", out)
        self.assertTrue(one_mark.exists())
        self.assertTrue(two_mark.exists())
        self.assertFalse(self.state_path.exists())
        self.assertNotIn("検査の間に", out)

    def test_dirty_run_does_not_reread_marker(self):
        calls: list[Path] = []

        def marker(root):
            calls.append(root)
            return None

        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two")
        code, _ = self.run_all([one, two], marker=marker)
        self.assertEqual(code, 0)
        self.assertEqual(len(calls), 1)

    def test_run_without_continue_resets_state(self):
        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two", ok=False)
        self.put_state(self.HEAD, [self.old_entry(one), self.old_entry(two)])
        code, _ = self.run_all([one, two], marker=lambda root: self.HEAD)
        self.assertEqual(code, 1)
        entries = self.read_state()["steps"]
        self.assertEqual([e["name"] for e in entries], ["one"])
        self.assertFalse(entries[0]["finished_at"].startswith("2000-"))
        # 最初の手順で落ちる (手順ごとの書き直しが無い) ときも、頭で空にしてあるので古い成功が残らない
        self.put_state(self.HEAD, [self.old_entry(one), self.old_entry(two)])
        code, _ = self.run_all([two, one], marker=lambda root: self.HEAD)
        self.assertEqual(code, 1)
        self.assertEqual(self.read_state()["steps"], [])

    def test_marker_change_mid_run_stops_recording(self):
        seen = iter(["a"])
        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two")
        code, out = self.run_all([one, two], marker=lambda root: next(seen, "b"))
        self.assertEqual(code, 0)
        self.assertIn("記録しない: 検査の間に作業ツリーか HEAD が変わった", out)
        self.assertEqual(out.count("記録しない:"), 1)
        self.assertFalse(self.state_path.exists())

    def test_step_write_failure_stops_recording_once(self):
        real_replace = verify.os.replace
        calls = {"n": 0}

        def flaky_replace(src, dst):
            calls["n"] += 1
            if calls["n"] == 1:
                return real_replace(src, dst)
            raise PermissionError("locked")

        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two")
        with mock.patch.object(verify.os, "replace", flaky_replace):
            code, out = self.run_all([one, two], marker=lambda root: self.HEAD)
        self.assertEqual(code, 0)
        self.assertEqual(out.count("記録しない: state.json を書けない ("), 1)
        self.assertEqual(self.read_state()["steps"], [])
        self.assertNotIn("記録: ", out)

    def test_reset_write_failure_keeps_verdict(self):
        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two")
        with mock.patch.object(verify.os, "replace", side_effect=PermissionError("locked")):
            code, out = self.run_all([one, two], marker=lambda root: self.HEAD)
        self.assertEqual(code, 0)
        lines = out.splitlines()
        self.assertIn("one: ok", lines)
        self.assertIn("two: ok", lines)
        self.assertIn("記録しない: state.json を書けない (", out)
        self.assertNotIn("注意: 古い state.json", out)

    def test_reset_failure_warns_about_stale_state(self):
        one, _ = self.touch_step("one")
        two, _ = self.touch_step("two", ok=False)
        self.put_state(self.HEAD, [self.old_entry(one), self.old_entry(two)])
        with (
            mock.patch.object(verify.os, "replace", side_effect=PermissionError("locked")),
            mock.patch.object(Path, "unlink", side_effect=PermissionError("locked")),
        ):
            code, out = self.run_all([one, two], marker=lambda root: self.HEAD)
        self.assertEqual(code, 1)
        self.assertIn("記録しない: state.json を書けない (", out)
        self.assertIn("注意: 古い state.json が残っている", out)


class StateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.dir = Path(self.tmp.name)
        self.path = self.dir / "state.json"

    def tearDown(self):
        self.tmp.cleanup()

    @staticmethod
    def entry(**over) -> dict:
        e = {"name": "fmt", "cmd": "cargo fmt", "exit": 0, "excerpt": ["x"], "finished_at": "t"}
        e.update(over)
        return e

    def test_load_state_accepts_good_file(self):
        data = {"version": 1, "head": "h", "steps": [self.entry()]}
        self.path.write_text(json.dumps(data), encoding="utf-8")
        state, reason = verify.load_state(self.path, "h")
        self.assertEqual(state, data)
        self.assertEqual(reason, "")

    def test_load_state_rejects_bad_files(self):
        no_excerpt = self.entry()
        del no_excerpt["excerpt"]
        good = lambda steps, **top: {"version": 1, "head": "h", "steps": steps, **top}  # noqa: E731
        cases: dict[str, bytes] = {
            "無い": b"",
            "壊れた JSON": b"{not json",
            "UTF-8 でない": b"\xff\xfe\x00{",
            "トップが配列": json.dumps([1]).encode(),
            "version 2": json.dumps(good([], version=2)).encode(),
            "version true": json.dumps(good([], version=True)).encode(),
            "head 違い": json.dumps(good([], head="other")).encode(),
            "steps が dict": json.dumps(good({})).encode(),
            "excerpt が無い": json.dumps(good([no_excerpt])).encode(),
            "名前の重複": json.dumps(good([self.entry(), self.entry()])).encode(),
            "exit が 1": json.dumps(good([self.entry(exit=1)])).encode(),
            "exit が true": json.dumps(good([self.entry(exit=True)])).encode(),
        }
        for label, raw in cases.items():
            with self.subTest(label):
                if label == "無い":
                    self.path.unlink(missing_ok=True)
                else:
                    self.path.write_bytes(raw)
                state, reason = verify.load_state(self.path, "h")
                self.assertIsNone(state)
                self.assertTrue(reason)

    def test_tree_mark_none_outside_git(self):
        # 一時ディレクトリは git の外 (この test だけが git を起動してよい)
        self.assertIsNone(verify.tree_mark(self.dir))

    def test_tree_mark_uses_install_helpers(self):
        with mock.patch.object(verify.install, "tree_changes", return_value=[" M x"]):
            self.assertIsNone(verify.tree_mark(self.dir))
        with (
            mock.patch.object(verify.install, "tree_changes", return_value=[]),
            mock.patch.object(verify.install, "head_commit", return_value="f" * 40),
        ):
            self.assertEqual(verify.tree_mark(self.dir), "f" * 40)
        with mock.patch.object(verify.install, "tree_changes", side_effect=RuntimeError("no git")):
            self.assertIsNone(verify.tree_mark(self.dir))


def forbid_real_frame(case: unittest.TestCase) -> list:
    """main を通すテストの setUp で、既定の spawn (本物の verify_frame.py の起動) を「呼ばれたら失敗する」
    偽物に差し替える。make_frame_session の差し込みを忘れたテストは、窓を作らずに落ちる"""
    calls: list = []

    def refuse(*args, **kwargs):
        calls.append(args)
        raise AssertionError("本物の verify_frame.py を起動しようとした (make_frame_session を差し替える)")

    patcher = mock.patch.object(verify, "spawn_frame", refuse)
    patcher.start()
    case.addCleanup(patcher.stop)
    return calls


class MainTest(unittest.TestCase):
    def setUp(self):
        self.refused = forbid_real_frame(self)

    def test_main_rejects_quick_with_ignored(self):
        err = io.StringIO()
        with self.assertRaises(SystemExit) as cm, contextlib.redirect_stderr(err):
            verify.main(["--quick", "--ignored"])
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("--quick と --ignored", err.getvalue())

    def test_main_passes_jobs_to_run_all(self):
        with mock.patch.object(verify, "run_all", return_value=0) as run_all:
            self.assertEqual(verify.main(["--jobs", "2"]), 0)
        call = run_all.call_args
        # 実装は名前で渡す。位置で渡しても読めるように両方を見る (run_all の 6 番目の引数が jobs)
        jobs = call.kwargs["jobs"] if "jobs" in call.kwargs else call.args[5]
        self.assertEqual(jobs, 2)

    def test_main_passes_continue_to_run_all(self):
        with mock.patch.object(verify, "run_all", return_value=0) as run_all:
            self.assertEqual(verify.main(["--continue", "--quick"]), 0)
        self.assertIs(run_all.call_args.kwargs.get("resume"), True)
        with mock.patch.object(verify, "run_all", return_value=0) as run_all:
            verify.main(["--quick"])
        self.assertIs(run_all.call_args.kwargs.get("resume"), False)

    def test_main_rejects_zero_jobs(self):
        err = io.StringIO()
        with self.assertRaises(SystemExit) as cm, contextlib.redirect_stderr(err):
            verify.main(["--jobs", "0"])
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("--jobs は 1 以上", err.getvalue())


# --- 重い検査の枠 (D-111、plan 2026-11-05-verify-frame のステップ 2) ---
# どのテストも本物の verify_frame.py を起動しない (窓を作らない)。偽の子は一時ディレクトリの小さな Python

NOTICE = f"枠: 画面の縁に枠を出した。{verify.LEAD_MS} ms 後に始める (D-111)"

CHILD_READY_UNTIL_EOF = """\
    import os, sys
    sys.stderr.write("frame child log\\n")
    sys.stdout.write("ready\\n")
    sys.stdout.flush()
    while os.read(0, 4096):
        pass
"""
CHILD_READY_IGNORES_EOF = """\
    import sys, time
    sys.stdout.write("ready\\n")
    sys.stdout.flush()
    time.sleep(30)
"""
CHILD_SILENT_SLEEP = "import time\ntime.sleep(30)\n"
CHILD_SILENT_EXIT2 = "import sys\nsys.exit(2)\n"
CHILD_UNEXPECTED_LINE = """\
    import sys, time
    print("hello")
    sys.stdout.flush()
    time.sleep(30)
"""


def child_error(reason: str) -> str:
    return f"import sys\nprint({('error: ' + reason)!r})\nsys.stdout.flush()\nsys.exit(1)\n"


class FrameCase(unittest.TestCase):
    """偽の子を起動する spawn と、記録するだけの眠りと止めを持つ"""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.dir = Path(self.tmp.name)
        self.root = self.dir / "root"
        self.root.mkdir()
        self.procs: list[subprocess.Popen] = []
        self.spawned: list[dict] = []
        self.slept: list[float] = []
        self.killed: list[int] = []
        self.addCleanup(self.tmp.cleanup)
        self.addCleanup(self.reap)

    def reap(self):
        for p in self.procs:
            if p.poll() is None:
                verify.kill_tree(p.pid)
                p.kill()
            p.wait(timeout=10)
            for s in (p.stdin, p.stdout):
                if s is not None and not s.closed:
                    s.close()

    def fake_spawn(self, body: str):
        script = self.dir / f"child{len(list(self.dir.glob('child*.py')))}.py"
        script.write_text(textwrap.dedent(body), encoding="utf-8")

        def spawn(cmd, cwd, stderr, env):
            self.spawned.append({"cmd": cmd, "cwd": cwd, "env": env})
            p = subprocess.Popen(
                [sys.executable, str(script)],
                cwd=cwd,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=stderr,
                env=env,
            )
            self.procs.append(p)  # テストの側で持ち続ける (GC で標準入力が閉じる経路に頼らない)
            return p

        return spawn

    def kill(self, pid: int) -> None:
        self.killed.append(pid)
        verify.kill_tree(pid)

    def session(self, body: str, root=None, **kw) -> verify.FrameSession:
        return verify.FrameSession(
            self.root if root is None else root,
            ["unused"],
            spawn=self.fake_spawn(body),
            sleep=self.slept.append,
            killer=self.kill,
            **kw,
        )


class FrameSessionTest(FrameCase):
    def test_child_that_ignores_eof_is_killed(self):
        # (6) 標準入力を閉じても終わらない子は killer で止められる
        s = self.session(CHILD_READY_IGNORES_EOF, stop_wait=0.5)
        out = io.StringIO()
        self.assertTrue(s.start(out))
        proc = self.procs[0]
        s.stop()
        self.assertTrue(proc.stdin.closed)
        self.assertEqual(self.killed, [proc.pid])
        self.assertIsNotNone(proc.poll())
        s.stop()  # 2 回目は何もしない
        self.assertEqual(self.killed, [proc.pid])

    def test_silent_child_times_out_and_is_stopped(self):
        # (4) 何も出さずに眠り続ける子は上限で「出せない」になり、止められる
        s = self.session(CHILD_SILENT_SLEEP, ready_timeout=0.5, stop_wait=0.5)
        out = io.StringIO()
        self.assertFalse(s.start(out))
        self.assertIn("枠: 出せない (0.5 秒で ready が来ない)。枠なしで続ける", out.getvalue().splitlines())
        self.assertEqual(self.slept, [])
        proc = self.procs[0]
        self.assertEqual(self.killed, [proc.pid])
        self.assertIsNotNone(proc.poll())

    def test_child_exiting_silently_reports_exit_code_without_waiting(self):
        # (7) 何も出さずに exit 2 で終わる子は、上限を待たずに終了コードつきの「出せない」になる
        s = self.session(CHILD_SILENT_EXIT2, ready_timeout=10.0)
        out = io.StringIO()
        started = time.monotonic()
        self.assertFalse(s.start(out))
        self.assertLess(time.monotonic() - started, 5)
        self.assertIn(
            "枠: 出せない (子が何も出さずに終わった (exit=2))。枠なしで続ける", out.getvalue().splitlines()
        )
        self.assertEqual(self.slept, [])
        self.assertEqual(self.killed, [])

    def test_unexpected_line_is_reported_and_child_stopped(self):
        # ready でも error: でもない行は「出せない」になり、眠らず、子は止められる
        s = self.session(CHILD_UNEXPECTED_LINE, stop_wait=0.5)
        out = io.StringIO()
        self.assertFalse(s.start(out))
        self.assertIn(
            "枠: 出せない (子の行が ready でも error: でもない (hello))。枠なしで続ける",
            out.getvalue().splitlines(),
        )
        self.assertEqual(self.slept, [])
        proc = self.procs[0]
        self.assertTrue(proc.stdin.closed)
        self.assertEqual(self.killed, [proc.pid])
        self.assertIsNotNone(proc.poll())

    def test_start_creates_log_dir_in_bare_root(self):
        # (8) target の無い root でも子のログを開けて知らせの行を出す
        bare = self.dir / "bare"
        bare.mkdir()
        s = self.session(CHILD_READY_UNTIL_EOF, root=bare)
        out = io.StringIO()
        self.assertTrue(s.start(out))
        self.assertEqual(out.getvalue().splitlines(), [NOTICE])
        s.stop()
        self.assertEqual(self.procs[0].returncode, 0)
        log = (bare / "target" / "verify" / "frame.log").read_text(encoding="utf-8")
        self.assertIn("frame child log", log)
        self.assertEqual(self.spawned[0]["cwd"], bare)

    def test_spawn_oserror_is_reported(self):
        def spawn(cmd, cwd, stderr, env):
            raise FileNotFoundError("no python")

        s = verify.FrameSession(self.root, ["x"], spawn=spawn, sleep=self.slept.append, killer=self.kill)
        out = io.StringIO()
        self.assertFalse(s.start(out))
        self.assertIn("枠: 出せない (no python)。枠なしで続ける", out.getvalue())
        self.assertEqual(self.slept, [])

    def test_make_frame_session_command(self):
        s = verify.make_frame_session(self.root, "py", 1800, 6)
        self.assertEqual(
            s.cmd, ["py", str(HERE / "verify_frame.py"), "--max-seconds", str(1800 * 6 + 120)]
        )
        self.assertEqual(s.root, self.root)


class FrameMainTest(FrameCase):
    def setUp(self):
        super().setUp()
        self.refused = forbid_real_frame(self)
        self.made: list[tuple] = []
        self.run_all_calls: list[dict] = []

    def run_main(self, argv, body, run_all_result=0, heading="== fmt ==", **session_kw):
        """make_frame_session を偽の子の FrameSession に、run_all を見出しを出すだけのものに替えて main を打つ"""

        def make(root, python, timeout, n_steps):
            self.made.append((root, python, timeout, n_steps))
            return self.session(body, **session_kw)

        def fake_run_all(root, steps, timeout, out=sys.stdout, **kw):
            alive = [p.poll() is None for p in self.procs]
            self.run_all_calls.append({"kw": kw, "alive": alive})
            print(heading, file=out)
            if isinstance(run_all_result, Exception):
                raise run_all_result
            return run_all_result

        buf = io.StringIO()
        with (
            mock.patch.object(verify, "make_frame_session", make),
            mock.patch.object(verify, "run_all", fake_run_all),
            contextlib.redirect_stdout(buf),
        ):
            code = verify.main(argv + ["--root", str(self.root)])
        return code, buf.getvalue().splitlines()

    def test_ignored_shows_frame_before_first_step(self):
        # (1) 知らせの行が最初の見出しより前、眠りは 3.0 で 1 回、run_all の後に子が終わっている
        code, lines = self.run_main(["--ignored"], CHILD_READY_UNTIL_EOF)
        self.assertEqual(code, 0)
        self.assertLess(lines.index(NOTICE), lines.index("== fmt =="))
        self.assertEqual(self.slept, [3.0])
        self.assertEqual(self.run_all_calls[0]["alive"], [True])
        proc = self.procs[0]
        self.assertTrue(proc.stdin.closed)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(self.killed, [])
        self.assertEqual(self.made, [(self.root, sys.executable, 1800.0, 6)])
        self.assertEqual(self.refused, [])

    def test_without_ignored_starts_no_frame(self):
        # (2) --ignored なしでは子を起動しない
        for argv in ([], ["--quick"]):
            with self.subTest(argv=argv):
                code, lines = self.run_main(argv, CHILD_READY_UNTIL_EOF)
                self.assertEqual(code, 0)
                self.assertNotIn(NOTICE, lines)
        self.assertEqual(self.made, [])
        self.assertEqual(self.spawned, [])
        self.assertEqual(self.refused, [])

    def test_error_line_continues_without_frame(self):
        # (3) error: の行は「出せない (<理由>)」になり、眠らずに検査を続ける。日本語の理由も化けない
        # 親の環境の PYTHONIOENCODING を外し、子の環境の値が FrameSession が足したものだけになるようにする
        for reason in ("x", "窓を作れない"):
            with self.subTest(reason=reason), mock.patch.dict(verify.os.environ):
                verify.os.environ.pop("PYTHONIOENCODING", None)
                self.slept.clear()
                self.run_all_calls.clear()
                code, lines = self.run_main(["--ignored"], child_error(reason))
                self.assertEqual(code, 0)
                line = f"枠: 出せない ({reason})。枠なしで続ける"
                self.assertIn(line, lines)
                self.assertLess(lines.index(line), lines.index("== fmt =="))
                self.assertNotIn(NOTICE, lines)
                self.assertEqual(self.slept, [])
                self.assertEqual(len(self.run_all_calls), 1)
                self.assertEqual(self.spawned[-1]["env"].get("PYTHONIOENCODING"), "utf-8")

    def test_frame_is_stopped_when_run_all_fails_or_raises(self):
        # (5) run_all が失敗 (exit 1) でも例外でも子が止められる
        code, _ = self.run_main(["--ignored"], CHILD_READY_UNTIL_EOF, run_all_result=1)
        self.assertEqual(code, 1)
        with self.assertRaises(RuntimeError):
            self.run_main(["--ignored"], CHILD_READY_UNTIL_EOF, run_all_result=RuntimeError("boom"))
        self.assertEqual(len(self.procs), 2)
        for proc in self.procs:
            self.assertTrue(proc.stdin.closed)  # stop が標準入力を閉じた記録
            self.assertEqual(proc.wait(timeout=5), 0)

    def test_continue_also_shows_frame_first(self):
        # (9) --ignored --continue でも子を起動し、知らせの行が引き継ぎの見出しより前に出る
        heading = "== fmt (引き継ぎ: 2026-11-06T00:00:00+09:00、HEAD abc1234) =="
        code, lines = self.run_main(["--ignored", "--continue"], CHILD_READY_UNTIL_EOF, heading=heading)
        self.assertEqual(code, 0)
        self.assertLess(lines.index(NOTICE), lines.index(heading))
        self.assertIs(self.run_all_calls[0]["kw"].get("resume"), True)
        self.assertEqual(len(self.spawned), 1)

    def test_default_spawn_is_refused_without_session_patch(self):
        # 対照: setUp の spawn_frame の差し替えだけが効いた状態で main(["--ignored"]) を打つと、偽物が呼ばれて落ちる
        with (
            mock.patch.object(verify, "run_all", return_value=0) as run_all,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            with self.assertRaises(AssertionError):
                verify.main(["--ignored", "--root", str(self.root)])
        self.assertEqual(len(self.refused), 1)
        run_all.assert_not_called()


if __name__ == "__main__":
    unittest.main()
