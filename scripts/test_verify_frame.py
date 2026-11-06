"""scripts/verify_frame.py のテスト (D-111)。

使い方:
- python scripts/test_verify_frame.py            純関数と写しの見張り (窓を作らない)
- AI_CU_FRAME_REAL=1 python scripts/test_verify_frame.py   実の窓のテストも走らせる (主エージェントだけ。入力は送らない)
"""

import os
import queue
import re
import subprocess
import sys
import threading
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # __pycache__ を作らない (scripts/ は build.rs の引き金)

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import verify_frame  # noqa: E402

OVERLAY_RS = ROOT / "src" / "win32" / "overlay.rs"
REAL = os.environ.get("AI_CU_FRAME_REAL") == "1"
REAL_SKIP_REASON = (
    "実の窓のテストは AI_CU_FRAME_REAL=1 のときだけ走る (窓を作るので主エージェントだけが打つ。D-111)"
)


def rect_ltrb(r):
    x, y, w, h = r
    return (x, y, x + w, y + h)


def overlaps(a, b) -> bool:
    al, at, ar, ab = rect_ltrb(a)
    bl, bt, br, bb = rect_ltrb(b)
    return al < br and bl < ar and at < bb and bt < ab


class ScaleTest(unittest.TestCase):
    def test_scale_rounds_half_up_like_overlay_scale_px(self):
        self.assertEqual(verify_frame.scale(6, 96), 6)
        self.assertEqual(verify_frame.scale(6, 120), 8)
        self.assertEqual(verify_frame.scale(6, 144), 9)
        self.assertEqual(verify_frame.scale(6, 168), 11)
        self.assertEqual(verify_frame.scale(14, 96), 14)
        self.assertEqual(verify_frame.scale(14, 120), 18)
        self.assertEqual(verify_frame.scale(14, 144), 21)
        self.assertEqual(verify_frame.scale(14, 168), 25)
        self.assertEqual(verify_frame.scale(8, 120), 10)

    def test_scale_is_at_least_one(self):
        self.assertEqual(verify_frame.scale(0, 96), 1)
        self.assertEqual(verify_frame.scale(1, 10), 1)


class BandRectsTest(unittest.TestCase):
    def test_primary_monitor_at_120_dpi(self):
        self.assertEqual(
            verify_frame.band_rects((0, 0, 3840, 2160), 120),
            [
                (0, 0, 3840, 8),
                (0, 2152, 3840, 8),
                (0, 8, 8, 2144),
                (3832, 8, 8, 2144),
            ],
        )

    def test_monitor_with_negative_origin_at_96_dpi(self):
        self.assertEqual(
            verify_frame.band_rects((-1920, -200, 0, 880), 96),
            [
                (-1920, -200, 1920, 6),
                (-1920, 874, 1920, 6),
                (-1920, -194, 6, 1068),
                (-6, -194, 6, 1068),
            ],
        )

    def test_bands_stay_inside_monitor_and_do_not_overlap(self):
        for mon in [(0, 0, 3840, 2160), (-1920, -200, 0, 880), (3840, 0, 5120, 1024), (0, 0, 10, 10)]:
            for dpi in (96, 120, 144, 168, 240):
                bands = verify_frame.band_rects(mon, dpi)
                self.assertEqual(len(bands), 4)
                ml, mt, mr, mb = mon
                for b in bands:
                    bl, bt, br, bb = rect_ltrb(b)
                    self.assertTrue(ml <= bl and mt <= bt and br <= mr and bb <= mb, (mon, dpi, b))
                for i in range(4):
                    for j in range(i + 1, 4):
                        self.assertFalse(overlaps(bands[i], bands[j]), (mon, dpi, bands[i], bands[j]))


class LabelRectTest(unittest.TestCase):
    def test_bottom_center_above_band_of_work_area(self):
        # 作業領域はタスクバーの分だけ下が短い
        self.assertEqual(
            verify_frame.label_rect((0, 0, 3840, 2112), 500, 20, 120),
            (1660, 2064, 520, 40),
        )

    def test_centered_on_negative_origin_work_area(self):
        x, y, w, h = verify_frame.label_rect((-1920, -200, 0, 840), 400, 18, 96)
        self.assertEqual((w, h), (416, 34))
        self.assertEqual(x, -1920 + (1920 - 416) // 2)
        self.assertEqual(y + h, 840 - 6)

    def test_not_in_upper_half_of_monitor(self):
        for work, dpi in [((0, 0, 3840, 2112), 120), ((0, 0, 1920, 1040), 96), ((-1920, -200, 0, 880), 144)]:
            _, top, _, bottom = work
            x, y, w, h = verify_frame.label_rect(work, 600, 25, dpi)
            self.assertGreaterEqual(y, (top + bottom) // 2, (work, dpi))
            self.assertLessEqual(y + h, bottom)

    def test_does_not_exceed_work_area(self):
        x, y, w, h = verify_frame.label_rect((0, 0, 100, 30), 500, 50, 96)
        self.assertEqual((x, y, w, h), (0, 0, 100, 30))


class ConstantsTest(unittest.TestCase):
    def test_class_name_differs_from_ai_cu_overlay(self):
        self.assertEqual(verify_frame.CLASS_NAME, "ai_cu_verify_frame")
        self.assertNotEqual(verify_frame.CLASS_NAME, "ai_cu_overlay")

    def test_label_text(self):
        self.assertEqual(
            verify_frame.LABEL_TEXT,
            "ai-cu の重い検査中 — 実入力のテストの間はマウスとキーに触れない",
        )

    def test_alphas_are_below_255_so_window_is_see_through(self):
        self.assertEqual(verify_frame.BAND_ALPHA, 200)
        self.assertEqual(verify_frame.LABEL_ALPHA, 230)

    def test_ex_style_has_all_five_bits(self):
        s = verify_frame.FRAME_EX_STYLE
        for bit in (0x00080000, 0x00000020, 0x08000000, 0x00000080, 0x00000008):
            self.assertTrue(s & bit, hex(bit))
        self.assertEqual(verify_frame.WDA_EXCLUDEFROMCAPTURE, 0x11)


class CopyOfOverlayTest(unittest.TestCase):
    """src/win32/overlay.rs が正で、verify_frame.py の定数は写し。食い違えば落とす。"""

    @classmethod
    def setUpClass(cls):
        cls.src = OVERLAY_RS.read_text(encoding="utf-8")

    def const(self, name: str) -> str:
        m = re.search(rf"^\s*(?:pub\s+)?const\s+{name}\s*:\s*[^=]+=\s*(.+?);\s*$", self.src, re.M)
        self.assertIsNotNone(m, f"overlay.rs に {name} が無い")
        return m.group(1).strip()

    def test_numbers_match(self):
        for name in ("BAND_THICKNESS_96", "LABEL_FONT_PX_96", "LABEL_PAD_96", "RETOPMOST_MS"):
            with self.subTest(name=name):
                self.assertEqual(int(self.const(name).replace("_", "")), getattr(verify_frame, name))

    def test_face_matches(self):
        self.assertEqual(self.const("LABEL_FACE"), f'"{verify_frame.LABEL_FACE}"')

    def test_blue_matches(self):
        parts = [p.strip() for p in self.const("BLUE").strip("()").split(",") if p.strip()]
        self.assertEqual(tuple(int(p, 0) for p in parts), verify_frame.BLUE)

    def test_class_name_differs_from_overlay_class(self):
        overlay_class = self.const("OVERLAY_CLASS").strip('"')
        self.assertEqual(overlay_class, "ai_cu_overlay")
        self.assertNotEqual(verify_frame.CLASS_NAME, overlay_class)


class ArgsTest(unittest.TestCase):
    def test_max_seconds(self):
        self.assertEqual(verify_frame.parse_args(["--max-seconds", "30"]).max_seconds, 30)
        self.assertEqual(verify_frame.parse_args([]).max_seconds, verify_frame.DEFAULT_MAX_SECONDS)

    def test_bad_args_exit_2(self):
        import contextlib
        import io

        for argv in (["--max-seconds", "x"], ["--max-seconds", "0"], ["--nope"]):
            with self.subTest(argv=argv):
                with contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as cm:
                        verify_frame.parse_args(argv)
                self.assertEqual(cm.exception.code, 2)


class LoadFailureTest(unittest.TestCase):
    def test_load_failure_prints_error_line_and_exits_1(self):
        # 窓を作らない: _win32 を、DLL の読み込みの失敗を真似て例外を出すものに差し替える
        import contextlib
        import io
        from unittest import mock

        def boom():
            raise OSError("shcore を読めない (テストの真似)")

        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(verify_frame, "_win32", boom):
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = verify_frame.run(5)
        self.assertEqual(code, 1)
        self.assertEqual(out.getvalue(), "error: OSError: shcore を読めない (テストの真似)\n")
        self.assertIn("Traceback", err.getvalue())


class _FakePost:
    """`_watch_stdin` に渡す偽の w。PostThreadMessageW の呼び出しを記録し、results の順に返す。"""

    def __init__(self, results):
        import types

        self.results = list(results)
        self.calls = []
        self.ctypes = types.SimpleNamespace(get_last_error=lambda: 1444)

    def PostThreadMessageW(self, tid, msg, wparam, lparam):
        self.calls.append((tid, msg, wparam, lparam))
        return self.results.pop(0) if self.results else 1


class _Exited(Exception):
    pass


class WatchStdinTest(unittest.TestCase):
    """窓を作らない。読む fd はパイプにし、PostThreadMessageW は偽物にする。"""

    def watch(self, fake):
        import contextlib
        import io
        from unittest import mock

        r, wfd = os.pipe()
        os.write(wfd, b"some input\n")
        os.close(wfd)
        # os._exit と time.sleep はプロセス全体で差し替えず、引数で差し込む
        exit_mock = mock.Mock(side_effect=_Exited)
        sleeps = []
        err = io.StringIO()
        try:
            with contextlib.redirect_stderr(err):
                try:
                    verify_frame._watch_stdin(fake, 42, fd=r, exit=exit_mock, sleep=sleeps.append)
                except _Exited:
                    pass
        finally:
            os.close(r)
        self.sleeps = sleeps
        return exit_mock, err.getvalue()

    def test_eof_posts_wm_quit_to_ui_thread(self):
        fake = _FakePost([1])
        exit_mock, _ = self.watch(fake)
        self.assertEqual(fake.calls, [(42, verify_frame.WM_QUIT, 0, 0)])
        exit_mock.assert_not_called()

    def test_post_retried_after_failure(self):
        fake = _FakePost([0, 0, 1])
        exit_mock, err = self.watch(fake)
        self.assertEqual(len(fake.calls), 3)
        exit_mock.assert_not_called()
        self.assertIn("GetLastError=1444", err)
        self.assertEqual(self.sleeps, [verify_frame.POST_QUIT_RETRY_S] * 2)

    def test_post_failing_every_time_exits_process_with_3(self):
        fake = _FakePost([0] * verify_frame.POST_QUIT_TRIES)
        exit_mock, err = self.watch(fake)
        self.assertEqual(len(fake.calls), verify_frame.POST_QUIT_TRIES)
        self.assertEqual(verify_frame.EXIT_POST_QUIT_FAILED, 3)
        exit_mock.assert_called_once_with(3)
        self.assertIn("WM_QUIT を送れない", err)

    def test_defaults_are_os_exit_and_time_sleep(self):
        import inspect

        params = inspect.signature(verify_frame._watch_stdin).parameters
        self.assertIs(params["exit"].default, os._exit)
        self.assertIs(params["sleep"].default, __import__("time").sleep)


class _FakeDestroy:
    """Frame.close に渡す偽の w。DestroyWindow は ok に入っている hwnd だけ成功する。"""

    def __init__(self, ok):
        import types

        self.ok = set(ok)
        self.ctypes = types.SimpleNamespace(get_last_error=lambda: 1400)

    def DestroyWindow(self, h):
        return 1 if h in self.ok else 0

    def DeleteObject(self, _h):
        return 1

    def UnregisterClassW(self, _name, _hinst):
        return 1


class FrameCloseTest(unittest.TestCase):
    """窓を作らない。Frame を __init__ を通さずに組み、close が壊せた数だけを数えることを見る。"""

    def test_counts_only_destroyed_windows_and_logs_failures(self):
        import contextlib
        import io

        frame = verify_frame.Frame.__new__(verify_frame.Frame)
        frame.w = _FakeDestroy(ok=[11, 13])
        frame.hinst = 0
        frame.hwnds = [11, 12, 13]
        frame.kinds = {}
        frame.fonts = []
        frame.band_brush = frame.label_brush = None
        frame.class_registered = False
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            frame.close()
        text = err.getvalue()
        self.assertIn("後始末: 窓 2 個を壊した", text)
        self.assertIn("DestroyWindow が失敗 (hwnd=12、GetLastError=1400)", text)
        self.assertEqual(frame.hwnds, [])


# 子: 標準入力を読むスレッドを立てたまま、主が先に終わる (窓は作らない。PostThreadMessageW は偽物)
_CHILD_SRC = """\
import sys, threading, time
sys.dont_write_bytecode = True
sys.path.insert(0, sys.argv[1])
import verify_frame
class W:
    def PostThreadMessageW(self, *a):
        return 1
threading.Thread(target=verify_frame._watch_stdin, args=(W(), 0), daemon=True).start()
time.sleep(0.5)
print("main-done", flush=True)
sys.exit(0)
"""


class _Drain:
    """子の標準出力と標準エラーを起動の直後から別のスレッドで読み続ける (パイプが溜まって子が止まらないように)。

    `communicate` は使わない: 子の標準入力を閉じてしまい、「標準入力を開いたまま終わる」経路を撃てないため。
    標準出力は行ごとに `lines` (Queue) にも入れる (`ready` を待つため)。"""

    def __init__(self, proc):
        self.lines: "queue.Queue[bytes]" = queue.Queue()
        self._out: list[bytes] = []
        self._err: list[bytes] = []
        self._threads = [
            threading.Thread(target=self._pump, args=(proc.stdout, self._out, self.lines), daemon=True),
            threading.Thread(target=self._pump, args=(proc.stderr, self._err, None), daemon=True),
        ]
        for t in self._threads:
            t.start()

    @staticmethod
    def _pump(stream, sink, q):
        try:
            for line in iter(stream.readline, b""):
                sink.append(line)
                if q is not None:
                    q.put(line)
        except (OSError, ValueError):
            pass
        if q is not None:
            q.put(b"")

    def finish(self, timeout: float = 5) -> tuple[str, str]:
        """子が終わった後に呼ぶ。読みのスレッドを待ち、(標準出力, 標準エラー) を返す。"""
        for t in self._threads:
            t.join(timeout=timeout)
        dec = lambda xs: b"".join(xs).decode("utf-8", errors="replace")  # noqa: E731
        return dec(self._out), dec(self._err)


def _close_pipes(proc):
    for s in (proc.stdin, proc.stdout, proc.stderr):
        try:
            if s is not None:
                s.close()
        except OSError:
            pass


class WatchStdinChildTest(unittest.TestCase):
    def test_main_exits_0_while_stdin_still_open(self):
        # 標準入力が開いたまま主が終わっても (上限の時間、例外、Ctrl+C の経路)、
        # 読むスレッドのせいで終了の処理が落ちない (_enter_buffered_busy で exit 139 にならない)
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            script = Path(tmp) / "child_watch_stdin.py"
            script.write_text(_CHILD_SRC, encoding="utf-8")
            proc = subprocess.Popen(
                [sys.executable, "-B", str(script), str(HERE)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            drain = _Drain(proc)
            try:
                code = proc.wait(timeout=20)
                out, err = drain.finish()
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait(timeout=10)
                _close_pipes(proc)
        self.assertIn("main-done", out)
        self.assertNotIn("Fatal Python error", err)
        self.assertEqual(code, 0, err)


# ---- 実の窓 (AI_CU_FRAME_REAL=1 のときだけ) ----


def _real_api():
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    LPARAM = ctypes.c_ssize_t
    WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, LPARAM)
    MONITORENUMPROC = ctypes.WINFUNCTYPE(
        wintypes.BOOL, wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.RECT), LPARAM
    )
    sig = {
        "SetProcessDpiAwarenessContext": (wintypes.BOOL, [wintypes.HANDLE]),
        "EnumWindows": (wintypes.BOOL, [WNDENUMPROC, LPARAM]),
        "EnumDisplayMonitors": (
            wintypes.BOOL,
            [wintypes.HANDLE, ctypes.POINTER(wintypes.RECT), MONITORENUMPROC, LPARAM],
        ),
        "GetWindowThreadProcessId": (wintypes.DWORD, [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]),
        "GetClassNameW": (ctypes.c_int, [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]),
        "IsWindowVisible": (wintypes.BOOL, [wintypes.HWND]),
        "GetWindowLongPtrW": (ctypes.c_ssize_t, [wintypes.HWND, ctypes.c_int]),
        "GetWindowDisplayAffinity": (wintypes.BOOL, [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]),
        "GetForegroundWindow": (wintypes.HWND, []),
        "WindowFromPoint": (wintypes.HWND, [wintypes.POINT]),
        "GetWindowRect": (wintypes.BOOL, [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]),
    }
    api = {}
    for name, (res, args) in sig.items():
        f = getattr(user32, name)
        f.restype, f.argtypes = res, args
        api[name] = f
    api["WNDENUMPROC"] = WNDENUMPROC
    api["MONITORENUMPROC"] = MONITORENUMPROC
    api["ctypes"] = ctypes
    api["wintypes"] = wintypes
    return api


def _pid_of(api, hwnd) -> int:
    pid = api["wintypes"].DWORD(0)
    api["GetWindowThreadProcessId"](hwnd, api["ctypes"].byref(pid))
    return pid.value


def _windows_of(api, pid: int) -> list[dict]:
    """pid の持つトップレベルの窓 (クラス、見えるか、拡張スタイル、矩形)。"""
    ct = api["ctypes"]
    found = []

    def cb(hwnd, _lparam):
        if _pid_of(api, hwnd) == pid:
            buf = ct.create_unicode_buffer(256)
            api["GetClassNameW"](hwnd, buf, 256)
            rc = api["wintypes"].RECT()
            api["GetWindowRect"](hwnd, ct.byref(rc))
            found.append(
                {
                    "hwnd": hwnd,
                    "class": buf.value,
                    "visible": bool(api["IsWindowVisible"](hwnd)),
                    "ex": api["GetWindowLongPtrW"](hwnd, -20),  # GWL_EXSTYLE
                    "rect": (rc.left, rc.top, rc.right, rc.bottom),
                }
            )
        return 1

    proc = api["WNDENUMPROC"](cb)
    api["EnumWindows"](proc, 0)
    return found


def _monitor_count(api) -> int:
    n = []

    def cb(_h, _dc, _r, _l):
        n.append(1)
        return 1

    api["EnumDisplayMonitors"](None, None, api["MONITORENUMPROC"](cb), 0)
    return len(n)


@unittest.skipUnless(REAL, REAL_SKIP_REASON)
class RealWindowTest(unittest.TestCase):
    """子を起動して窓を見せ、子の pid の窓だけを見て性質を確かめる。入力は送らない。"""

    def spawn(self, max_seconds: int):
        env = dict(os.environ)
        env["PYTHONIOENCODING"] = "utf-8"
        return subprocess.Popen(
            [sys.executable, "-B", str(HERE / "verify_frame.py"), "--max-seconds", str(max_seconds)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )

    def test_real_frame_ends_at_max_seconds_with_stdin_open(self):
        # 標準入力を閉じずに上限の時間で終わる経路 (win32-reviewer の M1)。
        # 標準入力を読むスレッドが残ったままでも、終了の処理で落ちずに 0 で終わる
        proc = self.spawn(1)
        drain = _Drain(proc)
        try:
            code = proc.wait(timeout=5)
            out, err = drain.finish()
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=10)
            _close_pipes(proc)
        print(f"FRAME max_seconds code={code} stderr={err.strip()!r}", file=sys.stderr)
        self.assertEqual(out.strip(), "ready")
        self.assertNotIn("Fatal Python error", err)
        self.assertIn("後始末: 窓", err)
        self.assertNotIn("DestroyWindow が失敗", err)
        self.assertEqual(code, 0)

    def test_real_frame_windows(self):
        api = _real_api()
        # 物理ピクセルで矩形と点を扱う (子と同じ Per-Monitor V2)
        api["SetProcessDpiAwarenessContext"](-4)
        # 起動の前の前面 (子が前面を取らないことを、起動の前後で同じことで見る)
        fg_before = api["GetForegroundWindow"]()
        proc = self.spawn(30)
        drain = _Drain(proc)
        try:
            first = drain.lines.get(timeout=10).decode("utf-8", errors="replace").strip()
            self.assertEqual(first, "ready")

            mine = _windows_of(api, proc.pid)
            frames = [w for w in mine if w["class"] == verify_frame.CLASS_NAME and w["visible"]]
            n_mon = _monitor_count(api)
            print(f"FRAME monitors={n_mon} frame_windows={len(frames)}", file=sys.stderr)
            self.assertEqual(len(frames), n_mon * 5)
            # 子の pid の `ai_cu_overlay` が 0。CLASS_NAME を書き換えると上の数の断言も同じクラスで
            # 数えるので、この断言は空振りしうる。クラス名を分けたことの主の守りは、写しのテスト
            # (ConstantsTest と CopyOfOverlayTest のクラス名の断言)
            self.assertEqual([w for w in mine if w["class"] == "ai_cu_overlay"], [])

            ct, wt = api["ctypes"], api["wintypes"]
            for w in frames:
                for bit in (0x00080000, 0x00000020, 0x08000000, 0x00000080, 0x00000008):
                    self.assertTrue(w["ex"] & bit, (hex(w["ex"]), hex(bit)))
                aff = wt.DWORD(0)
                self.assertTrue(api["GetWindowDisplayAffinity"](w["hwnd"], ct.byref(aff)))
                self.assertEqual(aff.value, 0x11)

            # 前面は起動の前と同じ (NULL でも skip せずに記録して比べる)
            fg_after = api["GetForegroundWindow"]()
            print(f"FRAME foreground before={fg_before} after={fg_after}", file=sys.stderr)
            self.assertEqual(fg_after, fg_before)
            if fg_after:
                self.assertNotEqual(_pid_of(api, fg_after), proc.pid)

            # 帯 (モニターごとに 4 本) と札 (1 枚) の両方の中央の点で、WindowFromPoint が子の pid の窓でない。
            # その点で子の窓より上に別の最前面の窓があっても通ってしまう (点の窓が子の窓より下の窓かは
            # 確かめられない) ので、帯だけでなく札の中央も見て当たる点を増やす
            for w in frames:
                left, top, right, bottom = w["rect"]
                pt = wt.POINT((left + right) // 2, (top + bottom) // 2)
                hit = api["WindowFromPoint"](pt)
                self.assertTrue(not hit or _pid_of(api, hit) != proc.pid, (w["rect"], hit))

            proc.stdin.close()
            self.assertEqual(proc.wait(timeout=2), 0)
            _, err = drain.finish()
            # 窓を壊したことは子の後始末の行で見る (close が DestroyWindow の成功した数を出す)
            self.assertNotIn("DestroyWindow が失敗", err)
            self.assertIn(f"後始末: 窓 {n_mon * 5} 個を壊した", err)
            # 子のプロセスが消えたことの確かめ (終わったプロセスの pid の窓は列挙されないので、
            # 窓を壊したことの確かめにはならない。それは上の後始末の行で見る)
            self.assertEqual(
                [w for w in _windows_of(api, proc.pid) if w["class"] == verify_frame.CLASS_NAME], []
            )
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=10)
            _close_pipes(proc)


if __name__ == "__main__":
    unittest.main(verbosity=2)
