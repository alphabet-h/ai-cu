//! 操作の前の枠 (D-093)。画面を変える操作の前に、全モニターの縁に青い帯と、上の中央に案内の札を出す。
//!
//! 窓は専用の UI スレッド (`OverlayThread`) が持つ。モニターごとに 4 本の帯の窓と 1 枚の札の窓を作り、どれも
//! `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` の `WS_POPUP` で、
//! 題名なし、クラス名は `ai_cu_overlay` (他のプロセスの枠を探すのに使う)。中身は `UpdateLayeredWindow(ULW_ALPHA)` で
//! premultiplied BGRA を載せ、`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` で撮影から外す (付けられなければ
//! 枠を出さない)。表示は `SW_SHOWNOACTIVATE` で、前面を取らない。
//!
//! 見せる・隠すの判断は純関数の状態機械 `OverlayState` (期限と残す時間) が持つ。窓を見せ始めた時刻
//! (`GetTickCount64`) を窓のプロパティ `ai_cu_overlay_since` に置き、`overlay_shown_for_ms` が自分と他の
//! ai-cu のプロセスの枠の経過を読む (予告の間を省く判定。spec の「自分で決めたこと」)。
//! MCP の呼び出しで、見えている枠の残す時間を延ばす (`extend`。見えていなければ新しく出さない。D-107)。
//!
//! このモジュールは配線しない (実行器からの呼び出しは plan のステップ 5 と 6)。既定の `cargo test` は窓を作らない。

use std::cell::Cell;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE,
    DeleteDC, DeleteObject, DrawTextW, EnumDisplayMonitors, FW_SEMIBOLD, GdiFlush, GetDC,
    GetMonitorInfoW, HBITMAP, HDC, HFONT, HGDIOBJ, HMONITOR, MONITORINFO, OUT_DEFAULT_PRECIS,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW, GetMessageW,
    GetPropW, GetWindowThreadProcessId, HWND_TOPMOST, IsHungAppWindow, IsWindowVisible, KillTimer,
    MA_NOACTIVATE, MB_ICONASTERISK, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    RegisterClassExW, RemovePropW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetPropW, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WM_APP,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_TIMER, WM_USER,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::geometry::Rect;

/// 枠の窓のクラス名 (固定。他のプロセスの枠もこの名前で探す。D-093)
pub const OVERLAY_CLASS: &str = "ai_cu_overlay";
/// 札の文言 (固定。spec の「本人が決めたこと」)
pub const LABEL_TEXT: &str = "ai-cu が操作中 — マウスを動かすと止まる";
/// 見せ始めた時刻 (`GetTickCount64`) を置く窓のプロパティの名前
pub const SINCE_PROP: &str = "ai_cu_overlay_since";

/// 帯の太さ (96 DPI のピクセル。モニターの DPI で拡大する)
const BAND_THICKNESS_96: i32 = 6;
/// 札の文字の高さ (96 DPI のピクセル)
const LABEL_FONT_PX_96: i32 = 14;
/// 札の文字の周りの余白 (96 DPI のピクセル)
const LABEL_PAD_96: i32 = 8;
/// 札の書体 (無ければ GDI が代わりの書体を選ぶ)
const LABEL_FACE: &str = "Yu Gothic UI";
/// 枠の青 (`#2F7BFF` 相当)。RGB の順
const BLUE: (u8, u8, u8) = (0x2F, 0x7B, 0xFF);
/// 帯の外側 (画面の縁) の不透明度
const BAND_ALPHA_OUTER: u8 = 220;
/// 帯の内側の端の不透明度
const BAND_ALPHA_INNER: u8 = 40;
/// 札の地の不透明度
const LABEL_ALPHA: u8 = 230;
/// 最前面に上げ直す間隔 (ミリ秒)
const RETOPMOST_MS: u32 = 2000;
/// 起動を待つ長さ (窓を作り終えたことの知らせ)
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// `Drop` で終わりを待つ長さ (過ぎたらスレッドを切り離す)
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// 枠のスレッドへの知らせ (スレッドのメッセージ。窓に結びつかない)
const WM_OVERLAY_WAKE: u32 = WM_APP + 1;
const WM_OVERLAY_REBUILD: u32 = WM_APP + 2;
const WM_OVERLAY_QUIT: u32 = WM_APP + 3;

const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

// 見せ始めの時刻 (u64) を窓のプロパティ (HANDLE = ポインタの幅) にそのまま詰める前提。32 ビットでは切れるので止める
const _: () = assert!(usize::BITS == 64);

// ---- 純関数 (unsafe なし) ----

/// モニターの縁を覆う 4 本の帯 (上・下・左・右)。互いに重ならず、上と下は全幅、左と右は上下の帯の間を埋める。
/// 太さはモニターの幅と高さの半分で頭打ちにし、負なら 0 にする。
pub fn band_rects(monitor: Rect, thickness: i32) -> [Rect; 4] {
    let half = (monitor.w / 2).min(monitor.h / 2);
    let t = u32::try_from(thickness.max(0)).unwrap_or(0).min(half);
    let ti = t as i32;
    let inner_h = monitor.h - 2 * t;
    [
        Rect::new(monitor.x, monitor.y, monitor.w, t),
        Rect::new(
            monitor.x,
            (i64::from(monitor.y) + i64::from(monitor.h) - i64::from(t)) as i32,
            monitor.w,
            t,
        ),
        Rect::new(monitor.x, monitor.y + ti, t, inner_h),
        Rect::new(
            (i64::from(monitor.x) + i64::from(monitor.w) - i64::from(t)) as i32,
            monitor.y + ti,
            t,
            inner_h,
        ),
    ]
}

/// 札の矩形: モニターの上の中央で、上端から `thickness` 下 (上の帯のすぐ下)。大きさは文字の幅と高さに
/// 余白 `pad` を四方に足したもので、モニターの幅を超えない。
pub fn label_rect(monitor: Rect, text_w: i32, text_h: i32, pad: i32, thickness: i32) -> Rect {
    let w = (i64::from(text_w.max(0)) + 2 * i64::from(pad.max(0))).min(i64::from(monitor.w));
    let h = (i64::from(text_h.max(0)) + 2 * i64::from(pad.max(0))).min(i64::from(monitor.h));
    let x = i64::from(monitor.x) + (i64::from(monitor.w) - w) / 2;
    let y = i64::from(monitor.y) + i64::from(thickness.max(0));
    Rect::new(x as i32, y as i32, w as u32, h as u32)
}

/// 帯の不透明度。`depth` は画面の縁からの距離 (0 が最も外側)。外側が濃く、内側へ単調に薄くなる。
/// 太さが 1 以下なら外側の値。`depth` が範囲の外なら端の値にそろえる。
pub fn band_alpha(depth: i32, thickness: i32) -> u8 {
    if thickness <= 1 {
        return BAND_ALPHA_OUTER;
    }
    let d = i64::from(depth.clamp(0, thickness - 1));
    let span = i64::from(BAND_ALPHA_OUTER - BAND_ALPHA_INNER);
    let a = i64::from(BAND_ALPHA_OUTER) - span * d / i64::from(thickness - 1);
    a as u8
}

/// 96 DPI のピクセルをモニターの DPI に拡大する (四捨五入、1 以上)。
fn scale_px(px96: i32, dpi: u32) -> i32 {
    let v = (i64::from(px96) * i64::from(dpi) + 48) / 96;
    v.clamp(1, i64::from(i32::MAX)) as i32
}

/// `OverlayState::extend` に渡せる残す時間の上限 (ミリ秒)。延長は呼ぶたびにこの長さまでなので、
/// 主スレッドが固まっても最後の延長からこの時間のうちに枠は消える (D-107)
pub const EXTEND_MAX_MS: u64 = 60_000;

/// 見えている枠の見せ始めの時刻のうち、最も古いものからの経過 (ミリ秒)。無ければ None。
/// 見せ始めが `now` より新しく見えるもの (時計の読みの前後) は 0 として扱う (= 予告をまるごと待つ)。
pub fn oldest_elapsed(now: u64, sinces: &[u64]) -> Option<u64> {
    sinces.iter().map(|s| now.saturating_sub(*s)).max()
}

/// 枠を見せるかの状態機械 (期限と残す時間)。時刻はミリ秒 (実機では `GetTickCount64`)。
///
/// - `show(now, deadline_ms)`: 見せる。期限を `now + deadline_ms` に置き、前の `release` の終わりを取り消す
/// - `release(now, linger_ms)`: 見せる終わりを `min(期限, now + linger_ms)` に置く (見せていなければ何もしない)
/// - `extend(now, linger_ms)`: 見えていて解かれた枠の終わりを `now + linger_ms` まで延ばす (早めない)。
///   期限も同じだけ延ばす (`release_at ≤ deadline_at` を保ち、延長の後の `release` が期限で縮まないように)。
///   見えていないか、まだ解かれていない (実行中) なら何もしない。D-107
/// - `visible(now)`: 終わり (`release` の終わりか、無ければ期限) より前なら真
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayState {
    active: bool,
    deadline_at: u64,
    release_at: Option<u64>,
}

impl OverlayState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn show(&mut self, now_ms: u64, deadline_ms: u64) {
        self.active = true;
        self.deadline_at = now_ms.saturating_add(deadline_ms);
        self.release_at = None;
    }

    pub fn release(&mut self, now_ms: u64, linger_ms: u64) {
        if self.active {
            self.release_at = Some(self.deadline_at.min(now_ms.saturating_add(linger_ms)));
        }
    }

    pub fn extend(&mut self, now_ms: u64, linger_ms: u64) {
        // 主スレッドが固まれば最後の延長から linger_ms で消える保証は、linger_ms が短いことに頼る。
        // 呼び手は定数 (MCP_OVERLAY_LINGER_MS) だけ。設定の値を渡す変更が入ったら上限を決め直す (win32-reviewer)
        debug_assert!(
            linger_ms <= EXTEND_MAX_MS,
            "extend の linger_ms が上限を超えた: {linger_ms}"
        );
        // 見えていなければ新しく出さない (D-093 の範囲 A)。実行中 (release 前) の期限は実行器の計算のまま
        if !self.visible(now_ms) {
            return;
        }
        if let Some(end) = self.release_at {
            let until = now_ms.saturating_add(linger_ms);
            self.release_at = Some(end.max(until));
            // 消える時刻を決めるのは release_at。期限も延ばすのは release_at ≤ deadline_at を保つため
            // (延長の後に show を挟まずに release が来ても、期限で縮まない)
            self.deadline_at = self.deadline_at.max(until);
        }
    }

    fn end(&self) -> u64 {
        self.release_at.unwrap_or(self.deadline_at)
    }

    pub fn visible(&self, now_ms: u64) -> bool {
        self.active && now_ms < self.end()
    }

    /// 見せる終わりまでの残り (ミリ秒)。見えていなければ None。1 回きりのタイマーをこの長さで張り直す。
    pub fn next_check(&self, now_ms: u64) -> Option<u64> {
        self.visible(now_ms).then(|| self.end() - now_ms)
    }
}

// ---- 読み取り (どのプロセスの枠も) ----

/// クラス名が `ai_cu_overlay` のトップレベルの窓 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayWindow {
    pub hwnd: usize,
    pub pid: u32,
    pub visible: bool,
    /// 窓のプロパティ `ai_cu_overlay_since` (見せ始めの `GetTickCount64`)。無ければ None
    pub since_tick: Option<u64>,
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: buf は 64 個の u16 の書き込み可能な領域で、その要素数を渡す。関数はそれを超えて書かない。
    //         無効なハンドルなら 0 が返る。
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// クラス名が `ai_cu_overlay` のトップレベルの窓 (見えないものを含む)。`pid` が Some ならそのプロセスのものだけ。
/// 他のプロセスの窓のうち、応答しない (`IsHungAppWindow` が真) ものは数えない (固まった ai-cu の残した枠で
/// 予告の間を省き続けないため。D-093)。自分のプロセスの窓はそのまま数える。読み取りだけ。
pub fn overlay_windows(pid: Option<u32>) -> Vec<OverlayWindow> {
    let prop = super::wide(SINCE_PROP);
    let me = std::process::id();
    super::window::top_level_hwnds()
        .into_iter()
        .filter_map(|hwnd| {
            let h = hwnd as HWND;
            if class_name(h) != OVERLAY_CLASS {
                return None;
            }
            let mut owner = 0u32;
            // SAFETY: owner は書き込み可能な u32。無効なハンドルなら 0 が返り、owner は 0 のまま。
            unsafe { GetWindowThreadProcessId(h, &mut owner) };
            if owner == 0 || pid.is_some_and(|p| p != owner) {
                return None;
            }
            // SAFETY: ハンドルの値を渡すだけ。読み取りで、窓へメッセージを送らない。無効なハンドルなら 0 が返る。
            if owner != me && unsafe { IsHungAppWindow(h) } != 0 {
                return None;
            }
            // SAFETY: ハンドルの値を渡すだけ。無効なハンドルや見えない窓なら 0 が返る。
            let visible = unsafe { IsWindowVisible(h) } != 0;
            // SAFETY: prop は NUL 終端の UTF-16 で呼び出しの間生きている。プロパティが無いか無効なハンドルなら
            //         null が返る。返る値は窓が持つ整数 (見せ始めの時刻) で、ポインタとして参照しない。
            let raw = unsafe { GetPropW(h, prop.as_ptr()) } as usize;
            Some(OverlayWindow {
                hwnd,
                pid: owner,
                visible,
                since_tick: (raw != 0).then_some(raw as u64),
            })
        })
        .collect()
}

/// 見えている ai-cu の枠 (自分と他のプロセス) のうち、最も古いものが見え始めてからの経過 (ミリ秒)。無ければ None。
/// 窓を列挙した後に時計を読む (列挙の間に見せ始めた枠を「未来」にしないため。それでも新しく見えれば 0)。
pub fn overlay_shown_for_ms() -> Option<u64> {
    let sinces: Vec<u64> = overlay_windows(None)
        .into_iter()
        .filter(|w| w.visible)
        .filter_map(|w| w.since_tick)
        .collect();
    // SAFETY: 引数を取らず、起動からのミリ秒を返すだけ。
    let now = unsafe { GetTickCount64() };
    oldest_elapsed(now, &sinces)
}

/// 予告の音 (`MessageBeep(MB_ICONASTERISK)`。利用者の Windows の通知音の設定に従い、非同期に鳴る)。
pub fn beep() {
    // SAFETY: 音の種類の定数を渡すだけ。音を待ち行列に入れて戻る (Microsoft Learn の Remarks)。
    unsafe { MessageBeep(MB_ICONASTERISK) };
}

// ---- 窓 (枠のスレッドの中だけで触る) ----

thread_local! {
    /// 作り直しの知らせを、まだ処理していないものとして 1 つだけ積むための印 (帯のどの窓にも同じ通知が届くため)
    static REBUILD_PENDING: Cell<bool> = const { Cell::new(false) };
    /// 窓の組を作っている間 (`Windows::build`) は真。その間の `WM_DPICHANGED` は、作った窓を目的のモニターへ
    /// 置いたことによるもので、作り直しの理由にしない (DPI の違うモニターで作り直しが繰り返されないため)
    static BUILDING: Cell<bool> = const { Cell::new(false) };
}

/// `BUILDING` を立て、`Drop` で (途中の失敗でも) 下ろす
struct BuildingGuard;

impl BuildingGuard {
    fn enter() -> BuildingGuard {
        BUILDING.with(|b| b.set(true));
        BuildingGuard
    }
}

impl Drop for BuildingGuard {
    fn drop(&mut self) {
        BUILDING.with(|b| b.set(false));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // クリックされても前面にしない (WS_EX_TRANSPARENT で届かない想定だが、念のため)
        WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
        // 作っている間の DPI の変化は、窓を目的の矩形に置いたことによるもの。大きさは自分で決めているので無視する
        WM_DPICHANGED if BUILDING.with(|b| b.get()) => return 0,
        WM_NCDESTROY => {
            // 壊れる前に自分のプロパティを消す (SetPropW の文書の求め)。Windows の Drop でも消しているが、
            // それを通らずに壊れたとき (OS が壊したときなど) の守り。無ければ null が返るだけ。
            let prop = super::wide(SINCE_PROP);
            // SAFETY: hwnd はこのウィンドウプロシージャが受け取った、まだ有効な自分の窓。prop は NUL 終端の
            //         UTF-16 で呼び出しの間生きている。
            unsafe { RemovePropW(hwnd, prop.as_ptr()) };
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            if !REBUILD_PENDING.with(|p| p.replace(true)) {
                // SAFETY: ウィンドウプロシージャは窓を作ったスレッド (枠のスレッド) で呼ばれる。そのスレッドの
                //         キューに窓に結びつかない知らせを置くだけ。
                unsafe { PostThreadMessageW(GetCurrentThreadId(), WM_OVERLAY_REBUILD, 0, 0) };
            }
            return 0;
        }
        _ => {}
    }
    // SAFETY: ウィンドウプロシージャが受け取った引数をそのまま既定の処理に渡す。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 32bpp の上から下の DIB を選んだメモリ DC。`Drop` で選択を戻して消す。
struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Dib {
    fn new(w: i32, h: i32) -> Result<Dib, String> {
        if w <= 0 || h <= 0 {
            return Err(format!("DIB の大きさが空 ({w}x{h})"));
        }
        // SAFETY: null を渡すと画面と互換のメモリ DC が返る。失敗なら null。
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        if dc.is_null() {
            return Err("CreateCompatibleDC が失敗".into());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // 負の高さで上から下の並びにする
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: dc は上で作った有効な DC、info は 32bpp の w x h を正しく記述し、bits は書き込み先の
        //         ポインタ変数。区画 (hsection) は使わないので null と 0。
        let bmp = unsafe {
            CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bmp.is_null() || bits.is_null() {
            // SAFETY: 上で作った DC を 1 回だけ消す (まだ何も選んでいない)。
            unsafe { DeleteDC(dc) };
            return Err(format!(
                "CreateDIBSection が失敗 (GetLastError={})",
                super::last_error()
            ));
        }
        // SAFETY: dc と bmp は有効。戻る元のオブジェクトは Drop で選び直す。
        let old = unsafe { SelectObject(dc, bmp as HGDIOBJ) };
        Ok(Dib {
            dc,
            bmp,
            old,
            bits: bits.cast(),
            w,
            h,
        })
    }

    /// 画素 (BGRA、上から下)。GDI の描画を済ませてから読む (`GdiFlush`)。
    fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: 引数なし。このスレッドの GDI の溜まった描画を流すだけ。
        unsafe { GdiFlush() };
        let len = (self.w as usize) * (self.h as usize) * 4;
        // SAFETY: bits は CreateDIBSection が返した 32bpp x w x h (行の詰め物なし) の領域で、bmp を消すまで
        //         生きている。&mut self の借用の間、他から触られない (GDI の描画は上で流した)。
        unsafe { std::slice::from_raw_parts_mut(self.bits, len) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: new で選んだ元のオブジェクトを選び直し、作った DIB と DC を 1 回ずつ消す。
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp as HGDIOBJ);
            DeleteDC(self.dc);
        }
    }
}

/// premultiplied の BGRA の 1 画素
fn premultiplied(rgb: (u8, u8, u8), alpha: u8) -> [u8; 4] {
    let m = |c: u8| ((u16::from(c) * u16::from(alpha) + 127) / 255) as u8;
    [m(rgb.2), m(rgb.1), m(rgb.0), alpha]
}

/// 層付きの窓に DIB の中身を載せ、位置と大きさを `rect` にする。
fn present(hwnd: HWND, rect: Rect, dib: &Dib) -> Result<(), String> {
    // SAFETY: null を渡すと画面の DC が返る。下で同じ hwnd (null) で解放する。
    let screen = unsafe { GetDC(std::ptr::null_mut()) };
    let dst = POINT {
        x: rect.x,
        y: rect.y,
    };
    let size = SIZE {
        cx: dib.w,
        cy: dib.h,
    };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: hwnd はこのスレッドが作った WS_EX_LAYERED の窓。dib.dc は 32bpp の DIB を選んだ有効な DC で、
    //         size はその大きさ。POINT、SIZE、BLENDFUNCTION は呼び出しの間生きているローカル変数。
    //         screen は null でもよい (既定のパレット)。
    let ok = unsafe {
        UpdateLayeredWindow(
            hwnd, screen, &dst, &size, dib.dc, &src, 0, &blend, ULW_ALPHA,
        )
    };
    let err = super::last_error();
    if !screen.is_null() {
        // SAFETY: 上の GetDC(null) で得た DC を 1 回だけ解放する。
        unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
    }
    if ok == 0 {
        return Err(format!("UpdateLayeredWindow が失敗 (GetLastError={err})"));
    }
    Ok(())
}

/// 帯の 1 本 (`index` は 0 上、1 下、2 左、3 右) の中身を描く。
fn paint_band(hwnd: HWND, rect: Rect, index: usize, thickness: i32) -> Result<(), String> {
    let mut dib = Dib::new(rect.w as i32, rect.h as i32)?;
    let (w, h) = (dib.w, dib.h);
    let px = dib.pixels();
    for y in 0..h {
        for x in 0..w {
            let depth = match index {
                0 => y,
                1 => h - 1 - y,
                2 => x,
                _ => w - 1 - x,
            };
            let i = ((y as usize) * (w as usize) + x as usize) * 4;
            px[i..i + 4].copy_from_slice(&premultiplied(BLUE, band_alpha(depth, thickness)));
        }
    }
    present(hwnd, rect, &dib)
}

/// 札の書体 (`Drop` で消す)
struct Font(HFONT);

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: CreateFontW が返した書体を 1 回だけ消す。どの DC にも選んだままにしない (呼び出し側で戻す)。
        unsafe { DeleteObject(self.0 as HGDIOBJ) };
    }
}

fn label_font(dpi: u32) -> Result<Font, String> {
    let face = super::wide(LABEL_FACE);
    // SAFETY: 整数の引数と、呼び出しの間生きている NUL 終端の UTF-16 の書体名を渡すだけ。失敗なら null。
    let f = unsafe {
        CreateFontW(
            -scale_px(LABEL_FONT_PX_96, dpi),
            0,
            0,
            0,
            FW_SEMIBOLD as i32,
            0,
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(ANTIALIASED_QUALITY),
            u32::from(DEFAULT_PITCH),
            face.as_ptr(),
        )
    };
    if f.is_null() {
        return Err("CreateFontW が失敗".into());
    }
    Ok(Font(f))
}

/// 札の文字の大きさ (ピクセル)。
fn measure_label(font: &Font) -> Result<(i32, i32), String> {
    // SAFETY: null を渡すと画面と互換のメモリ DC が返る。下で消す。
    let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    if dc.is_null() {
        return Err("CreateCompatibleDC が失敗".into());
    }
    let text = super::wide(LABEL_TEXT);
    let mut r = RECT::default();
    // SAFETY: dc と font は有効。元の書体は下で選び直す。
    let old = unsafe { SelectObject(dc, font.0 as HGDIOBJ) };
    // SAFETY: text は NUL 終端の UTF-16 で、長さ -1 は NUL までを意味する。r は書き込み可能な RECT。
    //         DT_CALCRECT は描かずに大きさだけを r に書く。
    let h = unsafe {
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &mut r,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        )
    };
    // SAFETY: 元の書体を選び直し、上で作った DC を 1 回だけ消す。
    unsafe {
        SelectObject(dc, old);
        DeleteDC(dc);
    }
    if h == 0 {
        return Err("DrawTextW (大きさ) が失敗".into());
    }
    Ok((r.right - r.left, r.bottom - r.top))
}

/// 札の中身を描く: 黒地に白で文字を描いて明るさをマスクにし、青い地に白い文字を載せる。
fn paint_label(hwnd: HWND, rect: Rect, font: &Font, pad: i32) -> Result<(), String> {
    let mut dib = Dib::new(rect.w as i32, rect.h as i32)?;
    let text = super::wide(LABEL_TEXT);
    let mut r = RECT {
        left: pad,
        top: pad,
        right: dib.w - pad,
        bottom: dib.h - pad,
    };
    // 黒 (アルファ 0) の地にする。CreateDIBSection の領域は 0 で始まるが、それに頼らず明示に埋める
    dib.pixels().fill(0);
    // SAFETY: dib.dc は DIB を選んだ有効な DC、font は有効な書体 (元の書体は下で選び直す)。
    //         text は NUL 終端の UTF-16、r は書き込み可能な RECT。
    unsafe {
        let old = SelectObject(dib.dc, font.0 as HGDIOBJ);
        SetBkMode(dib.dc, TRANSPARENT as i32);
        SetTextColor(dib.dc, 0x00FF_FFFF);
        DrawTextW(
            dib.dc,
            text.as_ptr(),
            -1,
            &mut r,
            DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dib.dc, old);
    }
    for p in dib.pixels().as_chunks_mut::<4>().0 {
        // 文字の覆い (アンチエイリアスの明るさ)。ClearType でないので 3 つの値はほぼ等しい
        let c = u16::from(p[0].max(p[1]).max(p[2]));
        let mix = |b: u8| ((u16::from(b) * (255 - c) + 255 * c + 127) / 255) as u8;
        let rgb = (mix(BLUE.0), mix(BLUE.1), mix(BLUE.2));
        p.copy_from_slice(&premultiplied(rgb, LABEL_ALPHA));
    }
    present(hwnd, rect, &dib)
}

/// 1 つの枠の窓を `rect` の位置と大きさで作る (見せない)。WDA を付けられなければ壊して Err。
/// 最初から目的の矩形 (= 目的のモニター) に作るのは、仮の位置 (主モニター) から DPI の違うモニターへ動かして
/// `WM_DPICHANGED` を起こさないため (起きても `BUILDING` の間は無視する)。
fn create_overlay_window(class: &[u16], rect: Rect) -> Result<HWND, String> {
    // SAFETY: null を渡すと、この実行ファイルのモジュールのハンドルが返る。
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    let w = i32::try_from(rect.w).unwrap_or(i32::MAX);
    let h = i32::try_from(rect.h).unwrap_or(i32::MAX);
    // SAFETY: class は NUL 終端の UTF-16 で呼び出しの間生きている。題名・親・メニューは null。中身は
    //         UpdateLayeredWindow が後で載せる (同じ rect)。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            rect.x,
            rect.y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(format!(
            "CreateWindowExW が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    // SAFETY: hwnd はこのプロセスのトップレベルの窓で、定数を渡すだけ。
    if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } == 0 {
        let err = super::last_error();
        // SAFETY: hwnd はこのスレッドが作った窓で、作ったスレッドから 1 回だけ壊す。プロパティはまだ無い。
        unsafe { DestroyWindow(hwnd) };
        return Err(format!(
            "SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) が失敗 (GetLastError={err})"
        ));
    }
    Ok(hwnd)
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> windows_sys::core::BOOL {
    // SAFETY: lparam は monitors が渡した、生きている Vec<HMONITOR> への排他的なポインタ。
    //         EnumDisplayMonitors は同じスレッドで同期的にこの関数を呼ぶので、その間 Vec は他から触られない。
    let out = unsafe { &mut *(lparam as *mut Vec<HMONITOR>) };
    out.push(monitor);
    1
}

/// モニターの矩形 (物理ピクセル) と実効 DPI。
fn monitors() -> Result<Vec<(Rect, u32)>, String> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: DC と切り抜きは null (仮想画面の全モニター)。コールバックは上の collect_monitor で、lparam は
    //         呼び出しの間だけ生きている handles を指す。戻る前にすべての呼び出しを終える。
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut handles as *mut Vec<HMONITOR> as LPARAM,
        )
    };
    let mut out = Vec::new();
    for m in handles {
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: m は列挙で得たモニターのハンドル、mi は cbSize を正しく入れた書き込み可能な構造体。
        if unsafe { GetMonitorInfoW(m, &mut mi) } == 0 {
            continue;
        }
        let r = mi.rcMonitor;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            continue;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        // SAFETY: m は有効なモニターのハンドル、dx と dy は書き込み可能な u32。失敗なら 96 のまま使う。
        let hr = unsafe { GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
        let dpi = if hr >= 0 && dx > 0 { dx } else { 96 };
        out.push((Rect::new(r.left, r.top, w as u32, h as u32), dpi));
    }
    if out.is_empty() {
        return Err("モニターが見つからない (EnumDisplayMonitors)".into());
    }
    Ok(out)
}

/// 枠のスレッドが持つ窓の組
struct Windows {
    hwnds: Vec<HWND>,
}

impl Windows {
    /// 全モニターの帯と札を作って描く (見せない)。途中で失敗したら作った分を壊して Err。
    fn build(class: &[u16]) -> Result<Windows, String> {
        // made は guard より後に宣言したので先に落ちる (宣言の逆順)。途中の失敗で作った窓を壊す間も BUILDING は立ったまま
        let _guard = BuildingGuard::enter();
        let mut made = Windows { hwnds: Vec::new() };
        for (monitor, dpi) in monitors()? {
            let thickness = scale_px(BAND_THICKNESS_96, dpi);
            for (index, rect) in band_rects(monitor, thickness).into_iter().enumerate() {
                if rect.is_empty() {
                    continue;
                }
                let hwnd = create_overlay_window(class, rect)?;
                made.hwnds.push(hwnd);
                paint_band(hwnd, rect, index, thickness)?;
            }
            let font = label_font(dpi)?;
            let (tw, th) = measure_label(&font)?;
            let pad = scale_px(LABEL_PAD_96, dpi);
            let rect = label_rect(monitor, tw, th, pad, thickness);
            if !rect.is_empty() {
                let hwnd = create_overlay_window(class, rect)?;
                made.hwnds.push(hwnd);
                paint_label(hwnd, rect, &font, pad)?;
            }
        }
        Ok(made)
    }

    fn show(&self, since: u64) {
        let prop = super::wide(SINCE_PROP);
        for &h in &self.hwnds {
            // SAFETY: h はこのスレッドが作った窓。prop は NUL 終端の UTF-16 で呼び出しの間生きている。値は時刻の
            //         整数をハンドルの幅に入れたもの (64 ビットの usize に収まる) で、ポインタとして参照されない。
            //         隠すときと壊す前に RemovePropW で消す。
            unsafe { SetPropW(h, prop.as_ptr(), since.max(1) as usize as HANDLE) };
            // SAFETY: h はこのスレッドが作った窓。前面を取らない表示の定数を渡すだけ。
            unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
        }
        self.raise();
    }

    fn raise(&self) {
        for &h in &self.hwnds {
            // SAFETY: h はこのスレッドが作った窓。位置と大きさを変えず、前面を取らずに最前面の帯へ上げるだけ。
            unsafe {
                SetWindowPos(
                    h,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
    }

    fn hide(&self) {
        let prop = super::wide(SINCE_PROP);
        for &h in &self.hwnds {
            // SAFETY: h はこのスレッドが作った窓。隠す定数を渡すだけ。
            unsafe { ShowWindow(h, SW_HIDE) };
            // SAFETY: h はこのスレッドが作った窓で、自分が SetPropW で置いたプロパティだけを消す。無ければ null。
            unsafe { RemovePropW(h, prop.as_ptr()) };
        }
    }
}

impl Drop for Windows {
    fn drop(&mut self) {
        let prop = super::wide(SINCE_PROP);
        for &h in &self.hwnds {
            // SAFETY: h はこのスレッドが作った窓 (Windows は枠のスレッドの中だけで作られ、壊される)。壊す前に
            //         自分のプロパティを消し (SetPropW の文書の求め)、作ったスレッドから 1 回だけ壊す。
            unsafe {
                RemovePropW(h, prop.as_ptr());
                DestroyWindow(h);
            }
        }
    }
}

/// 枠のスレッドへの指示
enum Command {
    Show(u64),
    Release(u64),
    Extend(u64),
}

fn now_tick() -> u64 {
    // SAFETY: 引数を取らず、起動からのミリ秒を返すだけ。
    unsafe { GetTickCount64() }
}

/// 枠のスレッドの中の状態
struct Ui {
    class: Vec<u16>,
    windows: Option<Windows>,
    state: OverlayState,
    /// いま見せているなら、見せ始めの時刻
    shown_since: Option<u64>,
    check_timer: usize,
    errors: mpsc::Sender<String>,
    /// 窓の組を作れずにいる間の失敗の文 (成功で None)。`OverlayThread::build_error` と共有し、同じ失敗を
    /// 2 秒ごとの作り直しのたびに積まないための比較もこれを読む (値は 1 か所。D-103)
    build_error: Arc<Mutex<Option<String>>>,
}

impl Ui {
    /// 状態に合わせて見せる・隠す。見せる終わりへの 1 回きりのタイマーを張り直す。
    fn apply(&mut self, raise: bool) {
        let now = now_tick();
        let want = self.state.visible(now);
        if let Some(w) = &self.windows {
            match (want, self.shown_since) {
                (true, None) => {
                    w.show(now);
                    self.shown_since = Some(now);
                }
                (true, Some(_)) if raise => w.raise(),
                (false, Some(_)) => {
                    w.hide();
                    self.shown_since = None;
                }
                _ => {}
            }
        } else {
            // 窓が無ければ何も見せていない (作り直しの失敗の後。次に作れたときは、その時刻から見せ始める)
            self.shown_since = None;
        }
        self.rearm(now);
    }

    fn rearm(&mut self, now: u64) {
        if self.check_timer != 0 {
            // SAFETY: このスレッドが hwnd なしで張ったタイマーの ID を渡すだけ。既に無ければ失敗するだけ。
            unsafe { KillTimer(std::ptr::null_mut(), self.check_timer) };
            self.check_timer = 0;
        }
        if let Some(ms) = self.state.next_check(now) {
            let ms = u32::try_from(ms).unwrap_or(u32::MAX).max(1);
            // SAFETY: hwnd なしのスレッドのタイマー (ID は OS が選ぶ)。WM_TIMER はこのスレッドのキューに
            //         hwnd null で届き、下のループが処理する。コールバックは無い。
            self.check_timer = unsafe { SetTimer(std::ptr::null_mut(), 0, ms, None) };
        }
    }

    /// モニターの構成の変化で窓を作り直す。失敗は戻りの通り道に積む (このスレッドは標準エラーに書かない)。
    fn rebuild(&mut self) {
        REBUILD_PENDING.with(|p| p.set(false));
        let since = self.shown_since;
        self.windows = None;
        self.build_windows(since);
        self.apply(false);
    }

    /// 前の作り直しが失敗して窓が無いとき、もう一度作る (最前面の上げ直しのタイマーから 2 秒ごとに呼ぶ)。
    /// 窓が無いとこのスレッドに `WM_DISPLAYCHANGE` も届かないので、ここで作り直さないと枠が戻らない。
    fn retry_if_missing(&mut self) {
        if self.windows.is_none() {
            // 窓が無い間は何も見せていないので、見せ始めの時刻は引き継がない (apply がその時刻から見せる)
            self.build_windows(None);
        }
    }

    /// 窓の組を作って持つ。`since` があればその時刻から見せていたものとして見せる。失敗は戻りの通り道に
    /// 積む (このスレッドは標準エラーに書かない)。直前と同じ失敗は積まない。
    fn build_windows(&mut self, since: Option<u64>) {
        match Windows::build(&self.class) {
            Ok(w) => {
                if let Some(s) = since {
                    // 見せ始めの時刻は作り直しの前のものを引き継ぐ (予告を省く判定がずれないように)
                    w.show(s);
                }
                self.windows = Some(w);
                *lock_build_error(&self.build_error) = None;
            }
            Err(e) => {
                let mut last = lock_build_error(&self.build_error);
                if last.as_deref() != Some(e.as_str()) {
                    let _ = self.errors.send(format!("枠の作り直しに失敗: {e}"));
                }
                *last = Some(e);
            }
        }
    }
}

/// 作り直しの失敗の文のロックを取る。ロックが毒されていても (持ち手が panic した) 中身を読む
/// (値は文 1 つで、途中の書きかけは無い)
fn lock_build_error(m: &Mutex<Option<String>>) -> std::sync::MutexGuard<'_, Option<String>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn register_class(class: &[u16]) -> Result<(), String> {
    // SAFETY: null を渡すと、この実行ファイルのモジュールのハンドルが返る。
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    // SAFETY: wc は初期化済みで cbSize が正しく、class は登録の間生きている (クラス名は登録時に複製される)。
    if unsafe { RegisterClassExW(&wc) } == 0 {
        let err = super::last_error();
        if err != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("RegisterClassExW が失敗 (GetLastError={err})"));
        }
    }
    Ok(())
}

fn overlay_thread(
    commands: mpsc::Receiver<Command>,
    ready: mpsc::SyncSender<Result<u32, String>>,
    errors: mpsc::Sender<String>,
    build_error: Arc<Mutex<Option<String>>>,
    done: mpsc::Sender<()>,
) {
    let mut msg = MSG::default();
    // SAFETY: msg は書き込み可能な MSG。何も取り出さず (PM_NOREMOVE)、このスレッドのキューを作らせるだけ
    //         (PostThreadMessageW の文書の手順)。
    unsafe {
        PeekMessageW(
            &mut msg,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        )
    };
    // SAFETY: 引数を取らず、このスレッドの ID を返すだけ。
    let thread_id = unsafe { GetCurrentThreadId() };
    let fail = |e: String| {
        let _ = ready.send(Err(e));
        let _ = done.send(());
    };
    // 窓の座標と大きさは物理ピクセルで決めるので、このスレッドが Per-Monitor V2 であることを確かめる
    // (プロセスの宣言はスレッドごとに上書きできる)
    if !super::dpi::thread_is_per_monitor_v2() {
        fail("枠のスレッドの DPI 認識が Per-Monitor V2 になっていない".into());
        return;
    }
    let class = super::wide(OVERLAY_CLASS);
    let built = register_class(&class).and_then(|()| Windows::build(&class));
    let windows = match built {
        Ok(w) => w,
        Err(e) => {
            fail(e);
            return;
        }
    };
    // SAFETY: hwnd なしのスレッドのタイマー (ID は OS が選ぶ)。最前面の上げ直しに使う。コールバックは無い。
    let topmost_timer = unsafe { SetTimer(std::ptr::null_mut(), 0, RETOPMOST_MS, None) };
    if topmost_timer == 0 {
        let err = super::last_error();
        // 窓はこのスレッドで壊す (Windows の Drop)
        drop(windows);
        fail(format!(
            "最前面の上げ直しのタイマーを張れない (SetTimer、GetLastError={err})"
        ));
        return;
    }
    if ready.send(Ok(thread_id)).is_err() {
        // 呼び出し側が待つのをやめた (時間切れ)。窓は Drop で壊す
        // SAFETY: 上で張ったこのスレッドのタイマーを消す。
        unsafe { KillTimer(std::ptr::null_mut(), topmost_timer) };
        drop(windows);
        let _ = done.send(());
        return;
    }
    let mut ui = Ui {
        class,
        windows: Some(windows),
        state: OverlayState::new(),
        shown_since: None,
        check_timer: 0,
        errors,
        // 最初の窓の組は作れた (起動の成功) ので、共有の値は初期値の None のまま
        build_error,
    };
    loop {
        // SAFETY: msg は書き込み可能な MSG。このスレッドのすべての窓とスレッドのメッセージを受け取る。
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break;
        }
        if msg.hwnd.is_null() {
            match msg.message {
                WM_OVERLAY_WAKE => {
                    let mut raise = false;
                    while let Ok(cmd) = commands.try_recv() {
                        let now = now_tick();
                        match cmd {
                            Command::Show(deadline) => {
                                ui.state.show(now, deadline);
                                raise = true;
                            }
                            Command::Release(linger) => ui.state.release(now, linger),
                            // 最前面の上げ直しはしない。続く apply がタイマーを延びた終わりに張り直す
                            Command::Extend(linger) => ui.state.extend(now, linger),
                        }
                    }
                    ui.apply(raise);
                    continue;
                }
                WM_OVERLAY_REBUILD => {
                    ui.rebuild();
                    continue;
                }
                WM_OVERLAY_QUIT => break,
                WM_TIMER => {
                    // KillTimer の前にキューに入っていた古い WM_TIMER が後から届いても (ID が使い回されても)、
                    // apply は時計と状態から見せる・隠すを判定し直すだけなので害は無い
                    let id = msg.wParam;
                    if id == ui.check_timer {
                        ui.apply(false);
                    } else if id == topmost_timer {
                        ui.retry_if_missing();
                        ui.apply(true);
                    }
                    continue;
                }
                _ => {}
            }
        }
        // SAFETY: GetMessageW が埋めた有効な MSG を渡す。
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // SAFETY: このスレッドが張ったタイマーを消す (既に無ければ失敗するだけ)。
    unsafe {
        KillTimer(std::ptr::null_mut(), topmost_timer);
        KillTimer(std::ptr::null_mut(), ui.check_timer);
    }
    // 窓はこのスレッドで壊す (Windows の Drop)
    drop(ui);
    let _ = done.send(());
}

/// 枠を持つ専用の UI スレッド。`start` で窓を作り終えるまで待ち、以後は `show` と `release` で指示する。
/// `Drop` で終わりを知らせて待つ (1 秒で切り離す)。
pub struct OverlayThread {
    thread_id: u32,
    commands: mpsc::Sender<Command>,
    errors: mpsc::Receiver<String>,
    done: mpsc::Receiver<()>,
    handle: Option<JoinHandle<()>>,
    /// 窓の組を作れずにいる間の失敗の文 (枠のスレッドの `Ui` と共有。D-103)
    build_error: Arc<Mutex<Option<String>>>,
}

/// `OverlayThread::start` の失敗 (D-103)。`Display` は `message` をそのまま出す。
/// `detached` は、窓が作られるのを待つのが時間切れになり、join せずに切り離した枠のスレッドの handle
/// (呼び手がそのスレッドが終わったかを読めるように)。他の失敗ではスレッドは終わっているか立っていないので None
pub struct StartError {
    pub message: String,
    pub detached: Option<JoinHandle<()>>,
}

impl StartError {
    /// 切り離したスレッドの無い失敗
    fn plain(message: String) -> Self {
        Self {
            message,
            detached: None,
        }
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::fmt::Debug for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartError")
            .field("message", &self.message)
            .field("detached", &self.detached.is_some())
            .finish()
    }
}

impl std::error::Error for StartError {}

impl OverlayThread {
    /// スレッドを立て、キューを作り全モニターの枠の窓を作り終えるまで待つ (見せない)。窓を作れない、
    /// `WDA_EXCLUDEFROMCAPTURE` を付けられない、DPI の宣言ができないときは Err。待つのが時間切れなら、
    /// 切り離したスレッドの handle を `StartError::detached` に入れて返す (D-103)
    pub fn start() -> Result<OverlayThread, StartError> {
        super::dpi::ensure_per_monitor_v2().map_err(StartError::plain)?;
        let (cmd_tx, cmd_rx) = mpsc::channel();
        // 容量 0 (ランデブー): 受け手が去った後に作られた窓は、スレッドが自分で壊す
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let (err_tx, err_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let build_error = Arc::new(Mutex::new(None));
        let thread_build_error = Arc::clone(&build_error);
        let handle = std::thread::Builder::new()
            .name("ai-cu-overlay".into())
            .spawn(move || overlay_thread(cmd_rx, ready_tx, err_tx, thread_build_error, done_tx))
            .map_err(|e| StartError::plain(format!("枠のスレッドを立てられない: {e}")))?;
        let thread_id = match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(id)) => id,
            Ok(Err(e)) => {
                let _ = handle.join();
                return Err(StartError::plain(e));
            }
            Err(e @ mpsc::RecvTimeoutError::Timeout) => {
                // スレッドは窓を作る途中かもしれない。後で ready にたどり着けば自分で片付く (送り手が去っている)
                return Err(StartError {
                    message: format!("枠の窓が作られない: {e}"),
                    detached: Some(handle),
                });
            }
            // 送り手が去った: スレッドは終わりかけている (送り手は drop 済み)
            Err(e @ mpsc::RecvTimeoutError::Disconnected) => {
                return Err(StartError::plain(format!("枠の窓が作られない: {e}")));
            }
        };
        Ok(OverlayThread {
            thread_id,
            commands: cmd_tx,
            errors: err_rx,
            done: done_rx,
            handle: Some(handle),
            build_error,
        })
    }

    /// 枠のスレッドが窓の組を作れずにいる間 (モニターの構成の変化の後の作り直しの失敗)、その失敗の文。
    /// 窓の組を持っていれば None (D-103)
    pub fn build_error(&self) -> Option<String> {
        lock_build_error(&self.build_error).clone()
    }

    /// 枠のスレッドが終わったか (メッセージの輪が `GetMessageW` の -1 で抜けた、panic した)。
    /// handle が無ければ終わったと読む (D-103)
    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn send(&self, cmd: Command) {
        if self.commands.send(cmd).is_ok() {
            // SAFETY: start がキューを作り終えたことを確かめたスレッドの ID に、窓に結びつかない知らせを置くだけ。
            //         スレッドが既に終わっていれば失敗するだけ。
            unsafe { PostThreadMessageW(self.thread_id, WM_OVERLAY_WAKE, 0, 0) };
        }
    }

    /// 枠を見せる。`deadline_ms` 後には release が無くても自分で隠す (主スレッドが固まったときの守り。
    /// `extend` で期限が延びたら、その延びた時刻)。
    pub fn show(&self, deadline_ms: u64) {
        self.send(Command::Show(deadline_ms));
    }

    /// 枠を `linger_ms` 後に隠す (0 ならすぐ。期限より後にはしない)。その間の `show` で見せ続ける。
    pub fn release(&self, linger_ms: u64) {
        self.send(Command::Release(linger_ms));
    }

    /// 見えていて解かれた枠を `linger_ms` 後まで残す (見えていなければ何もしない。期限も延ばす。D-107)。
    pub fn extend(&self, linger_ms: u64) {
        self.send(Command::Extend(linger_ms));
    }

    /// モニターの構成の変化で作り直したときの失敗 (古いものから 1 つ)。無ければ None。
    pub fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}

impl Drop for OverlayThread {
    fn drop(&mut self) {
        // SAFETY: このスレッドの ID に、窓に結びつかない終わりの知らせを置くだけ。既に終わっていれば失敗するだけ。
        unsafe { PostThreadMessageW(self.thread_id, WM_OVERLAY_QUIT, 0, 0) };
        if self.done.recv_timeout(STOP_TIMEOUT).is_ok()
            && let Some(h) = self.handle.take()
        {
            let _ = h.join();
        }
        // 時間内に終わらなければ切り離す (プロセスの終わりで OS が窓ごと片付ける)。常駐する MCP のサーバでは
        // プロセスが長く生きるので、切り離したスレッドの窓もその間残りうる (期限の OverlayState で隠れるが、
        // スレッドが固まっていれば隠れない。他の ai-cu からは IsHungAppWindow で数えない)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(bands: &[Rect; 4], p: crate::geometry::Point) -> usize {
        bands.iter().filter(|b| b.contains(p)).count()
    }

    /// 起動の誤りの `Display` は文をそのまま出す (警告と `status` の error に使う。D-103)
    #[test]
    fn start_error_displays_message() {
        let e = StartError {
            message: "x".into(),
            detached: None,
        };
        assert_eq!(e.to_string(), "x");
    }

    #[test]
    fn band_rects_cover_edges_without_overlap() {
        for (monitor, t) in [
            (Rect::new(0, 0, 3840, 2160), 8),
            (Rect::new(-1920, 0, 1920, 1080), 6),
        ] {
            let bands = band_rects(monitor, t);
            for (i, a) in bands.iter().enumerate() {
                assert!(monitor.contains_rect(a), "{monitor:?} {a:?}");
                for b in &bands[i + 1..] {
                    assert!(!a.intersects(b), "{a:?} と {b:?} が重なる");
                }
            }
            let (l, r) = (monitor.x, monitor.x + monitor.w as i32 - 1);
            let (top, bottom) = (monitor.y, monitor.y + monitor.h as i32 - 1);
            // 縁の一周のすべての点がちょうど 1 本の帯に入る
            for x in l..=r {
                for y in [top, bottom] {
                    let p = crate::geometry::Point::new(x, y);
                    assert_eq!(covered(&bands, p), 1, "{p:?}");
                }
            }
            for y in top..=bottom {
                for x in [l, r] {
                    let p = crate::geometry::Point::new(x, y);
                    assert_eq!(covered(&bands, p), 1, "{p:?}");
                }
            }
            // 縁から t の内側の点は、どの帯にも入らない
            let inner = crate::geometry::Point::new(l + t, top + t);
            assert_eq!(covered(&bands, inner), 0);
            // 縁から t-1 の点は帯に入る
            let edge = crate::geometry::Point::new(l + t - 1, top + t);
            assert_eq!(covered(&bands, edge), 1);
            assert_eq!(bands[0].h, t as u32);
            assert_eq!(bands[2].w, t as u32);
        }
    }

    #[test]
    fn band_rects_clamp_thickness() {
        let bands = band_rects(Rect::new(0, 0, 10, 4), 100);
        assert_eq!(bands[0], Rect::new(0, 0, 10, 2));
        assert_eq!(bands[1], Rect::new(0, 2, 10, 2));
        assert!(bands[2].is_empty() && bands[3].is_empty());
        assert!(
            band_rects(Rect::new(0, 0, 10, 10), -3)
                .iter()
                .all(|b| b.is_empty())
        );
    }

    #[test]
    fn label_rect_is_top_center() {
        let m = Rect::new(0, 0, 3840, 2160);
        let r = label_rect(m, 400, 30, 10, 8);
        assert_eq!(r, Rect::new(1710, 8, 420, 50));
        assert_eq!(r.center().x, 1920);
        // 負の原点のモニターでも、そのモニターの上の中央
        let m2 = Rect::new(-1920, -200, 1920, 1080);
        let r2 = label_rect(m2, 400, 30, 10, 6);
        assert_eq!(r2, Rect::new(-1170, -194, 420, 50));
        assert!(m2.contains_rect(&r2));
        // 文字がモニターより広ければモニターの幅で切る
        let r3 = label_rect(Rect::new(0, 0, 300, 200), 400, 30, 10, 6);
        assert_eq!((r3.x, r3.w), (0, 300));
    }

    #[test]
    fn band_alpha_fades_inward() {
        for t in [2, 6, 8, 15, 30] {
            let a: Vec<u8> = (0..t).map(|d| band_alpha(d, t)).collect();
            assert_eq!(a[0], BAND_ALPHA_OUTER, "t={t}");
            assert_eq!(a[(t - 1) as usize], BAND_ALPHA_INNER, "t={t}");
            assert!(a.windows(2).all(|w| w[0] > w[1]), "t={t} {a:?}");
        }
        assert_eq!(band_alpha(0, 1), BAND_ALPHA_OUTER);
        assert_eq!(band_alpha(-5, 6), BAND_ALPHA_OUTER);
        assert_eq!(band_alpha(99, 6), BAND_ALPHA_INNER);
    }

    #[test]
    fn scale_px_follows_dpi() {
        assert_eq!(scale_px(6, 96), 6);
        assert_eq!(scale_px(6, 120), 8);
        assert_eq!(scale_px(6, 144), 9);
        assert_eq!(scale_px(14, 120), 18);
        assert_eq!(scale_px(1, 1), 1);
    }

    #[test]
    fn premultiplied_scales_color_by_alpha() {
        assert_eq!(
            premultiplied((0x2F, 0x7B, 0xFF), 255),
            [0xFF, 0x7B, 0x2F, 255]
        );
        assert_eq!(premultiplied((0x2F, 0x7B, 0xFF), 0), [0, 0, 0, 0]);
        let p = premultiplied((255, 255, 255), 128);
        assert_eq!(p, [128, 128, 128, 128]);
    }

    #[test]
    fn overlay_state_deadline_hides() {
        let mut s = OverlayState::new();
        assert!(!s.visible(0));
        s.show(0, 1000);
        assert!(s.visible(0));
        assert!(s.visible(999));
        assert!(!s.visible(1000));
    }

    #[test]
    fn overlay_state_linger() {
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 5000);
        assert!(s.visible(4999));
        assert!(!s.visible(5000));
        // 残す間の show で延びる
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 5000);
        s.show(4000, 600_000);
        assert!(s.visible(5000));
        assert!(s.visible(603_999));
        assert!(!s.visible(604_000));
        // 期限が短いときは期限で隠す
        let mut s = OverlayState::new();
        s.show(0, 1000);
        s.release(0, 5000);
        assert!(s.visible(999));
        assert!(!s.visible(1000));
        // release(0) はすぐ隠す
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(100, 0);
        assert!(!s.visible(100));
        // 見せていなければ release は何もしない
        let mut s = OverlayState::new();
        s.release(0, 5000);
        assert!(!s.visible(0));
    }

    #[test]
    fn overlay_state_extend() {
        // 解いた後の延長で終わりが延びる
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 5000);
        s.extend(4000, 30_000);
        assert!(s.visible(33_999));
        assert!(!s.visible(34_000));
        assert_eq!(s.next_check(4000), Some(30_000));
        // 早めない
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 50_000);
        s.extend(1000, 5000);
        assert!(s.visible(49_999));
        assert!(!s.visible(50_000));
        // 期限も早めない: 短い延長の後の release が、延長の終わり (6000) に縮んだ期限で切られない
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 50_000);
        s.extend(1000, 5000);
        s.release(2000, 30_000);
        assert!(s.visible(31_999));
        assert!(!s.visible(32_000));
        // 期限を越えて延ばす (期限 1000 で終わるはずだった枠)
        let mut s = OverlayState::new();
        s.show(0, 1000);
        s.release(0, 5000);
        s.extend(500, 30_000);
        assert!(s.visible(30_499));
        assert!(!s.visible(30_500));
        // 見えていなければ何もしない: 一度も見せていない
        let mut s = OverlayState::new();
        s.extend(0, 30_000);
        assert!(!s.visible(0));
        // release(0) の後
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(100, 0);
        s.extend(100, 30_000);
        assert!(!s.visible(100));
        // 残す時間が切れた後
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 5000);
        s.extend(5000, 30_000);
        assert!(!s.visible(5000));
        // 解く前 (実行中) は変えない
        let mut s = OverlayState::new();
        s.show(0, 1000);
        s.extend(500, 30_000);
        assert!(s.visible(999));
        assert!(!s.visible(1000));
        // 延長の後の release が期限で縮まない (期限を延ばしていなければ 1000 で消える)
        let mut s = OverlayState::new();
        s.show(0, 1000);
        s.release(0, 5000);
        s.extend(500, 30_000);
        s.release(600, 5000);
        assert!(s.visible(5599));
        assert!(!s.visible(5600));
    }

    #[test]
    fn overlay_state_next_check() {
        let mut s = OverlayState::new();
        assert_eq!(s.next_check(0), None);
        s.show(0, 1000);
        assert_eq!(s.next_check(400), Some(600));
        assert_eq!(s.next_check(1000), None);
        let mut s = OverlayState::new();
        s.show(0, 600_000);
        s.release(0, 5000);
        assert_eq!(s.next_check(0), Some(5000));
        assert_eq!(s.next_check(5000), None);
    }

    #[test]
    fn oldest_elapsed_picks_oldest() {
        assert_eq!(oldest_elapsed(1500, &[]), None);
        assert_eq!(oldest_elapsed(1500, &[1000, 400]), Some(1100));
        assert_eq!(oldest_elapsed(1500, &[1600]), Some(0));
    }

    /// guard を作った後に Err で早く戻る (`Windows::build` の途中の失敗と同じ形)
    fn fail_while_building(seen: &Cell<bool>) -> Result<(), String> {
        let _guard = BuildingGuard::enter();
        seen.set(BUILDING.with(|b| b.get()));
        Err("途中の失敗".into())
    }

    #[test]
    fn building_flag_drops_after_early_error() {
        // 印が立ったままだと、その後の本物の DPI の変化 (WM_DPICHANGED) を永久に無視してしまう
        assert!(!BUILDING.with(|b| b.get()));
        let seen = Cell::new(false);
        assert!(fail_while_building(&seen).is_err());
        assert!(seen.get(), "作っている間は BUILDING が立つ");
        assert!(!BUILDING.with(|b| b.get()), "戻った後は BUILDING が下りる");
    }

    #[test]
    fn overlay_constants_are_fixed() {
        assert_eq!(OVERLAY_CLASS, "ai_cu_overlay");
        assert_eq!(SINCE_PROP, "ai_cu_overlay_since");
        assert_eq!(LABEL_TEXT, "ai-cu が操作中 — マウスを動かすと止まる");
    }
}
