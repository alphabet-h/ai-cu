"""重い検査 (verify.py --ignored) の間、全モニターの縁に青い枠と札を出しておく子プロセス (D-111)。

何を出すか: モニターごとに、縁の 4 本の帯 (上・下・左・右) と、作業領域の下の中央の札 1 枚
(「ai-cu の重い検査中 — 実入力のテストの間はマウスとキーに触れない」)。帯は一様な青
(`SetLayeredWindowAttributes(LWA_ALPHA, 200)`)、札は青の地 (alpha 230) に白の文字。音は鳴らさない。

窓: クラス名 `ai_cu_verify_frame` (ai-cu の枠の `ai_cu_overlay` と別。ai-cu の `overlay_windows` に
数えさせない)。`WS_POPUP` に `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW |
WS_EX_TOPMOST`、題名なし、`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (外せなければ出さない)、
`SW_SHOWNOACTIVATE` で見せ、2 秒ごとに `SetWindowPos(HWND_TOPMOST, … SWP_NOACTIVATE)` で最前面に上げ直す。
撮影に写らず、ヒットテストを素通りし、前面を取らない (ai-cu の枠と同じ属性)。

定数 (青、太さ、文字の大きさ、余白、書体、上げ直しの間隔) と DPI の拡大の式 (四捨五入) は
src/win32/overlay.rs が正で、ここは写し。定数は scripts/test_verify_frame.py が overlay.rs と読み比べて見張る。
式は読み比べず、overlay.rs のテストと同じ値の表 (test_verify_frame.py の ScaleTest) で確かめる。

プロトコル:
- 起動: `python scripts/verify_frame.py [--max-seconds N]` (N は正の整数の秒。既定 600)
- 窓を作って見せたら、標準出力に `ready` の 1 行を出す。出せなければ `error: <理由>` の 1 行を出して終わる
- 標準入力が閉じたら (EOF) 窓を壊して終わる。`--max-seconds` を過ぎても自分で終わる (親が固まったときの守り)
- 標準入力は別のスレッドが `os.read` で fd から読む。`sys.stdin.buffer.read` で読むと、そのスレッドが
  BufferedReader のロックを握ったまま主が終わり、終了の処理で `Fatal Python error: _enter_buffered_busy` に
  なって落ちる (標準入力が開いたまま上限の時間や例外で終わる場合)
- 後始末 (窓を壊す) の後に標準エラーへ `verify_frame: 後始末: 窓 <n> 個を壊した` の 1 行を出す
  (n は DestroyWindow が成功した数。失敗は 1 つずつ GetLastError つきで標準エラーに出す)
- ログ (例外など) は標準エラー

終了コード: 0 正常 (標準入力の EOF か上限の時間)、1 出せない (窓を作れない、撮影から外せないなど) か
中断 (Ctrl+C。後始末の後に終える)、2 引数の誤り、3 標準入力の EOF の後に UI のスレッドへ WM_QUIT を
送れず、後始末をせずに終えた (窓は OS が壊す)。verify.py は `ready` の後の子の終了コードを見ない。

FFI を src/win32/ の外に置く例外 (D-111) なので、ctypes の宣言は win32-reviewer のレビューの対象。
import しただけでは Win32 を呼ばない (`main` から呼ぶ)。
"""

from __future__ import annotations

import argparse
import os
import sys
import threading
import time

CLASS_NAME = "ai_cu_verify_frame"
LABEL_TEXT = "ai-cu の重い検査中 — 実入力のテストの間はマウスとキーに触れない"

# ---- src/win32/overlay.rs の写し (test_verify_frame.py が読み比べる) ----
BAND_THICKNESS_96 = 6
LABEL_FONT_PX_96 = 14
LABEL_PAD_96 = 8
LABEL_FACE = "Yu Gothic UI"
BLUE = (0x2F, 0x7B, 0xFF)
RETOPMOST_MS = 2000
# ---- ここまで写し ----

BAND_ALPHA = 200  # 一様な帯の不透明度 (255 未満なので層の属性で透け、撮影の覆いに数えられない。D-077)
LABEL_ALPHA = 230
DEFAULT_MAX_SECONDS = 600
# SetTimer の上限 (USER_TIMER_MAXIMUM、ミリ秒)
USER_TIMER_MAXIMUM = 0x7FFFFFFF
# 標準入力の EOF で WM_QUIT を送るやり直しの回数と間 (尽きたらそのスレッドからプロセスを終える)
POST_QUIT_TRIES = 5
POST_QUIT_RETRY_S = 0.1
# WM_QUIT を送れずにそのスレッドからプロセスを終えるときの終了コード
EXIT_POST_QUIT_FAILED = 3

Rect = tuple[int, int, int, int]


# ---- 純関数 (Win32 を呼ばない) ----


def scale(px: int, dpi: int) -> int:
    """96 DPI のピクセルをモニターの DPI に拡大する (四捨五入、1 以上)。overlay.rs の `scale_px` と同じ式。

    Python の `round` は偶数への丸めなので使わない。
    """
    return max(1, (px * dpi + 48) // 96)


def band_rects(monitor: Rect, dpi: int) -> list[Rect]:
    """モニターの縁を覆う 4 本の帯 (上・下・左・右) を (x, y, w, h) で返す。

    `monitor` は (left, top, right, bottom)。太さは `scale(BAND_THICKNESS_96, dpi)` で、モニターの幅と
    高さの半分で頭打ち。上と下は全幅、左と右は上下の帯の間を埋める (重ならない)。overlay.rs の
    `band_rects` と同じ形。
    """
    left, top, right, bottom = monitor
    w, h = right - left, bottom - top
    t = max(0, min(scale(BAND_THICKNESS_96, dpi), w // 2, h // 2))
    inner_h = h - 2 * t
    return [
        (left, top, w, t),
        (left, bottom - t, w, t),
        (left, top + t, t, inner_h),
        (right - t, top + t, t, inner_h),
    ]


def label_rect(work: Rect, text_w: int, text_h: int, dpi: int) -> Rect:
    """札の矩形 (x, y, w, h)。`work` はモニターの作業領域 (GetMonitorInfoW の rcWork、(left, top, right, bottom))。

    大きさは文字の幅と高さに余白 `scale(LABEL_PAD_96, dpi)` を四方に足したもの (作業領域を超えない)。
    横は中央、縦は作業領域の下端から下の帯の太さ (`scale(BAND_THICKNESS_96, dpi)`) の分だけ上に下端を置く
    (タスクバーのアイコンにも、上の中央に出る ai-cu の札にも重ねない)。作業領域の上端より上には出さない。
    """
    left, top, right, bottom = work
    ww, wh = right - left, bottom - top
    pad = scale(LABEL_PAD_96, dpi)
    w = max(0, min(max(0, text_w) + 2 * pad, ww))
    h = max(0, min(max(0, text_h) + 2 * pad, wh))
    x = left + (ww - w) // 2
    y = max(top, bottom - scale(BAND_THICKNESS_96, dpi) - h)
    return (x, y, w, h)


def parse_args(argv: list[str]) -> argparse.Namespace:
    """引数を読む。誤りは argparse が終了コード 2 で終える。"""

    def positive_int(s: str) -> int:
        try:
            v = int(s)
        except ValueError:
            raise argparse.ArgumentTypeError(f"整数ではない: {s!r}") from None
        if v <= 0:
            raise argparse.ArgumentTypeError(f"正の整数ではない: {v}")
        return v

    p = argparse.ArgumentParser(
        prog="verify_frame.py",
        description="重い検査の間、画面の縁に枠を出す (D-111)。標準入力の EOF で終わる",
    )
    p.add_argument(
        "--max-seconds",
        type=positive_int,
        default=DEFAULT_MAX_SECONDS,
        help=f"この秒数を過ぎたら自分で終わる (既定 {DEFAULT_MAX_SECONDS})",
    )
    return p.parse_args(argv)


# ---- Win32 (main からだけ呼ぶ) ----


class FrameError(Exception):
    """枠を出せない理由 (`error: <理由>` の行になる)"""


def _win32():
    """ctypes の宣言をまとめて返す。64 ビットのハンドルとポインタを切り詰めないよう、argtypes と restype を必ず付ける。"""
    import ctypes
    from ctypes import wintypes

    class W:
        pass

    w = W()
    w.ctypes = ctypes
    w.wintypes = wintypes

    LRESULT = ctypes.c_ssize_t
    WPARAM = ctypes.c_size_t
    LPARAM = ctypes.c_ssize_t
    HWND = wintypes.HWND
    UINT = wintypes.UINT
    BOOL = wintypes.BOOL
    DWORD = wintypes.DWORD
    HANDLE = wintypes.HANDLE
    w.LRESULT, w.WPARAM, w.LPARAM = LRESULT, WPARAM, LPARAM

    WNDPROC = ctypes.WINFUNCTYPE(LRESULT, HWND, UINT, WPARAM, LPARAM)
    MONITORENUMPROC = ctypes.WINFUNCTYPE(
        BOOL, HANDLE, HANDLE, ctypes.POINTER(wintypes.RECT), LPARAM
    )
    w.WNDPROC, w.MONITORENUMPROC = WNDPROC, MONITORENUMPROC

    class WNDCLASSEXW(ctypes.Structure):
        _fields_ = [
            ("cbSize", UINT),
            ("style", UINT),
            ("lpfnWndProc", WNDPROC),
            ("cbClsExtra", ctypes.c_int),
            ("cbWndExtra", ctypes.c_int),
            ("hInstance", HANDLE),
            ("hIcon", HANDLE),
            ("hCursor", HANDLE),
            ("hbrBackground", HANDLE),
            ("lpszMenuName", wintypes.LPCWSTR),
            ("lpszClassName", wintypes.LPCWSTR),
            ("hIconSm", HANDLE),
        ]

    class MSG(ctypes.Structure):
        _fields_ = [
            ("hwnd", HWND),
            ("message", UINT),
            ("wParam", WPARAM),
            ("lParam", LPARAM),
            ("time", DWORD),
            ("pt", wintypes.POINT),
        ]

    class PAINTSTRUCT(ctypes.Structure):
        _fields_ = [
            ("hdc", HANDLE),
            ("fErase", BOOL),
            ("rcPaint", wintypes.RECT),
            ("fRestore", BOOL),
            ("fIncUpdate", BOOL),
            ("rgbReserved", ctypes.c_byte * 32),
        ]

    class MONITORINFO(ctypes.Structure):
        _fields_ = [
            ("cbSize", DWORD),
            ("rcMonitor", wintypes.RECT),
            ("rcWork", wintypes.RECT),
            ("dwFlags", DWORD),
        ]

    w.WNDCLASSEXW, w.MSG, w.PAINTSTRUCT, w.MONITORINFO = (
        WNDCLASSEXW,
        MSG,
        PAINTSTRUCT,
        MONITORINFO,
    )

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    shcore = ctypes.WinDLL("shcore", use_last_error=True)

    def fn(dll, name, restype, argtypes):
        f = getattr(dll, name)
        f.restype = restype
        f.argtypes = argtypes
        setattr(w, name, f)

    P = ctypes.POINTER
    fn(user32, "SetProcessDpiAwarenessContext", BOOL, [HANDLE])
    fn(kernel32, "GetModuleHandleW", HANDLE, [wintypes.LPCWSTR])
    fn(kernel32, "GetCurrentThreadId", DWORD, [])
    fn(user32, "RegisterClassExW", wintypes.ATOM, [P(WNDCLASSEXW)])
    fn(user32, "UnregisterClassW", BOOL, [wintypes.LPCWSTR, HANDLE])
    fn(user32, "DefWindowProcW", LRESULT, [HWND, UINT, WPARAM, LPARAM])
    fn(
        user32,
        "CreateWindowExW",
        HWND,
        [
            DWORD,
            wintypes.LPCWSTR,
            wintypes.LPCWSTR,
            DWORD,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            HWND,
            HANDLE,
            HANDLE,
            ctypes.c_void_p,
        ],
    )
    fn(user32, "DestroyWindow", BOOL, [HWND])
    # bAlpha は BYTE (符号なし)。wintypes.BYTE は符号付きの c_byte なので使わない
    fn(user32, "SetLayeredWindowAttributes", BOOL, [HWND, DWORD, ctypes.c_ubyte, DWORD])
    fn(user32, "SetWindowDisplayAffinity", BOOL, [HWND, DWORD])
    fn(user32, "GetWindowDisplayAffinity", BOOL, [HWND, P(DWORD)])
    fn(user32, "ShowWindow", BOOL, [HWND, ctypes.c_int])
    fn(
        user32,
        "SetWindowPos",
        BOOL,
        [HWND, HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, UINT],
    )
    fn(user32, "SetTimer", ctypes.c_size_t, [HWND, ctypes.c_size_t, UINT, ctypes.c_void_p])
    fn(user32, "KillTimer", BOOL, [HWND, ctypes.c_size_t])
    fn(user32, "GetMessageW", BOOL, [P(MSG), HWND, UINT, UINT])
    fn(user32, "TranslateMessage", BOOL, [P(MSG)])
    fn(user32, "DispatchMessageW", LRESULT, [P(MSG)])
    fn(user32, "PostThreadMessageW", BOOL, [DWORD, UINT, WPARAM, LPARAM])
    fn(user32, "EnumDisplayMonitors", BOOL, [HANDLE, P(wintypes.RECT), MONITORENUMPROC, LPARAM])
    fn(user32, "GetMonitorInfoW", BOOL, [HANDLE, P(MONITORINFO)])
    fn(shcore, "GetDpiForMonitor", ctypes.c_long, [HANDLE, ctypes.c_int, P(UINT), P(UINT)])
    fn(user32, "BeginPaint", HANDLE, [HWND, P(PAINTSTRUCT)])
    fn(user32, "EndPaint", BOOL, [HWND, P(PAINTSTRUCT)])
    fn(user32, "GetClientRect", BOOL, [HWND, P(wintypes.RECT)])
    fn(user32, "FillRect", ctypes.c_int, [HANDLE, P(wintypes.RECT), HANDLE])
    fn(user32, "DrawTextW", ctypes.c_int, [HANDLE, wintypes.LPCWSTR, ctypes.c_int, P(wintypes.RECT), UINT])
    fn(gdi32, "CreateSolidBrush", HANDLE, [DWORD])
    fn(
        gdi32,
        "CreateFontW",
        HANDLE,
        [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            DWORD,
            DWORD,
            DWORD,
            DWORD,
            DWORD,
            DWORD,
            DWORD,
            DWORD,
            wintypes.LPCWSTR,
        ],
    )
    fn(gdi32, "CreateCompatibleDC", HANDLE, [HANDLE])
    fn(gdi32, "DeleteDC", BOOL, [HANDLE])
    fn(gdi32, "SelectObject", HANDLE, [HANDLE, HANDLE])
    fn(gdi32, "DeleteObject", BOOL, [HANDLE])
    fn(gdi32, "SetBkMode", ctypes.c_int, [HANDLE, ctypes.c_int])
    fn(gdi32, "SetTextColor", DWORD, [HANDLE, DWORD])
    return w


# Win32 の定数
WS_POPUP = 0x80000000
WS_EX_LAYERED = 0x00080000
WS_EX_TRANSPARENT = 0x00000020
WS_EX_NOACTIVATE = 0x08000000
WS_EX_TOOLWINDOW = 0x00000080
WS_EX_TOPMOST = 0x00000008
FRAME_EX_STYLE = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
WDA_EXCLUDEFROMCAPTURE = 0x00000011
LWA_ALPHA = 0x00000002
SW_SHOWNOACTIVATE = 4
HWND_TOPMOST = -1
SWP_NOSIZE = 0x0001
SWP_NOMOVE = 0x0002
SWP_NOACTIVATE = 0x0010
WM_PAINT = 0x000F
WM_QUIT = 0x0012
WM_TIMER = 0x0113
WM_MOUSEACTIVATE = 0x0021
MA_NOACTIVATE = 3
MDT_EFFECTIVE_DPI = 0
DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
ERROR_CLASS_ALREADY_EXISTS = 1410
FW_SEMIBOLD = 600
DEFAULT_CHARSET = 1
OUT_DEFAULT_PRECIS = 0
CLIP_DEFAULT_PRECIS = 0
ANTIALIASED_QUALITY = 4
DEFAULT_PITCH = 0
TRANSPARENT = 1
DT_CENTER = 0x0001
DT_VCENTER = 0x0004
DT_SINGLELINE = 0x0020
DT_NOPREFIX = 0x0800
DT_CALCRECT = 0x0400


def _rgb(c: tuple[int, int, int]) -> int:
    """COLORREF (0x00BBGGRR)"""
    r, g, b = c
    return r | (g << 8) | (b << 16)


def _log(msg: str) -> None:
    try:
        print(f"verify_frame: {msg}", file=sys.stderr, flush=True)
    except OSError:
        pass


def _say(line: str) -> None:
    """親に 1 行を渡す (標準出力)。親が先にいなくなっていても落とさない。"""
    try:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()
    except OSError:
        pass


class Frame:
    """枠の窓の組と GDI の資源。`close` で (途中の失敗でも) 作った分を全部片付ける。"""

    def __init__(self, w) -> None:
        self.w = w
        self.hinst = w.GetModuleHandleW(None)
        self.hwnds: list[int] = []
        # hwnd → ("band", None) か ("label", hfont)
        self.kinds: dict[int, tuple[str, int | None]] = {}
        self.fonts: list[int] = []
        self.band_brush = None
        self.label_brush = None
        self.class_registered = False
        # WNDPROC の参照をプロセスの終わりまで持つ (集められると呼び出しで落ちる)
        self.wndproc = w.WNDPROC(self._wndproc)

    # -- 窓の手続き --
    def _wndproc(self, hwnd, msg, wparam, lparam):
        w = self.w
        try:
            if msg == WM_MOUSEACTIVATE:
                return MA_NOACTIVATE
            if msg == WM_PAINT:
                self._paint(hwnd)
                return 0
        except Exception as e:  # noqa: BLE001 (コールバックの外へ例外を出さない)
            _log(f"窓の手続きで例外: {e!r}")
        return w.DefWindowProcW(hwnd, msg, wparam, lparam)

    def _paint(self, hwnd) -> None:
        w = self.w
        ps = w.PAINTSTRUCT()
        hdc = w.BeginPaint(hwnd, w.ctypes.byref(ps))
        if not hdc:
            return
        try:
            rc = w.wintypes.RECT()
            w.GetClientRect(hwnd, w.ctypes.byref(rc))
            kind, font = self.kinds.get(int(hwnd or 0), ("band", None))
            if kind == "label":
                w.FillRect(hdc, w.ctypes.byref(rc), self.label_brush)
                old = w.SelectObject(hdc, font) if font else None
                try:
                    w.SetBkMode(hdc, TRANSPARENT)
                    w.SetTextColor(hdc, 0x00FFFFFF)
                    w.DrawTextW(
                        hdc,
                        LABEL_TEXT,
                        -1,
                        w.ctypes.byref(rc),
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    )
                finally:
                    if old:
                        w.SelectObject(hdc, old)
            else:
                w.FillRect(hdc, w.ctypes.byref(rc), self.band_brush)
        finally:
            w.EndPaint(hwnd, w.ctypes.byref(ps))

    # -- 作る --
    def build(self) -> None:
        w = self.w
        ct = w.ctypes
        # GetLastError は各呼び出しの直後に読む (次の呼び出しで上書きされうる)
        self.band_brush = w.CreateSolidBrush(_rgb(BLUE))
        if not self.band_brush:
            raise FrameError(f"CreateSolidBrush (帯) が失敗 (GetLastError={ct.get_last_error()})")
        self.label_brush = w.CreateSolidBrush(_rgb(BLUE))
        if not self.label_brush:
            raise FrameError(f"CreateSolidBrush (札) が失敗 (GetLastError={ct.get_last_error()})")

        wc = w.WNDCLASSEXW()
        wc.cbSize = ct.sizeof(w.WNDCLASSEXW)
        wc.style = 0
        wc.lpfnWndProc = self.wndproc
        wc.hInstance = self.hinst
        wc.hbrBackground = None
        wc.lpszClassName = CLASS_NAME
        # 既にある (ERROR_CLASS_ALREADY_EXISTS) も失敗にする。同じプロセスで既にあるクラスは別の
        # 窓の手続きを指しうるので、そのまま使わない
        if not w.RegisterClassExW(ct.byref(wc)):
            err = ct.get_last_error()
            what = " (ERROR_CLASS_ALREADY_EXISTS)" if err == ERROR_CLASS_ALREADY_EXISTS else ""
            raise FrameError(f"RegisterClassExW が失敗 (GetLastError={err}{what})")
        self.class_registered = True

        monitors = self._monitors()
        if not monitors:
            raise FrameError("モニターが見つからない (EnumDisplayMonitors)")
        for mon, work, dpi in monitors:
            for rect in band_rects(mon, dpi):
                self._make(rect, BAND_ALPHA, ("band", None))
            font = w.CreateFontW(
                -scale(LABEL_FONT_PX_96, dpi),
                0,
                0,
                0,
                FW_SEMIBOLD,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH,
                LABEL_FACE,
            )
            if not font:
                raise FrameError(f"CreateFontW が失敗 (GetLastError={ct.get_last_error()})")
            self.fonts.append(font)
            tw, th = self._measure(font)
            self._make(label_rect(work, tw, th, dpi), LABEL_ALPHA, ("label", font))

    def _monitors(self) -> list[tuple[Rect, Rect, int]]:
        w = self.w
        ct = w.ctypes
        handles: list[int] = []

        def on_monitor(hmon, _hdc, _rect, _lparam):
            handles.append(hmon)
            return 1

        cb = w.MONITORENUMPROC(on_monitor)
        if not w.EnumDisplayMonitors(None, None, cb, 0):
            raise FrameError(f"EnumDisplayMonitors が失敗 (GetLastError={ct.get_last_error()})")
        out = []
        for h in handles:
            mi = w.MONITORINFO()
            mi.cbSize = ct.sizeof(w.MONITORINFO)
            if not w.GetMonitorInfoW(h, ct.byref(mi)):
                continue
            m, k = mi.rcMonitor, mi.rcWork
            if m.right - m.left <= 0 or m.bottom - m.top <= 0:
                continue
            dx, dy = w.wintypes.UINT(96), w.wintypes.UINT(96)
            hr = w.GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, ct.byref(dx), ct.byref(dy))
            dpi = dx.value if hr >= 0 and dx.value > 0 else 96
            out.append(
                (
                    (m.left, m.top, m.right, m.bottom),
                    (k.left, k.top, k.right, k.bottom),
                    dpi,
                )
            )
        return out

    def _measure(self, font) -> tuple[int, int]:
        w = self.w
        ct = w.ctypes
        dc = w.CreateCompatibleDC(None)
        if not dc:
            raise FrameError(f"CreateCompatibleDC が失敗 (GetLastError={ct.get_last_error()})")
        try:
            old = w.SelectObject(dc, font)
            try:
                rc = w.wintypes.RECT(0, 0, 0, 0)
                if not w.DrawTextW(dc, LABEL_TEXT, -1, ct.byref(rc), DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX):
                    raise FrameError("DrawTextW (大きさ) が失敗")
                return (rc.right - rc.left, rc.bottom - rc.top)
            finally:
                if old:
                    w.SelectObject(dc, old)
        finally:
            w.DeleteDC(dc)

    def _make(self, rect: Rect, alpha: int, kind: tuple[str, int | None]) -> None:
        w = self.w
        ct = w.ctypes
        x, y, cw, ch = rect
        if cw <= 0 or ch <= 0:
            return
        hwnd = w.CreateWindowExW(
            FRAME_EX_STYLE, CLASS_NAME, None, WS_POPUP, x, y, cw, ch, None, None, self.hinst, None
        )
        if not hwnd:
            raise FrameError(f"CreateWindowExW が失敗 (GetLastError={ct.get_last_error()})")
        self.hwnds.append(hwnd)
        self.kinds[int(hwnd)] = kind
        if not w.SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA):
            raise FrameError(f"SetLayeredWindowAttributes が失敗 (GetLastError={ct.get_last_error()})")
        if not w.SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE):
            raise FrameError(f"SetWindowDisplayAffinity が失敗 (GetLastError={ct.get_last_error()})")
        # 読み戻す。Windows 10 2004 より前は WDA_EXCLUDEFROMCAPTURE が WDA_MONITOR (黒く写る) になる
        aff = w.wintypes.DWORD(0)
        if not w.GetWindowDisplayAffinity(hwnd, ct.byref(aff)):
            raise FrameError(f"GetWindowDisplayAffinity が失敗 (GetLastError={ct.get_last_error()})")
        if aff.value != WDA_EXCLUDEFROMCAPTURE:
            raise FrameError(
                f"撮影から外せない (GetWindowDisplayAffinity={aff.value:#x}、{WDA_EXCLUDEFROMCAPTURE:#x} でない)"
            )

    def show(self) -> None:
        for h in self.hwnds:
            self.w.ShowWindow(h, SW_SHOWNOACTIVATE)

    def raise_topmost(self) -> None:
        for h in self.hwnds:
            self.w.SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)

    # -- 片付ける --
    def close(self) -> None:
        w = self.w
        # 壊せた窓だけを数える (後始末の行は実の窓のテストが断言する)
        n = 0
        for h in self.hwnds:
            if w.DestroyWindow(h):
                n += 1
            else:
                _log(f"DestroyWindow が失敗 (hwnd={h}、GetLastError={w.ctypes.get_last_error()})")
        self.hwnds.clear()
        self.kinds.clear()
        for f in self.fonts:
            w.DeleteObject(f)
        self.fonts.clear()
        for b in (self.band_brush, self.label_brush):
            if b:
                w.DeleteObject(b)
        self.band_brush = self.label_brush = None
        if self.class_registered:
            w.UnregisterClassW(CLASS_NAME, self.hinst)
            self.class_registered = False
        _log(f"後始末: 窓 {n} 個を壊した")


def _watch_stdin(
    w,
    ui_thread_id: int,
    fd: int | None = None,
    exit=os._exit,  # noqa: A002 (テストが差し込む。プロセス全体の os._exit を差し替えないため)
    sleep=time.sleep,
) -> None:
    """標準入力 (`fd`。既定は sys.stdin の fd) を EOF まで読み捨て、UI のスレッドに WM_QUIT を送る。

    `sys.stdin.buffer.read` は使わない: daemon のスレッドが BufferedReader のロックを握ったまま主が終わると、
    終了の処理で `Fatal Python error: _enter_buffered_busy` になって落ちる。`os.read` はロックを持たない。
    WM_QUIT を送れなければ `sleep` を挟んで数回やり直し、尽きたらこのスレッドからプロセスを終える
    (`exit(EXIT_POST_QUIT_FAILED)`。既定は `os._exit`。窓は OS が壊す)。UI のスレッドが待ち続けて子が残るより良い。
    """
    try:
        if fd is None:
            fd = sys.stdin.fileno()
        while os.read(fd, 4096):
            pass
    except (OSError, ValueError, AttributeError):
        pass
    for i in range(POST_QUIT_TRIES):
        if w.PostThreadMessageW(ui_thread_id, WM_QUIT, 0, 0):
            return
        _log(f"PostThreadMessageW が失敗 ({i + 1} 回目、GetLastError={w.ctypes.get_last_error()})")
        sleep(POST_QUIT_RETRY_S)
    _log(f"WM_QUIT を送れないので、後始末をせずにプロセスを終える (終了コード {EXIT_POST_QUIT_FAILED}。窓は OS が壊す)")
    exit(EXIT_POST_QUIT_FAILED)


def _fail(e: Exception) -> int:
    """予期しない例外を標準エラーの traceback と `error: <型>: <文>` の 1 行にして 1 を返す。"""
    import traceback

    traceback.print_exc(file=sys.stderr)
    _say(f"error: {type(e).__name__}: {e}")
    return 1


def run(max_seconds: int) -> int:
    try:
        # DLL と関数の読み込み (shcore が無い、関数が解決できないなど) の失敗も error 行にする
        w = _win32()
        ct = w.ctypes
        # 最初の窓を作る前に Per-Monitor V2 を宣言する。失敗しても続ける (座標がずれるだけ。spec の「自分で決めたこと」)
        if not w.SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2):
            _log(f"SetProcessDpiAwarenessContext が失敗 (GetLastError={ct.get_last_error()})")
        frame = Frame(w)
    except Exception as e:  # noqa: BLE001 (理由を親に 1 行で渡す。後始末するものはまだ無い)
        return _fail(e)
    timers: list[int] = []
    try:
        try:
            frame.build()
        except FrameError as e:
            _say(f"error: {e}")
            return 1
        # GetLastError は各呼び出しの直後に読む
        retopmost = w.SetTimer(None, 0, RETOPMOST_MS, None)
        if not retopmost:
            _say(f"error: SetTimer (上げ直し) が失敗 (GetLastError={ct.get_last_error()})")
            return 1
        timers.append(retopmost)
        deadline = w.SetTimer(None, 0, min(max_seconds * 1000, USER_TIMER_MAXIMUM), None)
        if not deadline:
            _say(f"error: SetTimer (期限) が失敗 (GetLastError={ct.get_last_error()})")
            return 1
        timers.append(deadline)
        frame.show()
        _say("ready")
        # 窓を作ったのでこのスレッドのメッセージの列はある (PostThreadMessageW が届く)
        ui_tid = w.GetCurrentThreadId()
        threading.Thread(target=_watch_stdin, args=(w, ui_tid), daemon=True).start()
        msg = w.MSG()
        while True:
            r = w.GetMessageW(ct.byref(msg), None, 0, 0)
            if r == 0:  # WM_QUIT (標準入力の EOF)
                return 0
            if r == -1:
                _log(f"GetMessageW が失敗 (GetLastError={ct.get_last_error()})")
                return 0
            if not msg.hwnd and msg.message == WM_TIMER:
                if msg.wParam == retopmost:
                    frame.raise_topmost()
                elif msg.wParam == deadline:
                    _log(f"--max-seconds ({max_seconds} 秒) を過ぎたので終わる")
                    return 0
                continue
            w.TranslateMessage(ct.byref(msg))
            w.DispatchMessageW(ct.byref(msg))
    except KeyboardInterrupt:
        # 中断も finally の後始末を通してから、traceback なしで 1 で終える
        _log("中断 (KeyboardInterrupt) したので終わる")
        return 1
    except Exception as e:  # noqa: BLE001 (理由を親に 1 行で渡す)
        return _fail(e)
    finally:
        for t in timers:
            w.KillTimer(None, t)
        frame.close()


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        return run(args.max_seconds)
    except KeyboardInterrupt:
        # run の後始末 (finally) の外で届いた中断。作った物は run の finally か OS が片付ける
        _log("中断 (KeyboardInterrupt) したので終わる")
        return 1


if __name__ == "__main__":
    sys.exit(main())
