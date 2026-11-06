//! ウィンドウとプロセスの情報 (フォアグラウンドガードの判定材料)。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::GetWindowRgnBox;
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GA_ROOTOWNER, GWL_EXSTYLE, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetLayeredWindowAttributes, GetWindowDisplayAffinity, GetWindowLongW,
    GetWindowRect, GetWindowThreadProcessId, HTCAPTION, InternalGetWindowText, IsHungAppWindow,
    IsIconic, IsWindow, IsWindowVisible, LWA_ALPHA, LWA_COLORKEY, PostMessageW, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, SetForegroundWindow, WDA_EXCLUDEFROMCAPTURE, WM_CLOSE, WM_NCHITTEST,
    WS_EX_LAYERED, WS_EX_LAYOUTRTL, WS_EX_TOOLWINDOW, WindowFromPoint,
};

use crate::device::{
    HiddenWindow, StackWindow, TopWindow, WindowInfo, WindowOwner, WindowPresence,
};
use crate::geometry::{Point, Rect};

/// 閉じ忘れを防ぐハンドルの持ち主。
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: OpenProcess / OpenProcessToken が返した有効なハンドルで、ここで 1 回だけ閉じる。
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn hwnd_value(h: HWND) -> usize {
    h as usize
}

fn root_of(h: HWND) -> HWND {
    // SAFETY: GetAncestor は無効なハンドルに対しては null を返すだけで、メモリを参照しない。
    let root = unsafe { GetAncestor(h, GA_ROOT) };
    if root.is_null() { h } else { root }
}

/// 前面のウィンドウ (ルート)。無ければ None。
pub fn foreground() -> Option<WindowInfo> {
    // SAFETY: 引数なし。前面のウィンドウのハンドル (無ければ null) を返すだけ。
    let h = unsafe { GetForegroundWindow() };
    if h.is_null() {
        None
    } else {
        Some(info(root_of(h)))
    }
}

pub fn foreground_hwnd() -> usize {
    // SAFETY: 引数なし。前面のウィンドウのハンドル (無ければ null) を返すだけ。
    let h = unsafe { GetForegroundWindow() };
    if h.is_null() {
        0
    } else {
        hwnd_value(root_of(h))
    }
}

/// 座標の下のウィンドウのルート。
pub fn root_at(p: Point) -> usize {
    // SAFETY: POINT を値で渡し、ハンドル (無ければ null) を受け取るだけ。
    let h = unsafe { WindowFromPoint(POINT { x: p.x, y: p.y }) };
    if h.is_null() {
        0
    } else {
        hwnd_value(root_of(h))
    }
}

pub fn at_point(p: Point) -> Option<WindowInfo> {
    // SAFETY: POINT を値で渡し、ハンドル (無ければ null) を受け取るだけ。
    let h = unsafe { WindowFromPoint(POINT { x: p.x, y: p.y }) };
    if h.is_null() {
        None
    } else {
        Some(info(root_of(h)))
    }
}

fn read_wide(f: impl FnOnce(*mut u16, i32) -> i32, cap: usize) -> String {
    let mut buf = vec![0u16; cap];
    let n = f(buf.as_mut_ptr(), cap as i32);
    let n = usize::try_from(n).unwrap_or(0).min(cap);
    String::from_utf16_lossy(&buf[..n])
}

/// 窓の題名 (512 単位まで)。題名の読みはここ 1 か所にまとめ、`InternalGetWindowText` で読む。
/// `GetWindowTextW` / `GetWindowTextLengthW` は使わない: 自プロセスの窓には時間切れなしの WM_GETTEXT /
/// WM_GETTEXTLENGTH を送り、相手のスレッドがメッセージを汲まなければ止まる (Microsoft Learn の GetWindowText の
/// Remarks)。ai-cu は画面の縁の枠のスレッドの窓 (D-093) と各スレッドの IME の窓を持つ。他プロセスの窓では
/// 結果が同じ (D-099、D-105)。Learn の Remarks は他プロセスの帯 (caption) の無い窓に `GetWindowTextW` が空を返すと
/// 書くが、この PC では帯の無い題名つきの窓を含めて食い違いが無かった (D-105 の追記)。
fn window_title(h: HWND) -> String {
    // SAFETY: バッファの先頭と、その要素数 (cap) を渡す。InternalGetWindowText は cchmaxcount を超えて
    // 書かず、メッセージを送らない (窓の構造体から読む)。無効なハンドルなら 0 が返る。
    read_wide(|p, n| unsafe { InternalGetWindowText(h, p, n) }, 512)
}

fn info(h: HWND) -> WindowInfo {
    let mut w = info_untitled(h);
    w.title = window_title(h);
    w
}

/// 題名を読まない `info` (pid、クラス、実行ファイル名、管理者権限か。`title` は空)。撮影の判定 (D-077) に使う。
fn info_untitled(h: HWND) -> WindowInfo {
    let mut pid = 0u32;
    // SAFETY: pid は書き込み可能な u32。ハンドルが無効なら 0 が返り、pid は 0 のまま。
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    // SAFETY: バッファの先頭と、その要素数 (cap) を渡す。関数は cap を超えて書かない。
    let class = read_wide(|p, n| unsafe { GetClassNameW(h, p, n) }, 256);
    let (process, elevated) = process_info(pid);
    WindowInfo {
        hwnd: hwnd_value(h),
        pid,
        process,
        class,
        title: String::new(),
        elevated,
    }
}

fn token_elevated(process: HANDLE) -> Option<bool> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: process は有効なプロセスのハンドル (または現在のプロセスの擬似ハンドル)。token は書き込み先。
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    if ok == 0 || token.is_null() {
        return None;
    }
    let token = OwnedHandle(token);
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut ret = 0u32;
    // SAFETY: token は TOKEN_QUERY で開いた有効なハンドル。出力先は TOKEN_ELEVATION の大きさの領域で、その大きさを渡す。
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        )
    };
    if ok == 0 {
        None
    } else {
        Some(elevation.TokenIsElevated != 0)
    }
}

/// プロセスを PROCESS_QUERY_LIMITED_INFORMATION で開く。pid が 0 か開けなければ None。
fn open_limited(pid: u32) -> Option<OwnedHandle> {
    if pid == 0 {
        return None;
    }
    // SAFETY: 権限の定数と pid を渡すだけ。失敗すれば null が返る。
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        None
    } else {
        Some(OwnedHandle(h))
    }
}

/// 開いたプロセスの実行ファイル名。読めなければ空。
fn image_name(h: &OwnedHandle) -> String {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: h は PROCESS_QUERY_LIMITED_INFORMATION で開いた有効なハンドル。buf は len 要素の書き込み可能な領域で、
    // len は入力で容量、出力で書いた文字数 (NUL を除く) になる。
    let ok =
        unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return String::new();
    }
    let path = OsString::from_wide(&buf[..(len as usize).min(buf.len())]);
    std::path::Path::new(&path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 実行ファイル名だけ (管理者権限かは読まない)。開けないプロセスは空。
fn process_name(pid: u32) -> String {
    open_limited(pid)
        .map(|h| image_name(&h))
        .unwrap_or_default()
}

/// (実行ファイル名, 管理者権限か)。開けないプロセスは ("", None)。開くのは 1 回で、名前と権限を同じハンドルから読む。
fn process_info(pid: u32) -> (String, Option<bool>) {
    match open_limited(pid) {
        None => (String::new(), None),
        Some(h) => (image_name(&h), token_elevated(h.0)),
    }
}

unsafe extern "system" fn collect_hwnd(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: lparam は top_level_hwnds が渡した、生きている Vec<usize> への排他的なポインタ。
    // EnumWindows は同じスレッドで同期的にこの関数を呼ぶので、その間 Vec は他から触られない。
    let out = unsafe { &mut *(lparam as *mut Vec<usize>) };
    out.push(hwnd as usize);
    1
}

/// 見えていて題名のあるトップレベルのウィンドウ (ツールウィンドウを除く)。TUI の候補の一覧に使う (読み取りだけ)。
pub fn top_level_windows() -> Vec<WindowInfo> {
    top_level_windows_detailed()
        .into_iter()
        .map(|w| w.info)
        .collect()
}

/// 別の仮想デスクトップにあるなど、DWM が隠している (cloaked) ウィンドウか。
fn cloaked(h: HWND) -> bool {
    let mut value: u32 = 0;
    // SAFETY: 出力先は u32 の領域で、その大きさを渡す。DWMWA_CLOAKED は DWORD を書く。失敗したら 0 のまま。
    let hr = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED as u32,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr >= 0 && value != 0
}

/// 「見えている」の判定の元 (D-099。純関数)。`visible` は `IsWindowVisible`、`cloaked` は DWM の cloaked。
/// 見えていて cloaked でなければ `None` (見えている。`hidden_windows` に入れない)。それ以外は `Some(c)` で、
/// `c` は見えるが cloaked (別の仮想デスクトップなど) のときだけ真。見えず cloaked の窓は `Some(false)` (hidden)
pub fn hidden_state(visible: bool, cloaked: bool) -> Option<bool> {
    if visible && !cloaked {
        None
    } else {
        Some(visible && cloaked)
    }
}

/// 窓の見え方を読んで `hidden_state` に渡す (D-099)。`shown` と `hidden_top_windows` はこの読み 1 つを使い、
/// 2 つの一覧 (見えている窓と `hidden_windows`) が重ならず漏れないようにする。
fn visibility(h: HWND) -> Option<bool> {
    // SAFETY: ハンドルの値を渡すだけ。無効なハンドルや見えないウィンドウなら 0 が返り、メモリは参照しない。
    let visible = unsafe { IsWindowVisible(h) } != 0;
    hidden_state(visible, cloaked(h))
}

/// 見えているトップレベルの窓か (`IsWindowVisible` が真で cloaked でない)。読みは `visibility` 1 つ (D-099)
fn shown(h: HWND) -> bool {
    visibility(h).is_none()
}

/// 見えている枠の矩形 (物理ピクセル)。DWM の枠 (影を除く) が取れなければ GetWindowRect。
fn frame_rect(h: HWND) -> Option<Rect> {
    frame_rect_with_source(h).map(|(r, _)| r)
}

/// `frame_rect` と、DWM の枠が取れたか (偽なら `GetWindowRect` の予備で、見えない縁の分だけ大きい)。
fn frame_rect_with_source(h: HWND) -> Option<(Rect, bool)> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: 出力先は RECT の領域で、その大きさを渡す。DWMWA_EXTENDED_FRAME_BOUNDS は RECT を書く。
    let hr = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut r as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if hr < 0 {
        // SAFETY: 書き込み可能な RECT を渡すだけ。失敗したら 0 が返る。
        if unsafe { GetWindowRect(h, &mut r) } == 0 {
            return None;
        }
    }
    let (w, ht) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || ht <= 0 {
        return None;
    }
    Some((Rect::new(r.left, r.top, w as u32, ht as u32), hr >= 0))
}

/// その点が、ウィンドウの題名の帯 (ドラッグで動かせる部分。ボタンやタブではない) か。
/// 相手のウィンドウに `WM_NCHITTEST` で問い合わせる。応答しないアプリで固まらないよう、短い待ち時間で打ち切る。
/// 問い合わせだけで、入力も状態の変更も起こさない。
pub fn is_caption(hwnd: usize, p: Point) -> bool {
    // MAKELPARAM(x, y): 下位 16 ビットに x、上位 16 ビットに y (どちらも符号付き 16 ビットとして解釈される)
    let lparam = ((u32::from(p.y as u16) << 16) | u32::from(p.x as u16)) as LPARAM;
    let mut result: usize = 0;
    // SAFETY: hwnd は列挙で得たウィンドウのハンドルの値 (無効なら失敗して 0 が返るだけ)。result は書き込み可能な usize。
    // SMTO_ABORTIFHUNG と 100 ms の待ちで、相手が応答しなくても戻る。
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd as HWND,
            WM_NCHITTEST,
            0,
            lparam,
            SMTO_ABORTIFHUNG,
            100,
            &mut result,
        )
    };
    ok != 0 && result as u32 == HTCAPTION
}

/// 1 つのウィンドウの情報、枠の矩形、最小化の状態 (絞り込みはしない)。枠の矩形が取れなければ None。
pub fn describe(hwnd: usize) -> Option<TopWindow> {
    let h = hwnd as HWND;
    // SAFETY: ハンドルの値を渡すだけ。無効なハンドルや最小化していないウィンドウなら 0 が返る。
    let minimized = unsafe { IsIconic(h) } != 0;
    // 最小化したウィンドウの矩形は画面の外 (-32000 など) なので出さない
    let rect = if minimized {
        None
    } else {
        Some(frame_rect(h)?)
    };
    Some(TopWindow {
        info: info(h),
        rect,
        minimized,
    })
}

/// トップレベルのウィンドウのハンドルすべて (`EnumWindows`。見えるか、ツールウィンドウか、cloaked かで絞らない)。
/// Z 順 (前面から奥へ)。読み取りだけ (D-074)。戻り値の成否は見ない (途中で止まっても、集めた分を返す)。
pub fn top_level_hwnds() -> Vec<usize> {
    let mut hwnds: Vec<usize> = Vec::new();
    // SAFETY: コールバックは上の collect_hwnd で、lparam は呼び出しの間だけ生きている hwnds を指す。
    // EnumWindows は戻る前にすべての呼び出しを終える。
    unsafe { EnumWindows(Some(collect_hwnd), &mut hwnds as *mut Vec<usize> as LPARAM) };
    hwnds
}

/// 見えていて題名のあるトップレベルのウィンドウ (ツールウィンドウと cloaked を除く)。Z 順 (前面から奥へ)。
/// 読み取りだけで、入力も撮影もしない。
pub fn top_level_windows_detailed() -> Vec<TopWindow> {
    top_level_hwnds()
        .into_iter()
        .map(|v| v as HWND)
        .filter(|&h| {
            // 見えていて cloaked でない (判定の元は `hidden_state`。D-099)
            let visible = shown(h);
            // SAFETY: 列挙で得たハンドルを渡すだけ。GWL_EXSTYLE の読み取りだけで、既に無いウィンドウなら 0 が返る。
            let ex = unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32;
            // ツールウィンドウかを見る前に全部の窓の題名を読むのはわざと (短絡させない): 自プロセスの
            // ツールウィンドウにもメッセージを送らない読みを、実機テスト
            // `fixture_real_window_reads_do_not_wait_on_stalled_own_window` で撃つため (D-105)
            let titled = !window_title(h).is_empty();
            visible && titled && ex & WS_EX_TOOLWINDOW == 0
        })
        .filter_map(|h| describe(h as usize))
        .collect()
}

/// 窓が下の窓を透かして写しうるか (D-077)。`ex` は拡張スタイル、`layered` は `GetLayeredWindowAttributes` の
/// 結果 (フラグと alpha。呼び出しが失敗したら `None`)。`WS_EX_LAYERED` が無ければ偽 (不透明)。あれば、失敗
/// (`UpdateLayeredWindow` のピクセルごとのアルファを含む)、`LWA_COLORKEY`、255 未満の `LWA_ALPHA` のどれかで真。
/// `WS_EX_TRANSPARENT` はヒットテスト (マウスの素通し) にだけ効くので見ない。真なら覆いとみなさない (断る向き)
pub fn see_through(ex: u32, layered: Option<(u32, u8)>) -> bool {
    if ex & WS_EX_LAYERED == 0 {
        return false;
    }
    match layered {
        None => true,
        Some((flags, alpha)) => {
            flags & LWA_COLORKEY != 0 || (flags & LWA_ALPHA != 0 && alpha < 255)
        }
    }
}

/// 窓のリージョン (`GetWindowRgnBox` の結果。D-112)。`NoRegion` は ERROR (リージョンが無い、または読めない)、
/// `Empty` は NULLREGION、`Simple` は SIMPLEREGION で、矩形は窓の左上 (`GetWindowRect` の左上。DWM の枠ではない)
/// を原点にした座標。`Complex` は COMPLEXREGION と、分類できない値 (範囲外の戻り値、大きさ 0 以下の矩形)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRegion {
    NoRegion,
    Empty,
    Simple(Rect),
    Complex,
}

/// `GetWindowRgnBox` の戻り値 `code` と RECT の 4 値を分類する (D-112)。0 (ERROR) はリージョンが無いときと
/// 読み取りの失敗を区別できないので、リージョンが無いとみなす (`NoRegion`。D-112)。1 は `Empty`、2 は幅と高さが
/// どちらも 1 以上なら `Simple`、でなければ `Complex`。3 と範囲外の値は `Complex` (断る向き)。幅と高さは i64 で
/// 引く (i32 の端の値で panic しない。2 つの i32 の正の差は u32 に必ず収まるが、変換は守りとして `try_from` で書く)
pub fn classify_region(code: i32, left: i32, top: i32, right: i32, bottom: i32) -> WindowRegion {
    match code {
        0 => WindowRegion::NoRegion,
        1 => WindowRegion::Empty,
        2 => {
            let w = i64::from(right) - i64::from(left);
            let h = i64::from(bottom) - i64::from(top);
            match (u32::try_from(w).ok(), u32::try_from(h).ok()) {
                (Some(w), Some(h)) if w >= 1 && h >= 1 => {
                    WindowRegion::Simple(Rect::new(left, top, w, h))
                }
                _ => WindowRegion::Complex,
            }
        }
        _ => WindowRegion::Complex,
    }
}

/// リージョンが枠 `frame` (画面の物理ピクセル) をまるごと覆うか (覆いに数えてよいか。D-112)。`NoRegion` は真、
/// `Empty` と `Complex` は偽。`Simple` は、窓の原点 `window_origin` (`GetWindowRect` の左上) が読めて、リージョンの
/// 矩形を画面に直した矩形 (足し算は checked。溢れたら偽) が枠を含むときだけ真。原点が読めなければ偽 (断る向き)
pub fn region_covers_frame(
    frame: Rect,
    window_origin: Option<(i32, i32)>,
    region: WindowRegion,
) -> bool {
    match region {
        WindowRegion::NoRegion => true,
        WindowRegion::Empty | WindowRegion::Complex => false,
        WindowRegion::Simple(r) => {
            let Some((ox, oy)) = window_origin else {
                return false;
            };
            match (ox.checked_add(r.x), oy.checked_add(r.y)) {
                (Some(x), Some(y)) => Rect::new(x, y, r.w, r.h).contains_rect(&frame),
                _ => false,
            }
        }
    }
}

/// リージョンの矩形を画面に直すときの原点 (`region_covers_frame` の `window_origin`。D-112)。`window_rect_origin` は
/// `GetWindowRect` の左上 (読めなければ `None`)。右から左のレイアウト (`WS_EX_LAYOUTRTL`) の窓は、SetWindowRgn の文書の
/// Note ではリージョンの座標が窓の右上を原点にする (GetWindowRgnBox の文書は RTL に触れない) ので、左上で直すと位置を取り違え、一部だけのリージョンが枠を含むように見えうる。
/// 測っていないので `None` を返し、`Simple` のリージョンなら覆いに数えない (断る向き)
pub fn region_origin(ex: u32, window_rect_origin: Option<(i32, i32)>) -> Option<(i32, i32)> {
    if ex & WS_EX_LAYOUTRTL != 0 {
        return None;
    }
    window_rect_origin
}

/// 覆いとみなさない窓か (D-077。`stack_window` の `see_through`)。層の属性で透けうる (`see_through`)、撮影から
/// 外れる (`affinity` が `WDA_EXCLUDEFROMCAPTURE`。撮った画像に写らないので奥が写る)、DWM の枠が取れず
/// `GetWindowRect` の予備の矩形 (見えない縁の分だけ大きい) しか無い (`exact_frame` が偽)、リージョンが枠を
/// まるごと覆わない (`region_full` が偽。`region_covers_frame` の答え。D-112) のどれかで真。
/// `affinity` は `GetWindowDisplayAffinity` の結果で、失敗したら `None` (撮影から外れる設定は無いとみなす)。
/// 文書は「層付きの窓でだけ成功する」と書くが、この PC (Windows 11) では層付きでない窓でも成功して値を返した
/// (`fixture_real_capture_excluded_fixture_is_not_a_cover`)。失敗は閉じられた窓などに限ると見て、外れる側には
/// 倒さない (倒すと、失敗する環境では普通の窓まで覆いでなくなり撮影がほぼ断られる)。真は断る向き
pub fn not_a_cover(
    ex: u32,
    layered: Option<(u32, u8)>,
    affinity: Option<u32>,
    exact_frame: bool,
    region_full: bool,
) -> bool {
    see_through(ex, layered)
        || affinity == Some(WDA_EXCLUDEFROMCAPTURE)
        || !exact_frame
        || !region_full
}

/// `GetWindowDisplayAffinity` の値。失敗したら `None`。読み取りだけ。
pub fn display_affinity(hwnd: usize) -> Option<u32> {
    let h = hwnd as HWND;
    let mut affinity = 0u32;
    // SAFETY: affinity は書き込み可能なローカルの u32。どのプロセスの窓にも使える読み取りで、無効なハンドルでは
    // 0 が返り、そのときは affinity を使わない。
    let ok = unsafe { GetWindowDisplayAffinity(h, &mut affinity) };
    (ok != 0).then_some(affinity)
}

/// 層付きの窓の `GetLayeredWindowAttributes` (フラグと alpha)。失敗したら `None`。
fn layered_attributes(h: HWND) -> Option<(u32, u8)> {
    let mut alpha = 0u8;
    let mut flags = 0u32;
    // SAFETY: alpha と flags は書き込み可能なローカル変数。色キーは要らないので NULL を渡す (省略可能な引数)。
    // 無効なハンドルや UpdateLayeredWindow の窓では 0 が返り、そのときは alpha と flags を使わない。
    let ok = unsafe { GetLayeredWindowAttributes(h, std::ptr::null_mut(), &mut alpha, &mut flags) };
    (ok != 0).then_some((flags, alpha))
}

/// 窓のリージョン (`GetWindowRgnBox` を `classify_region` で分類したもの) と、リージョンの矩形の原点 (D-112)。
/// 原点は `Simple` のときだけ読み、それ以外は `None` (`region_covers_frame` は `Simple` 以外で原点を見ない)。
/// 原点は必ず `GetWindowRect` の左上 (DWM の枠ではない): 文書のとおりリージョンの座標は窓の矩形の左上が原点で、
/// 見えない縁のある窓では DWM の枠の左上と違う。`GetWindowRect` が失敗したら `None` (覆いに数えない)。
/// 右から左のレイアウト (`WS_EX_LAYOUTRTL`) の窓の扱いは純関数の `region_origin` が決める (`ex` は呼ぶ側が読んだ
/// 拡張スタイルで、ここでは渡すだけ)。読み取りだけ。
fn window_region(h: HWND, ex: u32) -> (WindowRegion, Option<(i32, i32)>) {
    let mut b = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: b は書き込み可能なローカルの RECT。どのプロセスの窓にも使える読み取りで、リージョンが無いときや
    // 無効なハンドルでは ERROR (0) が返り、そのときは b を使わない (`classify_region` は 0 で矩形を見ない)。
    let code = unsafe { GetWindowRgnBox(h, &mut b) };
    let region = classify_region(code, b.left, b.top, b.right, b.bottom);
    if !matches!(region, WindowRegion::Simple(_)) {
        return (region, None);
    }
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: r は書き込み可能なローカルの RECT。無効なハンドルなら 0 が返り、そのときは r を使わない。
    // 原点は `GetWindowRect` (窓の矩形) の左上で、DWM の枠 (`frame_rect_with_source`) ではない (リージョンの原点)。
    // 枠と原点は別の時点の読みで、その間に窓が動く競合は防がない (D-077 の列挙と同じ限界)。
    let ok = unsafe { GetWindowRect(h, &mut r) };
    (
        region,
        region_origin(ex, (ok != 0).then_some((r.left, r.top))),
    )
}

/// 撮影の判定 (D-077) のための 1 つの窓。見えていて、cloaked でなく、最小化しておらず、枠の矩形が取れるときだけ
/// Some。読む途中で閉じられた窓 (pid が 0、または最後の `IsWindow` が偽) も None (空の情報のまま不透明な覆いに
/// 数えて奥の deny の窓を引かないため。覆いを落とすのは断る向き)。題名は読まない。応答しないか (`hung`) は
/// `IsHungAppWindow` で読む (メッセージを送らない。D-115)。読み取りだけ。
pub fn stack_window(hwnd: usize) -> Option<StackWindow> {
    let h = hwnd as HWND;
    // 見えないか cloaked なら外す (判定の元は `hidden_state`。D-099)
    if !shown(h) {
        return None;
    }
    // SAFETY: ハンドルの値を渡すだけ。無効なハンドルや最小化していないウィンドウなら 0 が返る。
    if unsafe { IsIconic(h) } != 0 {
        return None;
    }
    let (rect, exact_frame) = frame_rect_with_source(h)?;
    // SAFETY: 同上。GWL_EXSTYLE の読み取りだけ (無効なハンドルなら 0 が返る)。
    let ex = unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32;
    let layered = if ex & WS_EX_LAYERED != 0 {
        layered_attributes(h)
    } else {
        None
    };
    let affinity = display_affinity(hwnd);
    let (region, origin) = window_region(h, ex);
    let region_full = region_covers_frame(rect, origin, region);
    let info = info_untitled(h);
    // 応答しない窓か (ゴーストの窓を deny に数えるかの判定。D-115)。最後の `IsWindow` の確かめより前に読む
    // (読む途中で閉じられた窓は下で落とす)。ゴーストの窓自身も真を返す (文書のとおり)。
    // SAFETY: ハンドルの値を渡すだけで、ポインタは渡さない。窓へメッセージを送らない読み (文書には書かれていないが、
    //         文書の判定はスレッドが最後にメッセージを取りに来た時刻によるので送る必要が無く、D-093 の枠でも使っている)。
    //         無効なハンドルなら偽 (0) が返ると見る (文書には無い。閉じた窓は下の `IsWindow` で落とす)。
    let hung = unsafe { IsHungAppWindow(h) } != 0;
    // SAFETY: ハンドルの値を渡すだけ。閉じられたハンドルなら 0 が返る。
    if info.pid == 0 || unsafe { IsWindow(h) } == 0 {
        return None;
    }
    Some(StackWindow {
        info,
        rect,
        // `rect` はリージョンで縮めない (deny の窓としても覆いとしても枠のまま。D-112)
        see_through: not_a_cover(ex, layered, affinity, exact_frame, region_full),
        hung,
    })
}

/// 撮影の判定 (D-077) のための、見えていて cloaked でなく最小化していないトップレベルの窓すべて (題名の無いもの、
/// ツールウィンドウを含む)。順は `EnumWindows` の順 (Z 順。前面から奥へ。design-notes)。題名は読まない。読み取りだけ。
pub fn stack_windows() -> Vec<StackWindow> {
    top_level_hwnds()
        .into_iter()
        .filter_map(stack_window)
        .collect()
}

/// 見えないトップレベルの窓 (`IsWindowVisible` が偽か、見えるが cloaked。D-099)。順は `EnumWindows` の順 (Z 順。
/// 前面から奥へ)。見えているかの読みは `shown` と同じ (`visibility`)。許可では絞らず、IME の窓などの雑音も呼ぶ側が
/// 除く。読み取りだけで、入力も撮影もしない。
/// 題名は `window_title` で読む (メッセージを送らない。D-099、D-105)。
/// プロセスの名前と権限は pid ごとに 1 回だけ読む (1 回の列挙の間にプロセスが終わって pid が再利用されると、後の
/// 窓に前のプロセスの名前が付きうる。`window_owners` と同じ限界で、極めて稀と推定)。読む途中で閉じられた窓 (pid が 0、最後の `IsWindow` が偽、
/// または読み終えた後の pid が最初と違う = ハンドルが別の窓に再利用された) は落とす。
pub fn hidden_top_windows() -> Vec<HiddenWindow> {
    let mut procs: std::collections::HashMap<u32, (String, Option<bool>)> =
        std::collections::HashMap::new();
    top_level_hwnds()
        .into_iter()
        .filter_map(|v| {
            let h = v as HWND;
            let is_cloaked = visibility(h)?;
            let pid = window_pid(h);
            if pid == 0 {
                return None;
            }
            // SAFETY: バッファの先頭と、その要素数 (cap) を渡す。関数は cap を超えて書かない。
            let class = read_wide(|p, n| unsafe { GetClassNameW(h, p, n) }, 256);
            let title = window_title(h);
            let (process, elevated) = procs
                .entry(pid)
                .or_insert_with(|| process_info(pid))
                .clone();
            // SAFETY: ハンドルの値を渡すだけ。閉じられたハンドルなら 0 が返る。
            if unsafe { IsWindow(h) } == 0 || window_pid(h) != pid {
                return None;
            }
            Some(HiddenWindow {
                info: WindowInfo {
                    hwnd: v,
                    pid,
                    process,
                    class,
                    title,
                    elevated,
                },
                cloaked: is_cloaked,
            })
        })
        .collect()
}

/// 窓の持ち主の pid。無効なハンドルなら 0。読み取りだけ。
fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid は書き込み可能な u32。ハンドルが無効なら 0 が返り、pid は 0 のまま。
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    pid
}

/// トップレベルのウィンドウ (見えないもの、ツールウィンドウ、cloaked を含む。`IsWindowVisible` で絞らない) の
/// 持ち主の pid と実行ファイル名。pid ごとに 1 つで、最初に見た順 (D-074)。名前が読めない持ち主 (昇格したプロセス
/// など) は空の名前で入る。読み取りだけで、入力も撮影もしない。
/// `GetWindowThreadProcessId` と `OpenProcess` の間にプロセスが終わって pid が再利用されると、別のプロセスの名前を
/// 読みうる (呼び出し側が出すのは許可済みの名前だけなので、許可に無い名前は漏れない)。
pub fn window_owners() -> Vec<WindowOwner> {
    let mut pids: Vec<u32> = Vec::new();
    for h in top_level_hwnds() {
        let mut pid = 0u32;
        // SAFETY: pid は書き込み可能な u32。列挙の後に閉じられたハンドルなら 0 が返り、pid は 0 のまま。
        unsafe { GetWindowThreadProcessId(h as HWND, &mut pid) };
        if pid != 0 && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids.into_iter()
        .map(|pid| WindowOwner {
            pid,
            process: process_name(pid),
        })
        .collect()
}

/// 有効なトップレベルのウィンドウか (前面化の補助プロセスが、渡された hwnd を確かめるのに使う。D-057)。
/// 無効なハンドルは枠の矩形が取れない (`describe` が None)。子ウィンドウはルートが自分ではない。読み取りだけ。
pub fn is_top_level(hwnd: usize) -> bool {
    hwnd != 0 && describe(hwnd).is_some() && hwnd_value(root_of(hwnd as HWND)) == hwnd
}

/// `SetForegroundWindow` を 1 回呼ぶ (D-050)。戻り値は OS が受け付けたか。0 になるのは、前面ロックで断られた、
/// ハンドルが無効 (閉じられた)、メニューの表示中、`LockSetForegroundWindow` 中など。1 でも前面になるとは限らない
/// (タスクバーの点滅だけのことがある) ので、前面になったかは呼び出し側が前面のウィンドウを取り直して確かめる。
/// 最小化したウィンドウは復元されずに活性化だけ移るので、呼ぶ前に `IsIconic` で断る。復元 (`ShowWindow`) や前面ロックの回避策
/// (Alt の空打ち、`AttachThreadInput`、`AllowSetForegroundWindow`、`SystemParametersInfo`) は使わない (D-020)。
pub fn set_foreground(hwnd: usize) -> bool {
    // SAFETY: ハンドルの値を渡すだけ。無効なハンドルなら 0 が返り、メモリは参照しない。
    unsafe { SetForegroundWindow(hwnd as HWND) != 0 }
}

/// `WM_CLOSE` を 1 回、待たずに置く (`PostMessageW`。D-062)。戻り値は置けたか。閉じるか (保存の確認を出すか) は
/// 相手のアプリが決める。入力は送らず、前面も変えない。`SendMessage` は相手が応答しないと戻らないので使わない。
pub fn post_close(hwnd: usize) -> bool {
    // SAFETY: ハンドルの値と、引数を持たないメッセージ (wParam と lParam は 0) を渡すだけ。無効なハンドルなら 0 が
    // 返り、メモリは参照しない。
    unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) != 0 }
}

/// ウィンドウが入力を受け付ける (無効になっていない) か (D-062)。モーダルのダイアログ (保存の確認など) を開いている
/// 間、持ち主のウィンドウは無効になり、その「閉じる」ボタンは押せない。`close` はそれに合わせて断る。
/// 既に無いハンドル (見つけた後に閉じられた) は「無効」と区別するため true を返す (続く `post_close` が失敗して 12 になる)。
/// 読み取りだけ。
pub fn is_enabled(hwnd: usize) -> bool {
    let h = hwnd as HWND;
    // SAFETY: どちらもハンドルの値を渡すだけ。無効なハンドルなら 0 が返り、メモリは参照しない。
    unsafe { IsWindow(h) == 0 || IsWindowEnabled(h) != 0 }
}

/// ハンドルのウィンドウの状態 (`close` の後に消えたかを見る。D-062、D-096)。ハンドルが無効なら `Destroyed`。
/// 持ち主の pid が 0 か `pid` と違えば、ハンドルが別のウィンドウに再利用されたとみなして `Destroyed`。pid が同じで
/// 見えなければ `Hidden` (タスクトレイに入るアプリ)、見えれば `Visible`。pid を読んでから見えるかを見る
/// (再利用された別のウィンドウの可視を読まない)。
/// 壊れる途中の窓も見えない (Microsoft Learn の「Destroying a Window」: WM_DESTROY は窓を画面から外した後に送られる)
/// ので、WM_DESTROY の処理の間は `Hidden` が返る。隠れた瞬間で「隠れただけ」と決めないのは呼ぶ側 (`close` の待ち) の
/// 役目。見えないと読んだ後は、ハンドルと pid を読み直し、読む間に壊れた窓を `Hidden` と言わない。読み取りだけ。
pub fn presence_of(hwnd: usize, pid: u32) -> WindowPresence {
    let h = hwnd as HWND;
    let alive_same_owner = || {
        // SAFETY: ハンドルの値を渡すだけ。無効なハンドルなら 0 が返り、メモリは参照しない。
        if unsafe { IsWindow(h) } == 0 {
            return false;
        }
        let mut owner = 0u32;
        // SAFETY: owner は書き込み可能な u32 で、呼び出しの間だけ使われる。ハンドルが無効なら 0 が返り、owner は 0 のまま。
        unsafe { GetWindowThreadProcessId(h, &mut owner) };
        owner != 0 && owner == pid
    };
    if !alive_same_owner() {
        return WindowPresence::Destroyed;
    }
    // SAFETY: ハンドルの値を渡すだけ。無効なハンドル (読む間に壊れた) なら 0 が返り、メモリは参照しない。
    if unsafe { IsWindowVisible(h) } != 0 {
        return WindowPresence::Visible;
    }
    // 見えない。IsWindowVisible は無効なハンドルにも 0 を返すので、読む間に壊れていないかを確かめ直す
    if alive_same_owner() {
        WindowPresence::Hidden
    } else {
        WindowPresence::Destroyed
    }
}

/// 所有者の連鎖の根 (`GA_ROOTOWNER`)。所有者が無ければ (または無効なハンドルなら) そのまま返す。読み取りだけ。
pub fn owner_root(hwnd: usize) -> usize {
    // SAFETY: GetAncestor は無効なハンドルに対しては null を返すだけで、メモリを参照しない。
    let r = unsafe { GetAncestor(hwnd as HWND, GA_ROOTOWNER) };
    if r.is_null() { hwnd } else { hwnd_value(r) }
}

/// このプロセスが管理者権限で動いているか。確かめられなければ false (管理者のウィンドウへの送信は拒否側に倒れる)。
pub fn self_elevated() -> bool {
    // SAFETY: 引数なし。現在のプロセスの擬似ハンドルを返すだけで、閉じる必要はない。
    let me = unsafe { GetCurrentProcess() };
    token_elevated(me).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WDA_MONITOR, WS_EX_TRANSPARENT};

    /// 透けうるかは層の属性で決まる (D-077)。失敗 (ピクセルごとのアルファ)、色キー、255 未満の alpha は透ける。
    /// alpha 255 の層付きの窓と `LWA_ALPHA` の無い層付きの窓は不透明 (層付きのアプリのたびに断らない)。
    /// `WS_EX_TRANSPARENT` (ヒットテストだけ) は見ない
    #[test]
    fn see_through_by_layered_attributes() {
        // 層なし: 属性に関わらず不透明
        assert!(!see_through(0, None));
        assert!(!see_through(WS_EX_TOOLWINDOW, Some((LWA_ALPHA, 0))));
        // 層ありで失敗 (UpdateLayeredWindow など)
        assert!(see_through(WS_EX_LAYERED, None));
        // 色キー
        assert!(see_through(WS_EX_LAYERED, Some((LWA_COLORKEY, 255))));
        assert!(see_through(
            WS_EX_LAYERED,
            Some((LWA_COLORKEY | LWA_ALPHA, 255))
        ));
        // alpha 254 は透け、255 は不透明
        assert!(see_through(WS_EX_LAYERED, Some((LWA_ALPHA, 254))));
        assert!(!see_through(WS_EX_LAYERED, Some((LWA_ALPHA, 255))));
        // LWA_ALPHA が無ければ alpha の値は使わない
        assert!(!see_through(WS_EX_LAYERED, Some((0, 0))));
        // TRANSPARENT だけ (層なし) は不透明、層付きでも TRANSPARENT は判定を変えない
        assert!(!see_through(WS_EX_TRANSPARENT, None));
        assert!(!see_through(
            WS_EX_LAYERED | WS_EX_TRANSPARENT,
            Some((LWA_ALPHA, 255))
        ));
    }

    /// 見えているかの判定の元 (D-099): 見えて cloaked でなければ入れない (None)。見えて cloaked なら cloaked、
    /// 見えなければ cloaked かどうかに関わらず hidden
    #[test]
    fn hidden_state_splits_shown_hidden_and_cloaked() {
        assert_eq!(hidden_state(true, false), None);
        assert_eq!(hidden_state(true, true), Some(true));
        assert_eq!(hidden_state(false, false), Some(false));
        assert_eq!(hidden_state(false, true), Some(false));
    }

    #[test]
    fn not_a_cover_adds_capture_exclusion_and_inexact_frame() {
        // 普通の不透明な窓 (affinity が読めない場合も含む) は覆い
        assert!(!not_a_cover(0, None, None, true, true));
        assert!(!not_a_cover(0, None, Some(0), true, true));
        // WDA_MONITOR は黒く写るので覆いのまま
        assert!(!not_a_cover(0, None, Some(WDA_MONITOR), true, true));
        // 撮影から外れる窓は写らないので覆いにしない
        assert!(not_a_cover(
            0,
            None,
            Some(WDA_EXCLUDEFROMCAPTURE),
            true,
            true
        ));
        // DWM の枠が取れない窓は覆いにしない
        assert!(not_a_cover(0, None, Some(0), false, true));
        // 層の属性で透ける窓は今までどおり
        assert!(not_a_cover(WS_EX_LAYERED, None, Some(0), true, true));
    }

    /// `GetWindowRgnBox` の戻り値の分類 (D-112)。ERROR はリージョンが無いとみなし、範囲外の値と大きさ 0 以下の
    /// SIMPLEREGION は COMPLEX (断る向き)。i32 の端の値でも panic しない
    #[test]
    fn classify_region_maps_codes_and_fails_closed() {
        assert_eq!(classify_region(0, 0, 0, 0, 0), WindowRegion::NoRegion);
        assert_eq!(classify_region(1, 0, 0, 0, 0), WindowRegion::Empty);
        assert_eq!(
            classify_region(2, 10, 20, 310, 220),
            WindowRegion::Simple(Rect::new(10, 20, 300, 200))
        );
        // 幅 0 と負の幅
        assert_eq!(classify_region(2, 10, 20, 10, 220), WindowRegion::Complex);
        assert_eq!(classify_region(2, 10, 20, 5, 220), WindowRegion::Complex);
        assert_eq!(classify_region(3, 0, 0, 100, 100), WindowRegion::Complex);
        // 範囲外の戻り値
        assert_eq!(classify_region(-1, 0, 0, 100, 100), WindowRegion::Complex);
        assert_eq!(classify_region(4, 0, 0, 100, 100), WindowRegion::Complex);
        // i32 の引き算なら debug で panic する端の値
        assert_eq!(
            classify_region(2, i32::MIN, 0, i32::MAX, 1),
            WindowRegion::Simple(Rect::new(i32::MIN, 0, u32::MAX, 1))
        );
    }

    /// リージョンが枠をまるごと覆うときだけ覆いに数える (D-112)。矩形は窓の原点 (`GetWindowRect` の左上) から
    /// 画面に直して比べる。原点が読めない、足し算が溢れる場合は偽 (断る向き)
    #[test]
    fn region_covers_frame_only_when_region_contains_frame() {
        let frame = Rect::new(960, 540, 640, 400);
        let origin = Some((960, 540));
        assert!(region_covers_frame(frame, origin, WindowRegion::NoRegion));
        assert!(region_covers_frame(frame, None, WindowRegion::NoRegion));
        assert!(!region_covers_frame(frame, origin, WindowRegion::Empty));
        assert!(!region_covers_frame(frame, origin, WindowRegion::Complex));
        let simple = |x, y, w, h| WindowRegion::Simple(Rect::new(x, y, w, h));
        // 枠と同じ、枠より大きい
        assert!(region_covers_frame(frame, origin, simple(0, 0, 640, 400)));
        assert!(region_covers_frame(frame, origin, simple(-5, -5, 700, 500)));
        // 枠の一部
        assert!(!region_covers_frame(
            frame,
            origin,
            simple(10, 20, 300, 200)
        ));
        assert!(!region_covers_frame(frame, origin, simple(0, 0, 639, 400)));
        // 原点が読めない
        assert!(!region_covers_frame(frame, None, simple(0, 0, 640, 400)));
        // 見えない縁で原点が枠より 7 ピクセル左上の窓
        let edged = Some((953, 533));
        assert!(region_covers_frame(frame, edged, simple(0, 0, 654, 414)));
        assert!(!region_covers_frame(frame, edged, simple(0, 0, 640, 400)));
        // 負の原点
        let left = Rect::new(-1000, -500, 400, 300);
        assert!(region_covers_frame(
            left,
            Some((-1000, -500)),
            simple(0, 0, 400, 300)
        ));
        // 足し算の溢れ (panic しない)
        assert!(!region_covers_frame(
            frame,
            Some((i32::MAX, 0)),
            simple(10, 0, 10, 10)
        ));
        // 端の値の Simple (panic しない)
        assert!(!region_covers_frame(
            Rect::new(0, 0, 100, 100),
            Some((0, 0)),
            simple(i32::MIN, 0, u32::MAX, 1)
        ));
    }

    /// リージョンが枠をまるごと覆わない窓は、他が全部覆いでも覆いにしない (D-112)
    #[test]
    fn not_a_cover_adds_partial_region() {
        assert!(not_a_cover(0, None, Some(0), true, false));
        assert!(!not_a_cover(0, None, Some(0), true, true));
    }

    /// 右から左のレイアウトの窓は原点を読まず、枠と同じリージョンでも覆いにしない (D-112。断る向き)。
    /// それ以外の窓は `GetWindowRect` の左上をそのまま使う
    #[test]
    fn region_origin_drops_rtl_layout() {
        let frame = Rect::new(960, 540, 640, 400);
        let full = WindowRegion::Simple(Rect::new(0, 0, 640, 400));
        let read = Some((960, 540));
        assert_eq!(region_origin(0, read), read);
        assert_eq!(region_origin(WS_EX_LAYERED, read), read);
        assert_eq!(region_origin(0, None), None);
        assert_eq!(region_origin(WS_EX_LAYOUTRTL, read), None);
        assert_eq!(region_origin(WS_EX_LAYOUTRTL | WS_EX_LAYERED, read), None);
        assert!(region_covers_frame(frame, region_origin(0, read), full));
        let rtl = region_covers_frame(frame, region_origin(WS_EX_LAYOUTRTL, read), full);
        assert!(!rtl);
        assert!(not_a_cover(WS_EX_LAYOUTRTL, None, Some(0), true, rtl));
        // リージョンの無い RTL の窓 (ほとんどの RTL の窓) は今までどおり覆い
        assert!(region_covers_frame(
            frame,
            region_origin(WS_EX_LAYOUTRTL, None),
            WindowRegion::NoRegion
        ));
    }
}
