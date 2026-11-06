"""/verify の手順 (.claude/skills/verify/SKILL.md の 0〜6) を順に走らせ、判定までを機械で行う (D-026、D-076)。

手順 (名前とログは SKILL.md と同じ。ログは target/verify/<名前>.log):
  0 target/verify を作る
  1 fmt      cargo fmt --all -- --check                          (末尾 15 行を出す)
  2 clippy   cargo clippy --all-targets -- -D warnings          (末尾 15 行)
  3 test     cargo test                                         (test result|FAILED|panicked の全行)
  4 hook     python .claude/hooks/test_guard_real_input.py      (末尾 5 行)
  5 release  cargo build --release                              (末尾 5 行)
  6 ignored  cargo test -- --ignored --nocapture --test-threads=1
             (test result|FAILED|panicked|NOT_FOREGROUND|NO_INPUT_DESKTOP の全行)
既定は 0〜5。--quick は 0〜3 (速い検査)、--ignored は 6 を足す (実入力に触れる。主エージェントだけが、
直列で走らせる)。--quick と --ignored は同時に渡せない。配置 (SKILL.md の手順 8、scripts/install.py) は入れない。
SKILL.md が正で、この script は写し (`--jobs` と `--continue` と --ignored の枠は SKILL.md にまだ無い (D-083、
D-111。利用者が SKILL.md を変えてよいと言うまでの間))。どちらかを変えたら python scripts/test_verify.py を走らせる。

並列数 (D-083): --jobs N (1 以上) を渡すと、cargo の手順 (fmt、clippy、test、release、ignored) の子の環境に
CARGO_BUILD_JOBS=N を足す (hook には渡さない。コマンドの字面は変えない)。渡さなければ環境は今のまま
(呼び出し元の CARGO_BUILD_JOBS があればそれが効く)。run_all の出力の頭 (--ignored では枠の知らせの行の後) に、
効いている並列数の出どころを 1 行出す (jobs=<N> (--jobs) / jobs=<値> (環境の CARGO_BUILD_JOBS) /
jobs=cargo の既定)。既定の並列数は変えない。空から作り直すときや、他のプロセスでメモリが詰まっているときの逃げ道。

重い検査の枠 (D-111): --ignored (--continue つきも) のときだけ、最初の手順の前に target/verify を作り、
枠の子 (この script と同じ場所の verify_frame.py。`--max-seconds` は手順の数 × 手順の制限 + 120 秒) を起動して
`ready` の 1 行を 5 秒まで待つ。来たら `枠: 画面の縁に枠を出した。3000 ms 後に始める (D-111)` を出して 3 秒
待ってから手順に入る。`error:` の行、何も出さずに子が終わった、時間切れ、起動の失敗なら
`枠: 出せない (<理由>)。枠なしで続ける` を出し、待たずに手順に入る (子が動いていれば止める)。最後の手順の後は
成否にかかわらず (例外でも) 子の標準入力を閉じて 2 秒待ち、残れば taskkill /T /F で止める。子の標準エラー
(ログ) は target/verify/frame.log。枠は知らせで、守りではない (人が触れても検査は止めない)。

通った手順の記録と引き継ぎ (D-083): 作業ツリーが clean (scripts/install.py の tree_changes と head_commit で見る。
配置と同じ定義) なら、手順が通るたびに target/verify/state.json に {"version": 1, "head": <HEAD の sha>,
"steps": [{"name", "cmd", "exit", "excerpt", "finished_at"}]} として書き (名前で 1 つ)、判定の行の後に
`記録: <finished_at>、HEAD <sha の先頭 7 文字>` を出す。走らせる前と通った直後で木の印が違えば記録を消して
以降は書かない。dirty か git が読めないときは記録を消して書かない。--continue を付けると、記録の head が今の
HEAD と同じ clean な木で、記録の cmd が今の手順と同じ手順を走らせずに「引き継ぎ」として出す (見出しに
`(引き継ぎ: <finished_at>、HEAD <sha7>)`、記録の exit と抜き出し、`判定: ok (引き継ぎ)`)。引き継げないときは
`引き継がない: <理由>` を出して全部を走らせる。--continue なしの呼び出しは、最初の手順の前に記録を空にする。
記録の読み書きの失敗は `記録しない: ...` を出すだけで、検査と判定は変えない。--continue は外から止められた
実行の打ち直しにだけ使い、exit 1 のあとは直して --continue なしで最初からやり直す (SKILL.md の手順 7)。

判定 (D-026。件数 0 の成功と空のログを成功と取り違えない):
  - どの手順も、時間切れか終了コードが 0 でなければ失敗
  - test と ignored は、test result の行が無い、ok でない行がある、FAILED か panicked の行がある、
    passed の合計が 0 (ignored では「全部飛ばした」) のどれかで失敗
  - hook は unittest の最後の行が OK で始まらなければ失敗
最初に失敗した手順で止め、残りは「未実行」と出す。cargo は 1 つずつ順に起動し、標準出力と標準エラーを
1 つのログにまとめる。手順ごとの制限時間 (既定 1800 秒) が切れたら、taskkill /T /F でプロセスの木ごと止める
(子だけを止めると孫の rustc やテストの実行ファイルが残る)。強制で止めた実入力のテストは後始末が走らない。

出力 (標準出力、UTF-8) は手順ごとに、コマンド、exit=<終了コード か timeout>、ログの該当行、判定。
最後に手順ごとのまとめ。そのまま会話に貼れる形 (SKILL.md の「報告」)。

終了コード: 0 全部通った / 1 どれかが失敗か時間切れ / 2 引数の誤り
Claude Code の Bash ツールからは、--quick も含めて run_in_background で起動し、終わりの通知を待つ
(制限は手順ごとなので、前景の上限 600 秒を超えうる)。

使い方: python scripts/verify.py [--quick | --ignored] [--jobs N] [--continue] [--timeout 秒] [--root DIR]
"""

import argparse
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

# scripts/__pycache__ を作らない (import install が書く)。scripts/ は build.rs の引き金なので、
# 作ると次の cargo が ai-cu を作り直す (test_verify.py と同じ趣旨)
sys.dont_write_bytecode = True
import install  # noqa: E402  (scripts/ は実行時に sys.path の先頭にある)

STATE_VERSION = 1

RESULT_RE = re.compile(r"^test result: (\w+)\. (\d+) passed; (\d+) failed", re.M)
FAILED_RE = re.compile(r"^.*(FAILED|panicked).*$", re.M)

IGNORED_TIMEOUT_NOTE = (
    "注意: 実入力のテストを強制で止めたので後始末 (押したキーとボタンを離す) が走っていない。"
    "押されたままのキーとボタンを確かめる"
)

# 重い検査の枠 (D-111)。待ちの長さは LEAD_MS の 1 つから、待ちと知らせの行の両方を作る
LEAD_MS = 3000
FRAME_READY_TIMEOUT = 5.0  # 子の `ready` を待つ上限 (秒)
FRAME_STOP_WAIT = 2.0  # 子の標準入力を閉じてから終わりを待つ長さ (秒)。過ぎたら木ごと止める
FRAME_MARGIN_SECONDS = 120  # 子の --max-seconds に足す余裕 (秒)
FRAME_LOG = "frame.log"


@dataclass(frozen=True)
class Step:
    """1 つの手順。抜き出しは `pattern` があればそれに当たる全行、無ければ末尾 `tail` 行。
    `kind` は判定の種類で "plain"、"test"、"ignored"、"unittest" のどれか。
    `cargo` は cargo を起動する手順か (--jobs の CARGO_BUILD_JOBS を渡す相手。D-083)"""

    name: str
    cmd: list[str]
    log: str
    kind: str
    tail: int | None = 5
    pattern: str | None = None
    cargo: bool = False


def steps(python: str, quick: bool, ignored: bool) -> list[Step]:
    """SKILL.md の手順 1〜6 (0 の target/verify の作成は run_all が行う)"""
    out = [
        Step(
            "fmt",
            ["cargo", "fmt", "--all", "--", "--check"],
            "fmt.log",
            "plain",
            tail=15,
            cargo=True,
        ),
        Step(
            "clippy",
            ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"],
            "clippy.log",
            "plain",
            tail=15,
            cargo=True,
        ),
        Step(
            "test",
            ["cargo", "test"],
            "test.log",
            "test",
            tail=None,
            pattern="test result|FAILED|panicked",
            cargo=True,
        ),
    ]
    if quick:
        return out
    out += [
        Step("hook", [python, ".claude/hooks/test_guard_real_input.py"], "hook.log", "unittest"),
        Step("release", ["cargo", "build", "--release"], "release.log", "plain", cargo=True),
    ]
    if ignored:
        out.append(
            Step(
                "ignored",
                ["cargo", "test", "--", "--ignored", "--nocapture", "--test-threads=1"],
                "ignored.log",
                "ignored",
                tail=None,
                pattern="test result|FAILED|panicked|NOT_FOREGROUND|NO_INPUT_DESKTOP",
                cargo=True,
            )
        )
    return out


def judge(kind: str, exit_code: int | None, log: str) -> tuple[bool, str]:
    """終了コードとログから (通ったか, 理由) を返す。理由の順は
    終了コード → test result の行の有無 → ok → FAILED/panicked → 0 件"""
    if exit_code is None:
        return False, "時間切れ"
    if exit_code != 0:
        return False, f"終了コード {exit_code}"
    if kind in ("test", "ignored"):
        rows = RESULT_RE.findall(log)
        if not rows:
            return False, "test result の行が無い"
        if any(status != "ok" for status, _, _ in rows):
            return False, "test result が ok でない"
        if FAILED_RE.search(log):
            return False, "FAILED か panicked の行がある"
        if sum(int(passed) for _, passed, _ in rows) == 0:
            return False, "走った件数が 0"
    elif kind == "unittest":
        lines = [line for line in log.splitlines() if line.strip()]
        if not lines or not lines[-1].strip().startswith("OK"):
            return False, "unittest の OK が無い"
    return True, "ok"


def excerpt(step: Step, log: str) -> list[str]:
    """ログの該当行。pattern があれば当たる全行、無ければ末尾 tail 行"""
    lines = log.splitlines()
    if step.pattern is not None:
        pat = re.compile(step.pattern)
        return [line for line in lines if pat.search(line)]
    if step.tail is None:
        return lines
    return lines[-step.tail :] if step.tail > 0 else []


def kill_tree(pid: int) -> None:
    """pid を根にしたプロセスの木を止める。taskkill が無い、失敗した、のどれでも戻る"""
    try:
        subprocess.run(
            ["taskkill", "/T", "/F", "/PID", str(pid)], capture_output=True, timeout=30
        )
    except (OSError, subprocess.SubprocessError):
        pass


def kill_and_reap(proc: subprocess.Popen, killer: Callable[[int], None]) -> bool:
    """killer で木ごと止め、proc.kill() を最後の守りにして 10 秒待つ。それでも止まらなければ True。
    run_step と FrameSession.stop が同じ段を踏むための 1 か所"""
    killer(proc.pid)
    try:
        proc.kill()  # 最後の守り。既に終わっていれば何もしない
    except OSError:
        pass
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        return True
    return False


def run_step(
    root: Path,
    step: Step,
    timeout: float,
    killer: Callable[[int], None],
    jobs: int | None = None,
) -> tuple[int | None, str, bool]:
    """1 手順を走らせ、(終了コード か 時間切れの None, ログの本文, 止まらなかったか) を返す。
    jobs が None でなく cargo の手順なら、子の環境に CARGO_BUILD_JOBS=jobs を足す (D-083)"""
    log_path = root / "target" / "verify" / step.log
    env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
    if jobs is not None and step.cargo:
        env["CARGO_BUILD_JOBS"] = str(jobs)
    stuck = False
    with open(log_path, "wb") as f:
        proc = subprocess.Popen(step.cmd, cwd=root, stdout=f, stderr=subprocess.STDOUT, env=env)
        try:
            code: int | None = proc.wait(timeout)
        except subprocess.TimeoutExpired:
            stuck = kill_and_reap(proc, killer)
            code = None
    text = log_path.read_bytes().decode("utf-8", errors="replace")
    return code, text, stuck


def tree_mark(root: Path) -> str | None:
    """木の印: 作業ツリーが clean で HEAD が読めればその sha、dirty か git が読めなければ None。
    clean の定義は配置 (scripts/install.py) と同じ関数で見る (D-083)"""
    try:
        if install.tree_changes(root):
            return None
        return install.head_commit(root)
    except RuntimeError:
        return None


def _is_str_list(value) -> bool:
    return isinstance(value, list) and all(isinstance(x, str) for x in value)


def _step_record_problem(entry) -> str | None:
    """記録の steps の 1 要素の形の誤り (無ければ None)"""
    if not isinstance(entry, dict):
        return "steps の要素が dict でない"
    for key in ("name", "cmd", "finished_at"):
        if not isinstance(entry.get(key), str):
            return f"steps の要素の {key} が文字列でない"
    code = entry.get("exit")
    if type(code) is not int or code != 0:  # bool (True / False) も断る
        return "steps の要素の exit が 0 でない"
    if not _is_str_list(entry.get("excerpt")):
        return "steps の要素の excerpt が文字列の配列でない"
    return None


def load_state(path: Path, head: str) -> tuple[dict | None, str]:
    """記録を読み、引き継げれば (記録, "")、引き継げなければ (None, 理由) を返す"""
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return None, "state.json が無い"
    except OSError as e:
        return None, f"state.json を読めない ({e})"
    try:
        data = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as e:
        return None, f"state.json を読めない ({e})"
    except json.JSONDecodeError as e:
        return None, f"state.json が JSON でない ({e})"
    if not isinstance(data, dict):
        return None, "state.json の形が違う (トップが dict でない)"
    version = data.get("version")
    if type(version) is not int or version != STATE_VERSION:
        return None, f"state.json の版が {STATE_VERSION} でない ({version!r})"
    recorded = data.get("head")
    if not isinstance(recorded, str):
        return None, "state.json の形が違う (head が文字列でない)"
    if recorded != head:
        return None, f"記録の HEAD {recorded[:7]} が今の HEAD {head[:7]} と違う"
    entries = data.get("steps")
    if not isinstance(entries, list):
        return None, "state.json の形が違う (steps が配列でない)"
    names: set[str] = set()
    for entry in entries:
        problem = _step_record_problem(entry)
        if problem is not None:
            return None, f"state.json の形が違う ({problem})"
        if entry["name"] in names:
            return None, f"state.json の形が違う (名前 {entry['name']} が重複している)"
        names.add(entry["name"])
    return {"version": STATE_VERSION, "head": recorded, "steps": entries}, ""


def save_state(path: Path, state: dict) -> str | None:
    """記録を一時ファイルに書いて置き換える。成功なら None、失敗なら理由"""
    tmp = path.with_name(path.name + ".tmp")
    try:
        tmp.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(tmp, path)
    except OSError as e:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        return str(e)
    return None


def drop_state(path: Path) -> str | None:
    """記録を消す (無ければ何もしない)。成功なら None、失敗なら理由"""
    try:
        path.unlink(missing_ok=True)
    except OSError as e:
        return str(e)
    return None


def jobs_line(jobs: int | None, environ) -> str:
    """出力の頭の 1 行: 効いている並列数の出どころ (cargo の既定の数は分からないので書かない)"""
    if jobs is not None:
        return f"jobs={jobs} (--jobs)"
    env_jobs = environ.get("CARGO_BUILD_JOBS")
    if env_jobs is not None:
        return f"jobs={env_jobs} (環境の CARGO_BUILD_JOBS)"
    return "jobs=cargo の既定"


def spawn_frame(cmd: list[str], cwd: Path, stderr, env: dict) -> subprocess.Popen:
    """枠の子を起動する (既定の起動)。標準入力と標準出力はパイプ (bytes)、標準エラーは渡されたファイル"""
    return subprocess.Popen(
        cmd, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, env=env
    )


class FrameSession:
    """重い検査の枠の子 (scripts/verify_frame.py) の起動・待ち・止め (D-111)。

    spawn (起動)、sleep (眠り)、killer (木ごと止める)、ready_timeout と stop_wait (待つ上限) は差し替えられる
    (test_verify.py は偽の子と記録するだけの眠りで撃ち、実の窓を作らない)。子の環境はここで組み
    (`os.environ` の写しに PYTHONIOENCODING=utf-8)、spawn に渡す"""

    def __init__(
        self,
        root: Path,
        cmd: list[str],
        spawn: Callable[..., subprocess.Popen],
        sleep: Callable[[float], None],
        killer: Callable[[int], None],
        ready_timeout: float = FRAME_READY_TIMEOUT,
        stop_wait: float = FRAME_STOP_WAIT,
    ):
        self.root = root
        self.cmd = cmd
        self._spawn = spawn
        self._sleep = sleep
        self._killer = killer
        self._ready_timeout = ready_timeout
        self._stop_wait = stop_wait
        self.proc: subprocess.Popen | None = None
        self._log = None
        self._reader: threading.Thread | None = None

    def start(self, out) -> bool:
        """子を起動して `ready` を待つ。出たら知らせの行を出して LEAD_MS 眠り True。
        出せなければ「出せない」の行を出し、子を止めて False (眠らない)"""
        reason = self._launch()
        if reason is None:
            print(f"枠: 画面の縁に枠を出した。{LEAD_MS} ms 後に始める (D-111)", file=out, flush=True)
            self._sleep(LEAD_MS / 1000)
            return True
        print(f"枠: 出せない ({reason})。枠なしで続ける", file=out, flush=True)
        self.stop()
        return False

    def _launch(self) -> str | None:
        """起動して `ready` を待つ。出れば None、出せなければ理由"""
        env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
        try:
            log_dir = self.root / "target" / "verify"
            log_dir.mkdir(parents=True, exist_ok=True)
            self._log = open(log_dir / FRAME_LOG, "wb")
            self.proc = self._spawn(self.cmd, cwd=self.root, stderr=self._log, env=env)
        except OSError as e:
            return str(e)
        return self._wait_ready()

    def _wait_ready(self) -> str | None:
        proc = self.proc
        assert proc is not None and proc.stdout is not None
        lines: queue.Queue[bytes] = queue.Queue()

        def read_line() -> None:
            try:
                lines.put(proc.stdout.readline())
            except (OSError, ValueError):
                lines.put(b"")

        self._reader = threading.Thread(target=read_line, daemon=True)
        self._reader.start()
        try:
            raw = lines.get(timeout=self._ready_timeout)
        except queue.Empty:
            return f"{self._ready_timeout:g} 秒で ready が来ない"
        # 子の error: の文は日本語。親の既定 (cp932) で読まない (run_step と同じ)
        line = raw.decode("utf-8", errors="replace").strip()
        if line == "ready":
            return None
        if line.startswith("error: "):
            return line[len("error: ") :]
        if not line:
            try:
                code: int | str = proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                code = "不明"
            return f"子が何も出さずに終わった (exit={code})"
        return f"子の行が ready でも error: でもない ({line})"

    def stop(self) -> None:
        """子の標準入力を閉じて stop_wait 秒待ち、残れば killer で木ごと止める。何度呼んでもよい"""
        proc, self.proc = self.proc, None
        if proc is not None:
            try:
                if proc.stdin is not None:
                    proc.stdin.close()
            except OSError:
                pass
            try:
                proc.wait(timeout=self._stop_wait)
            except subprocess.TimeoutExpired:
                kill_and_reap(proc, self._killer)  # 止まらなくても枠は検査の結果に効かせない
            reader = self._reader
            if reader is not None:
                reader.join(timeout=1)
            # 読みのスレッドが残っていれば (子の木が残ってパイプが開いている) 閉じずに置く (close が待つため)
            if proc.stdout is not None and (reader is None or not reader.is_alive()):
                proc.stdout.close()
        if self._log is not None:
            self._log.close()
            self._log = None


def frame_command(python: str, timeout: float, n_steps: int) -> list[str]:
    """枠の子のコマンド。子のパスはこの script の場所から組む (--root を別の木に向けても起動できる)"""
    script = Path(__file__).resolve().parent / "verify_frame.py"
    max_seconds = int(timeout * n_steps + FRAME_MARGIN_SECONDS)
    return [python, str(script), "--max-seconds", str(max_seconds)]


def make_frame_session(root: Path, python: str, timeout: float, n_steps: int) -> FrameSession:
    """main が使う枠の組 (既定の起動・眠り・止め)。差し替えの継ぎ目はこの 1 つ。
    spawn_frame は呼ばれるたびにモジュールの名前から引く (テストの mock.patch.object が効くように)"""
    return FrameSession(
        root,
        frame_command(python, timeout, n_steps),
        spawn=spawn_frame,
        sleep=time.sleep,
        killer=kill_tree,
    )


def run_all(
    root: Path,
    steps: list[Step],
    timeout: float,
    out=sys.stdout,
    killer: Callable[[int], None] = kill_tree,
    jobs: int | None = None,
    marker: Callable[[Path], str | None] = tree_mark,
    resume: bool = False,
) -> int:
    """手順を順に走らせ、最初の失敗で止める。全部通れば 0、そうでなければ 1。
    jobs は cargo の手順に渡す並列数 (None なら環境のまま)。marker は木の印 (clean なら HEAD の sha)、
    resume は --continue (同じ印の記録で通った手順を引き継ぐ)。どちらも D-083"""
    (root / "target" / "verify").mkdir(parents=True, exist_ok=True)
    state_path = root / "target" / "verify" / "state.json"
    print(jobs_line(jobs, os.environ), file=out)

    mark = marker(root)
    state: dict | None = None
    if resume:
        if mark is None:
            reason = "作業ツリーが clean でないか git が読めない"
        else:
            state, reason = load_state(state_path, mark)
        if state is None:
            print(f"引き継がない: {reason}", file=out)
    # 引き継げた記録 (名前 → 要素)。この呼び出しで書き足した要素とは分けて持つ
    carried = {e["name"]: e for e in state["steps"]} if state is not None else {}
    recording = mark is not None
    if state is None:
        # 引き継がない呼び出し: 古い木の記録や同じ木の古い成功を残さない
        err = None
        if mark is not None:
            state = {"version": STATE_VERSION, "head": mark, "steps": []}
            err = save_state(state_path, state)
        drop_err = drop_state(state_path) if mark is None or err is not None else None
        if err is not None or drop_err is not None:
            print(f"記録しない: state.json を書けない ({err or drop_err})", file=out)
            recording = False
            if drop_err is not None:
                print(
                    "注意: 古い state.json が残っている。次の --continue が古い記録を引き継ぎうる",
                    file=out,
                )

    results: list[tuple[str, str]] = []
    failed = False
    for i, step in enumerate(steps):
        cmd = " ".join(step.cmd)
        rec = carried.get(step.name)
        if rec is not None and rec["cmd"] == cmd:
            print(f"== {step.name} (引き継ぎ: {rec['finished_at']}、HEAD {mark[:7]}) ==", file=out)
            print("$ " + rec["cmd"], file=out)
            print(f"exit={rec['exit']}", file=out)
            for line in rec["excerpt"]:
                print(line, file=out)
            print("判定: ok (引き継ぎ)", file=out, flush=True)
            results.append((step.name, "ok (引き継ぎ)"))
            continue
        print(f"== {step.name} ==", file=out)
        print("$ " + cmd, file=out, flush=True)  # 止められた手順がどれかをログに残す
        code, text, stuck = run_step(root, step, timeout, killer, jobs=jobs)
        finished_at = datetime.now().astimezone().isoformat(timespec="seconds")
        print(f"exit={'timeout' if code is None else code}", file=out)
        lines = excerpt(step, text)
        for line in lines:
            print(line, file=out)
        ok, reason = judge(step.kind, code, text)
        if stuck:
            reason += "、プロセスが止まらない"
        if code is None and step.kind == "ignored":
            print(IGNORED_TIMEOUT_NOTE, file=out)
        print("判定: ok" if ok else f"判定: NG ({reason})", file=out, flush=True)
        if ok and recording and state is not None:
            if marker(root) != mark:
                print("記録しない: 検査の間に作業ツリーか HEAD が変わった", file=out)
                drop_state(state_path)
                recording = False
            else:
                entry = {
                    "name": step.name,
                    "cmd": cmd,
                    "exit": code,
                    "excerpt": lines,
                    "finished_at": finished_at,
                }
                state["steps"] = [e for e in state["steps"] if e["name"] != step.name] + [entry]
                err = save_state(state_path, state)
                if err is not None:
                    print(f"記録しない: state.json を書けない ({err})", file=out)
                    recording = False
                else:
                    # 標準出力がファイルだとブロック単位でバッファされる。外から止められても残るように
                    print(f"記録: {finished_at}、HEAD {mark[:7]}", file=out, flush=True)
        results.append((step.name, "ok" if ok else f"NG ({reason})"))
        if not ok:
            failed = True
            rest = [s.name for s in steps[i + 1 :]]
            if rest:
                print("未実行: " + ", ".join(rest), file=out)
            results += [(name, "未実行") for name in rest]
            break
    print("== まとめ ==", file=out)
    for name, verdict in results:
        print(f"{name}: {verdict}", file=out)
    out.flush()
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):  # 引数の誤りの文 (標準エラー) も化けさせない
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(
        description="/verify の手順を順に走らせて判定する (D-076)。終了コード 0 / 1 / 2"
    )
    parser.add_argument("--quick", action="store_true", help="fmt、clippy、test だけ")
    parser.add_argument(
        "--ignored", action="store_true", help="実入力のテストを足す (主エージェントだけ)"
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=None,
        help="cargo の手順に CARGO_BUILD_JOBS として渡す並列数 (1 以上。既定は環境のまま)",
    )
    parser.add_argument(
        "--continue",
        dest="resume",
        action="store_true",
        help="同じ HEAD の clean な木で通った手順を target/verify/state.json から引き継ぐ (D-083)",
    )
    parser.add_argument("--timeout", type=float, default=1800, help="手順ごとの制限 (秒)")
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parent.parent, help="リポジトリの根"
    )
    args = parser.parse_args(argv)
    if args.quick and args.ignored:
        parser.error("--quick と --ignored は同時に渡せない")
    if args.jobs is not None and args.jobs < 1:
        parser.error("--jobs は 1 以上")
    step_list = steps(sys.executable, args.quick, args.ignored)
    # 知らせの行と見出しを同じ流れに出す (1 回だけ読む)
    out = sys.stdout
    frame = None
    if args.ignored:
        frame = make_frame_session(args.root, sys.executable, args.timeout, len(step_list))
    try:
        if frame is not None:
            frame.start(out)
        return run_all(
            args.root,
            step_list,
            args.timeout,
            out=out,
            jobs=args.jobs,
            resume=args.resume,
        )
    finally:
        if frame is not None:
            frame.stop()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
