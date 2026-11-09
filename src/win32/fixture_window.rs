//! 実機テスト専用のフィクスチャウィンドウ (D-020)。一意なタイトルの最前面のポップアップで、
//! 受け取ったマウスとキーのメッセージを記録する。製品の経路からは使わない。
//!
//! IME はこのウィンドウに対して外す (`ImmAssociateContextEx`)。IME が有効だと、キーの押下が
//! `VK_PROCESSKEY` に化けて、送ったキーを検証できない。`open_with_ime` だけは外さず、IME のメッセージ
//! (`WM_IME_STARTCOMPOSITION` など) も記録する (IME がオンのときの `type` の測定用。D-133)。
//!
//! `open_with_controls` は BUTTON と読み取り専用の EDIT を子に持つ (UIA の実機テスト用。子のメッセージは
//! 記録しない。D-088)。`open_with_password` は `ES_PASSWORD` の EDIT を 1 つ子に持つ (D-090)。`open` は子を持たない。
//!
//! `set_hide_on_close(true)` にしたフィクスチャは、`WM_CLOSE` で壊れず隠れるだけになる (トレイに入るアプリを模す。D-096)。
//!
//! `stall` は窓のスレッドを一時的に眠らせ、メッセージを汲まない自プロセスの窓を作る (D-105)。
//! 別のスレッドから眠らせるときは `staller` の値 (`FixtureStaller`) を渡す (D-108)。
//!
//! `set_region` はフィクスチャに窓のリージョン (`SetWindowRgn`) を付け外しする (撮影の覆いの判定の実機テスト用。D-112)。
//!
//! `open_with_caption` は窓の上端の帯 (`FIXTURE_CAPTION_HEIGHT`) を `WM_NCHITTEST` で `HTCAPTION` にして、タイトルバーの
//! 代わりに窓を動かせるようにする (drag の途中のガードの実機テスト用。D-132)。

use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateEllipticRgn, CreateRectRgn, CreateSolidBrush, DeleteObject,
    EndPaint, FillRect, HGDIOBJ, HRGN, PAINTSTRUCT, SetWindowRgn,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::Ime::{
    GCS_COMPSTR, GCS_RESULTSTR, ImmAssociateContextEx, ImmGetContext, ImmGetOpenStatus,
    ImmReleaseContext, ImmSetOpenStatus,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    ES_AUTOHSCROLL, ES_MULTILINE, ES_PASSWORD, ES_READONLY, GetClientRect, GetDlgItem, GetMessageW,
    GetWindowRect, GetWindowThreadProcessId, HMENU, HTCAPTION, IDC_ARROW, IsWindowVisible,
    LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SMTO_ABORTIFHUNG, SW_HIDE,
    SendMessageTimeoutW, SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity, ShowWindow,
    ShowWindowAsync, TranslateMessage, WINDOW_STYLE, WM_APP, WM_CHAR, WM_CLOSE, WM_DESTROY,
    WM_ERASEBKGND, WM_GETTEXT, WM_GETTEXTLENGTH, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
    WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::geometry::{Point, Rect};
use crate::script::Button;

/// フィクスチャの塗りの色 (撮影のテストで照合する)
pub const FIXTURE_RGB: (u8, u8, u8) = (0x22, 0x88, 0xDD);

/// 子つきのフィクスチャ (`open_with_controls`) の子のボタンの題名 (UIA の名前になる。D-088)
pub const FIXTURE_BUTTON_TEXT: &str = "ai-cu fixture button";
/// 子つきのフィクスチャ (`open_with_controls`) の子のボタンのコントロール ID (D-088)
pub const FIXTURE_BUTTON_ID: i32 = 101;
/// 子つきのフィクスチャ (`open_with_controls`) の子のエディットの本文。CRLF の改行と日本語を含む 2 行 (D-088)
pub const FIXTURE_EDIT_TEXT: &str = "一行目 fixture\r\n二行目 テキスト";
/// 子つきのフィクスチャ (`open_with_controls`) の子のエディットのコントロール ID (D-088)
pub const FIXTURE_EDIT_ID: i32 = 102;
/// 子つきのフィクスチャ (`open_with_controls`) の子のボタンの位置と大きさ。クライアント座標 (D-088)
pub const FIXTURE_BUTTON_RECT: Rect = Rect::new(10, 10, 200, 32);
/// 子つきのフィクスチャ (`open_with_controls`) の子のエディットの位置と大きさ。クライアント座標 (D-088)
pub const FIXTURE_EDIT_RECT: Rect = Rect::new(10, 52, 300, 80);
/// パスワード欄つきのフィクスチャ (`open_with_password`) の子の `ES_PASSWORD` のエディットの初期の文字列 (D-090)
pub const FIXTURE_PASSWORD_TEXT: &str = "ai-cu-secret";
/// パスワード欄つきのフィクスチャ (`open_with_password`) の子のエディットのコントロール ID (D-090)
pub const FIXTURE_PASSWORD_ID: i32 = 103;
/// パスワード欄つきのフィクスチャ (`open_with_password`) の子のエディットの位置と大きさ。クライアント座標 (D-090)。
/// 右と下に親の空いた所を残す (フォーカスを親に戻すクリックの先)
pub const FIXTURE_PASSWORD_RECT: Rect = Rect::new(10, 10, 300, 28);
/// 帯つきのフィクスチャ (`open_with_caption`) の帯の高さ。窓の上端 (`GetWindowRect`) からのピクセル数 (D-132)
pub const FIXTURE_CAPTION_HEIGHT: i32 = 40;
/// `FixtureEvent::ImeComposition` の `flags` の、未確定の文字列が変わった印 (`GCS_COMPSTR`。D-133)
pub const FIXTURE_GCS_COMPSTR: u32 = GCS_COMPSTR;
/// `FixtureEvent::ImeComposition` の `flags` の、確定した文字列がある印 (`GCS_RESULTSTR`。D-133)
pub const FIXTURE_GCS_RESULTSTR: u32 = GCS_RESULTSTR;

const MK_LBUTTON: usize = 0x0001;
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

/// `FixtureWindow::set_region` で付ける窓のリージョン (D-112 の実機テスト用)。`Clear` は外す、`Rect` は窓の左上
/// (`GetWindowRect`) を原点にした矩形、`Ellipse` は窓全体に内接する楕円 (矩形でない形。COMPLEXREGION になる)
#[derive(Debug, Clone, Copy)]
pub enum FixtureRegion {
    Clear,
    Rect(Rect),
    Ellipse,
}

/// 受け取ったメッセージ。座標はクライアント座標。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureEvent {
    ButtonDown {
        button: Button,
        x: i32,
        y: i32,
    },
    ButtonUp {
        button: Button,
        x: i32,
        y: i32,
    },
    DoubleClick {
        x: i32,
        y: i32,
    },
    /// 左ボタンを押したままの移動
    DragMove {
        x: i32,
        y: i32,
    },
    Wheel {
        delta: i32,
    },
    KeyDown {
        vk: u16,
    },
    Char {
        unit: u16,
    },
    /// `WM_IME_STARTCOMPOSITION`。IME を外さないフィクスチャ (`open_with_ime`) にだけ届く (D-133)
    ImeStart,
    /// `WM_IME_COMPOSITION`。`flags` は `lparam` の下位 32 ビット (`GCS_*`) (D-133)
    ImeComposition {
        flags: u32,
    },
    /// `WM_IME_ENDCOMPOSITION` (D-133)
    ImeEnd,
}

static EVENTS: Mutex<Vec<(usize, FixtureEvent)>> = Mutex::new(Vec::new());

fn record(hwnd: HWND, e: FixtureEvent) {
    if let Ok(mut v) = EVENTS.lock() {
        v.push((hwnd as usize, e));
    }
}

/// `WM_CLOSE` を受けても壊さず隠すだけにするフィクスチャのハンドル (`set_hide_on_close`。D-096)
static HIDE_ON_CLOSE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// `hwnd` が `WM_CLOSE` で隠れるだけのフィクスチャか。lock が毒されていても中身を読む (`set_hide_on_close` と同じ。
/// Drop は WM_CLOSE を置く前に必ず一覧から外すので、毒されていても join が返らなくなることはない)
fn hides_on_close(hwnd: HWND) -> bool {
    HIDE_ON_CLOSE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&(hwnd as usize))
}

/// フィクスチャの窓のスレッドを `wparam` ミリ秒だけ眠らせる自前のメッセージ (`stall`。D-105 の実機テスト用)。
/// 枠の窓 (`overlay.rs`) が使う `WM_APP + 1..=3` と重ならない値にする (クラスは別だが、grep で見分けやすくする)
const WM_FIXTURE_STALL: u32 = WM_APP + 0x10;

/// フィクスチャの IME を窓のスレッドで開け閉めする自前のメッセージ (`set_ime_open`。D-133)。`wparam` が非 0 なら開く。
/// 読み戻した状態 (1 か 0) を返し、入力コンテキストが無ければ -1 を返す
const WM_FIXTURE_IME_OPEN: u32 = WM_APP + 0x11;

/// `WM_FIXTURE_STALL` で眠っている間のフィクスチャのハンドル (`stall`、`is_stalled`。D-105)
static STALLED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// `STALLED` を取る。毒されていても中身を使う (`HIDE_ON_CLOSE` と同じ。窓の手続きは `extern "system"` なので、
/// 中で panic すると巻き戻らずにプロセスごと止まる)
fn stalled() -> std::sync::MutexGuard<'static, Vec<usize>> {
    STALLED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 帯つきのフィクスチャのハンドル (`open_with_caption`。D-132)。`WM_DESTROY` で外す
static CAPTION_WINDOWS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// `CAPTION_WINDOWS` を取る。毒されていても中身を使う (`stalled` と同じ。窓の手続きの中で panic させない)
fn caption_windows() -> MutexGuard<'static, Vec<usize>> {
    CAPTION_WINDOWS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 画面座標の `y` が、上端が `window_top` の窓の帯の中か (D-132)。窓より上の点は帯に数えない
/// (マウスを捕まえている間は、窓の外の点でも `WM_NCHITTEST` が窓に届く)
fn in_caption_band(y: i32, window_top: i32) -> bool {
    (0..FIXTURE_CAPTION_HEIGHT).contains(&(y - window_top))
}

/// 座標は符号つきで読む (Microsoft Learn の WM_NCHITTEST の Remarks。複数モニターでは負になりうる)
fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam & 0xFFFF) as u16 as i16;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16;
    (i32::from(x), i32::from(y))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let (x, y) = lparam_xy(lparam);
    match msg {
        WM_LBUTTONDOWN => {
            // パスワード欄つき (`open_with_password`) だけ、親の空いた所のクリックでフォーカスを親に戻す
            // (既定の処理はクリックでフォーカスを動かさない。子つきと子なしの振る舞いは変えない。D-090)
            // SAFETY: このウィンドウプロシージャが受け取った、このスレッドの窓のハンドルと整数の ID を渡すだけ。
            //         子が無ければ null が返り、メモリは参照しない。
            if !unsafe { GetDlgItem(hwnd, FIXTURE_PASSWORD_ID) }.is_null() {
                // SAFETY: hwnd はこのスレッドが作った窓 (ウィンドウプロシージャは作ったスレッドで呼ばれる)。
                //         SetFocus はそのスレッドの窓にフォーカスを移すだけ。
                unsafe { SetFocus(hwnd) };
            }
            record(
                hwnd,
                FixtureEvent::ButtonDown {
                    button: Button::Left,
                    x,
                    y,
                },
            );
        }
        WM_LBUTTONUP => record(
            hwnd,
            FixtureEvent::ButtonUp {
                button: Button::Left,
                x,
                y,
            },
        ),
        WM_RBUTTONDOWN => record(
            hwnd,
            FixtureEvent::ButtonDown {
                button: Button::Right,
                x,
                y,
            },
        ),
        WM_RBUTTONUP => record(
            hwnd,
            FixtureEvent::ButtonUp {
                button: Button::Right,
                x,
                y,
            },
        ),
        WM_LBUTTONDBLCLK => record(hwnd, FixtureEvent::DoubleClick { x, y }),
        WM_MOUSEMOVE if wparam & MK_LBUTTON != 0 => record(hwnd, FixtureEvent::DragMove { x, y }),
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16;
            record(
                hwnd,
                FixtureEvent::Wheel {
                    delta: i32::from(delta),
                },
            );
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => record(
            hwnd,
            FixtureEvent::KeyDown {
                vk: (wparam & 0xFFFF) as u16,
            },
        ),
        WM_CHAR => record(
            hwnd,
            FixtureEvent::Char {
                unit: (wparam & 0xFFFF) as u16,
            },
        ),
        // IME のメッセージは記録だけして、下の既定の処理に進む (確定の文字を WM_CHAR にするのも既定の処理)。
        // IME を外したフィクスチャには届かない (D-133)
        WM_IME_STARTCOMPOSITION => record(hwnd, FixtureEvent::ImeStart),
        WM_IME_COMPOSITION => record(
            hwnd,
            FixtureEvent::ImeComposition {
                flags: lparam as u32,
            },
        ),
        WM_IME_ENDCOMPOSITION => record(hwnd, FixtureEvent::ImeEnd),
        WM_FIXTURE_IME_OPEN => {
            // SAFETY: このウィンドウプロシージャが受け取った、このスレッドの窓のハンドルを渡すだけ。
            //         入力コンテキストが無ければ null が返り、メモリは参照しない。
            let himc = unsafe { ImmGetContext(hwnd) };
            if himc.is_null() {
                return -1;
            }
            // SAFETY: himc は直前の ImmGetContext が返した非 null のコンテキストで、下の ImmReleaseContext まで有効。
            //         BOOL の値を渡すだけ。
            unsafe { ImmSetOpenStatus(himc, i32::from(wparam != 0)) };
            // SAFETY: himc は上と同じで、状態を読むだけ。
            let open = unsafe { ImmGetOpenStatus(himc) } != 0;
            // SAFETY: 同じ窓のハンドルと、その窓の ImmGetContext が返した himc を 1 回だけ返す。以後 himc は使わない。
            unsafe { ImmReleaseContext(hwnd, himc) };
            return LRESULT::from(open);
        }
        WM_FIXTURE_STALL => {
            // 印を立ててからロックを放して眠る (握ったまま眠ると `stall` / `is_stalled` の確認が眠りの終わりまで
            // 待たされる)。眠る間、このスレッドはメッセージを汲まない (D-105 の実機テスト用)
            stalled().push(hwnd as usize);
            std::thread::sleep(Duration::from_millis(wparam as u64));
            stalled().retain(|h| *h != hwnd as usize);
            return 0;
        }
        // 帯つきのフィクスチャだけ、帯の中をタイトルバーと答える。ここの lparam は画面座標で、掴んだ後の窓の移動は
        // 既定の処理 (DefWindowProc) に任せる。帯の外と読めないときは下の既定の処理に進む (D-132)
        WM_NCHITTEST if caption_windows().contains(&(hwnd as usize)) => {
            if let Ok(window) = screen_rect_of(hwnd)
                && in_caption_band(y, window.y)
            {
                return HTCAPTION as LRESULT;
            }
        }
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: WM_PAINT の処理中に、このウィンドウのハンドルと書き込み可能な PAINTSTRUCT を渡す。
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if !hdc.is_null() {
                let (r, g, b) = FIXTURE_RGB;
                let color = u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
                // SAFETY: COLORREF の値を渡すだけ。返されたブラシはこの下で削除する。
                let brush = unsafe { CreateSolidBrush(color) };
                if !brush.is_null() {
                    // SAFETY: hdc は BeginPaint が返した有効な DC、rcPaint は ps の中の有効な RECT、brush は有効。
                    unsafe { FillRect(hdc, &ps.rcPaint, brush) };
                    // SAFETY: 上で作ったブラシを 1 回だけ削除する。DC には選択していない。
                    unsafe { DeleteObject(brush as HGDIOBJ) };
                }
            }
            // SAFETY: 対応する BeginPaint に渡したのと同じ PAINTSTRUCT を渡す。
            unsafe { EndPaint(hwnd, &ps) };
            return 0;
        }
        WM_CLOSE if hides_on_close(hwnd) => {
            // トレイに入るアプリを模す: 壊さず隠すだけ (D-096)。一覧に無ければ下の既定の処理 (DestroyWindow) に進む
            // SAFETY: ウィンドウプロシージャは窓を作ったスレッドで呼ばれるので、自分の窓に同期の ShowWindow を
            //         呼んでよい。ハンドルと表示の定数を渡すだけで、メモリは参照しない。
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            return 0;
        }
        WM_DESTROY => {
            caption_windows().retain(|h| *h != hwnd as usize);
            // SAFETY: このスレッドのメッセージループに WM_QUIT を置くだけ。
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        _ => {}
    }
    // SAFETY: ウィンドウプロシージャが受け取った引数をそのまま既定の処理に渡す。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub struct FixtureWindow {
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
    /// 生存の印 (開いたときに真、Drop が WM_CLOSE を置く前に偽)。`FixtureStaller` と共有する (D-108)。
    /// `stall` は印を確かめてから置くまでロックを握るので、Drop が印を偽にした後には置かない (win32-reviewer の指摘)
    alive: Arc<Mutex<bool>>,
}

/// 生存の印のロックを取る。毒されていても中身を使う (`stalled` と同じ)
fn lock_alive(alive: &Mutex<bool>) -> MutexGuard<'_, bool> {
    alive.lock().unwrap_or_else(|e| e.into_inner())
}

/// フィクスチャの窓のスレッドを眠らせる値 (D-108)。`FixtureWindow::staller` だけが作る (外から作れない)。
/// `Clone + Send` なので、フィクスチャを借りられない別のスレッド (UIA のスレッドの閉包など) に渡せる。
/// フィクスチャが閉じた後や、hwnd が自プロセスの窓でなくなった後は何も置かない
#[derive(Clone, Debug)]
pub struct FixtureStaller {
    hwnd: usize,
    alive: Arc<Mutex<bool>>,
}

/// 眠りのメッセージを置いてよいか (D-108)。生存の印が真で、窓の持ち主の pid (`GetWindowThreadProcessId`。0 は
/// 読めない) が自分の pid (`GetCurrentProcessId`) と等しいときだけ真
fn may_post(alive: bool, owner_pid: u32, my_pid: u32) -> bool {
    alive && owner_pid != 0 && owner_pid == my_pid
}

impl FixtureStaller {
    fn handle(&self) -> HWND {
        self.hwnd as HWND
    }

    /// フィクスチャの窓のスレッドを `dur` だけ眠らせる (D-105 の実機テスト用)。自分の窓に自分のメッセージ
    /// (`WM_FIXTURE_STALL`) を `PostMessageW` で置くだけで、入力も撮影もしない。眠る間は窓のスレッドがメッセージを
    /// 汲まない。置いたあと、窓の手続きが眠りの印を立てるまで最大 1 秒 (10 ms 刻み) 待ち、立ったかを返す
    /// (立つ前に測り始めると、止まる前に読み終えて対照が効かない)。`dur` は待ちの刻み (10 ms) より十分に長くする。
    /// 前の眠りが続いている間 (`is_stalled` が真) は、メッセージを置かずに偽を返す (前の眠りの印で真を返すと、
    /// 測りが前の眠りの残りにしか掛からない)。置くのに失敗しても偽。1 秒待っても印が立たずに偽を返したときも、
    /// 置いたメッセージはキューに残り、眠りは後で起きうる。Drop の WM_CLOSE は眠りが終わってから処理される。
    /// フィクスチャが閉じた後 (生存の印が偽) か、hwnd の持ち主が自プロセスでなければ、置かずに偽を返す (D-108)。
    /// Drop と並ぶときは、置き終えてから閉じる (Drop の join は眠りの長さだけ待つ) か、置かずに偽を返すかのどちらか
    pub fn stall(&self, dur: Duration) -> bool {
        if self.is_stalled() {
            return false;
        }
        // ミリ秒が WPARAM (usize) に収まらなければ上限に寄せる (テストは数秒しか渡さない)
        let ms = WPARAM::try_from(dur.as_millis()).unwrap_or(WPARAM::MAX);
        {
            // 印を確かめてから置くまでロックを握る。Drop は同じロックを取って印を偽にしてから WM_CLOSE を置くので、
            // 閉じ始めた後には置かない。PostMessageW は待たずに戻るので、握ったままでよい
            let alive = lock_alive(&self.alive);
            let mut owner = 0u32;
            // SAFETY: owner は書き込み可能な u32。ハンドルの値を渡して持ち主を読むだけで、窓へメッセージを送らない。
            //         無効なハンドルなら 0 が返り、owner は 0 のまま。
            unsafe { GetWindowThreadProcessId(self.handle(), &mut owner) };
            // SAFETY: 引数を取らず、このプロセスの id を返すだけ。
            let me = unsafe { GetCurrentProcessId() };
            if !may_post(*alive, owner, me) {
                return false;
            }
            // SAFETY: 生存の印が真 (ロックを握っているので Drop はまだ閉じ始めていない) で持ち主が自プロセスと確かめた
            //         フィクスチャの窓のハンドルに、整数だけを運ぶ自前のメッセージを置いて待たずに戻る。lParam は 0 で
            //         ポインタを渡さない。利用者が窓を閉じた後なら (印は真のまま) 失敗の 0 が返るだけで、メモリは参照しない
            //         (その間に hwnd の値が別のプロセスの窓に使い回される恐れは、持ち主の pid の確かめと hwnd の上位の
            //         再利用の数に頼る)。
            if unsafe { PostMessageW(self.handle(), WM_FIXTURE_STALL, ms, 0) } == 0 {
                return false;
            }
        }
        let start = std::time::Instant::now();
        while !self.is_stalled() {
            if start.elapsed() >= Duration::from_secs(1) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// フィクスチャの窓のスレッドが `stall` で眠っているか (眠りの印に自分のハンドルがあるか。D-105)
    pub fn is_stalled(&self) -> bool {
        stalled().contains(&self.hwnd)
    }
}

/// 親 `parent` の子のコントロールを 1 つ作る。`rect` はクライアント座標。失敗なら `GetLastError` の値を `Err` で返す。
/// 親を作ったスレッドから呼ぶ (子は親と同じスレッドのメッセージループで処理される)。
fn create_child(
    parent: HWND,
    hinst: windows_sys::Win32::Foundation::HINSTANCE,
    class: &str,
    text: &str,
    style: WINDOW_STYLE,
    rect: Rect,
    id: i32,
) -> Result<HWND, u32> {
    let class_w = super::wide(class);
    let text_w = super::wide(text);
    // SAFETY: クラス名と題名は NUL 終端の UTF-16 で、呼び出しの間生きている。parent はこのスレッドが作った
    // 有効なウィンドウ。WS_CHILD の窓では hMenu はポインタでなくコントロール ID として扱われる (参照されない)。
    let child = unsafe {
        CreateWindowExW(
            0,
            class_w.as_ptr(),
            text_w.as_ptr(),
            style,
            rect.x,
            rect.y,
            rect.w as i32,
            rect.h as i32,
            parent,
            id as isize as HMENU,
            hinst,
            std::ptr::null(),
        )
    };
    if child.is_null() {
        // 失敗の値は CreateWindowExW の直後に読む (class_w と text_w の drop の HeapFree より前)
        return Err(super::last_error());
    }
    Ok(child)
}

/// 子つきのフィクスチャの子 (ボタン、エディットの順) を作る。どちらかが失敗したら、その直後の
/// `GetLastError` を添えた文を返す (親を壊すのは呼び出し側)。
fn create_controls(
    parent: HWND,
    hinst: windows_sys::Win32::Foundation::HINSTANCE,
) -> Result<(), String> {
    // ボタンを先、エディットを後に作る (子は Z 順の下に足されるので、UIA の木の順がボタン、エディットになる)
    create_child(
        parent,
        hinst,
        "BUTTON",
        FIXTURE_BUTTON_TEXT,
        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as WINDOW_STYLE,
        FIXTURE_BUTTON_RECT,
        FIXTURE_BUTTON_ID,
    )
    .map_err(|err| format!("子 (button) の CreateWindowExW が失敗 (GetLastError={err})"))?;
    create_child(
        parent,
        hinst,
        "EDIT",
        FIXTURE_EDIT_TEXT,
        WS_CHILD
            | WS_VISIBLE
            | WS_BORDER
            | ES_MULTILINE as WINDOW_STYLE
            | ES_READONLY as WINDOW_STYLE,
        FIXTURE_EDIT_RECT,
        FIXTURE_EDIT_ID,
    )
    .map_err(|err| format!("子 (edit) の CreateWindowExW が失敗 (GetLastError={err})"))?;
    Ok(())
}

/// パスワード欄つきのフィクスチャの子 (`ES_PASSWORD | ES_AUTOHSCROLL` の EDIT 1 つ) を作る。失敗したら、その直後の
/// `GetLastError` を添えた文を返す (親を壊すのは呼び出し側。D-090)
fn create_password_edit(
    parent: HWND,
    hinst: windows_sys::Win32::Foundation::HINSTANCE,
) -> Result<(), String> {
    create_child(
        parent,
        hinst,
        "EDIT",
        FIXTURE_PASSWORD_TEXT,
        WS_CHILD
            | WS_VISIBLE
            | WS_BORDER
            | ES_PASSWORD as WINDOW_STYLE
            | ES_AUTOHSCROLL as WINDOW_STYLE,
        FIXTURE_PASSWORD_RECT,
        FIXTURE_PASSWORD_ID,
    )
    .map_err(|err| format!("子 (password edit) の CreateWindowExW が失敗 (GetLastError={err})"))?;
    Ok(())
}

/// フィクスチャの子の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Children {
    /// 子なし (`open`)
    None,
    /// BUTTON と読み取り専用の EDIT (`open_with_controls`。D-088)
    Controls,
    /// `ES_PASSWORD` の EDIT 1 つ (`open_with_password`。D-090)
    Password,
    /// 子なしで、窓の上端の帯が `HTCAPTION` になる (`open_with_caption`。D-132)
    Caption,
    /// 子なしで、IME を外さない (`open_with_ime`。D-133)
    Ime,
}

fn window_thread(
    rect: Rect,
    title: String,
    children: Children,
    tx: mpsc::SyncSender<Result<usize, String>>,
) {
    let class = super::wide(&format!("ai_cu_fixture_{}", std::process::id()));
    let title_w = super::wide(&title);
    // SAFETY: null を渡すと、この実行ファイルのモジュールのハンドルが返る。
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    // SAFETY: IDC_ARROW は組み込みのカーソルの識別子。hinstance は null (システムのカーソル)。
    let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hCursor: cursor,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    // SAFETY: wc は初期化済みで cbSize が正しく、文字列 class は登録の間生きている (クラス名は登録時に複製される)。
    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        let err = super::last_error();
        if err != ERROR_CLASS_ALREADY_EXISTS {
            let _ = tx.send(Err(format!("RegisterClassExW が失敗 (GetLastError={err})")));
            return;
        }
    }
    // 子つきのときだけ WS_CLIPCHILDREN を足す (親の WM_PAINT の塗りが子を上書きしないため)。子なしは今までどおり
    let style = if matches!(children, Children::Controls | Children::Password) {
        WS_POPUP | WS_VISIBLE | WS_BORDER | WS_CLIPCHILDREN
    } else {
        WS_POPUP | WS_VISIBLE | WS_BORDER
    };
    // SAFETY: クラス名とタイトルは NUL 終端の UTF-16 で、呼び出しの間生きている。親とメニューは null。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title_w.as_ptr(),
            style,
            rect.x,
            rect.y,
            rect.w as i32,
            rect.h as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = tx.send(Err(format!(
            "CreateWindowExW が失敗 (GetLastError={})",
            super::last_error()
        )));
        return;
    }
    // IME の測定用のフィクスチャだけ外さない (D-133)
    if children != Children::Ime {
        // SAFETY: hwnd はこのスレッドが作った有効なウィンドウ。null の入力コンテキストを結びつけて IME を外す。
        unsafe { ImmAssociateContextEx(hwnd, std::ptr::null_mut(), 0) };
    }
    let made = match children {
        Children::None | Children::Caption | Children::Ime => Ok(()),
        Children::Controls => create_controls(hwnd, hinst),
        Children::Password => create_password_edit(hwnd, hinst),
    };
    if let Err(e) = made {
        // 最前面の親を残さない。GetLastError は子を作る関数の中で DestroyWindow より先に読んである
        // SAFETY: hwnd はこのスレッドが作った有効なウィンドウで、DestroyWindow は作ったスレッドから呼ぶ必要がある。
        // 作れた子があれば、親と一緒に壊れる。
        unsafe { DestroyWindow(hwnd) };
        let _ = tx.send(Err(e));
        return;
    }
    // 呼び出し側へ返す前に印を付ける (返った直後のドラッグから帯が効く)。外すのは WM_DESTROY
    if children == Children::Caption {
        caption_windows().push(hwnd as usize);
    }
    if tx.send(Ok(hwnd as usize)).is_err() {
        // 呼び出し側が待つのをやめた (タイムアウト)。最前面のウィンドウを残さないよう、作ったスレッドで壊す
        // SAFETY: hwnd はこのスレッドが作った有効なウィンドウで、DestroyWindow は作ったスレッドから呼ぶ必要がある。
        unsafe { DestroyWindow(hwnd) };
        return;
    }
    let mut msg = MSG::default();
    loop {
        // SAFETY: msg は書き込み可能な MSG。このスレッドのすべてのウィンドウのメッセージを受け取る。
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break;
        }
        // SAFETY: GetMessageW が埋めた有効な MSG を渡す。
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 窓の矩形 (スクリーン座標、`GetWindowRect`)。
fn screen_rect_of(hwnd: HWND) -> Result<Rect, String> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: 窓のハンドルと、書き込み可能な RECT を渡す。無効なハンドルなら 0 が返るだけ。
    let ok = unsafe { GetWindowRect(hwnd, &mut r) };
    if ok == 0 {
        return Err("GetWindowRect が失敗".into());
    }
    Ok(Rect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    ))
}

impl FixtureWindow {
    /// フィクスチャを開く。DPI 認識の宣言 (`dpi::ensure_per_monitor_v2`) を先に済ませておくこと。
    pub fn open(rect: Rect, title: &str) -> Result<FixtureWindow, String> {
        Self::open_impl(rect, title, Children::None)
    }

    /// 子 (BUTTON と読み取り専用の複数行の EDIT) つきのフィクスチャを開く (UIA の実機テスト用。D-088)。
    /// 子の位置は `FIXTURE_BUTTON_RECT` と `FIXTURE_EDIT_RECT` (クライアント座標)。子のメッセージは記録しない。
    /// キーの記録は親にフォーカスがあるときだけ (子をクリックするとフォーカスが子に移り、子へのキーは記録されない)。子の IME は外していない (`ImmAssociateContextEx` は `IACE_CHILDREN` なし)。
    /// DPI 認識の宣言を先に済ませておくのは `open` と同じ。
    pub fn open_with_controls(rect: Rect, title: &str) -> Result<FixtureWindow, String> {
        Self::open_impl(rect, title, Children::Controls)
    }

    /// `ES_PASSWORD | ES_AUTOHSCROLL` の EDIT (ID `FIXTURE_PASSWORD_ID`、初期の文字列 `FIXTURE_PASSWORD_TEXT`、位置は
    /// `FIXTURE_PASSWORD_RECT`) を 1 つ子に持つフィクスチャを開く (パスワード欄の実機テスト用。D-090)。
    /// 子のメッセージは記録しない。このフィクスチャだけ、親の空いた所の左クリックでフォーカスを親に戻す。
    /// DPI 認識の宣言を先に済ませておくのは `open` と同じ。
    pub fn open_with_password(rect: Rect, title: &str) -> Result<FixtureWindow, String> {
        Self::open_impl(rect, title, Children::Password)
    }

    /// 窓の上端から `FIXTURE_CAPTION_HEIGHT` までの帯を掴むと窓が動くフィクスチャを開く (タイトルバーの代わり。D-132)。
    /// 帯の押下は非クライアントのメッセージになるので、`events` には記録されない。
    /// DPI 認識の宣言を先に済ませておくのは `open` と同じ。
    pub fn open_with_caption(rect: Rect, title: &str) -> Result<FixtureWindow, String> {
        Self::open_impl(rect, title, Children::Caption)
    }

    /// IME を外さないフィクスチャを開く (IME がオンのときの `type` の測定用。D-133)。子を持たず、`WM_IME_*` も
    /// `events` に記録する。IME の開け閉めは `set_ime_open`。DPI 認識の宣言を先に済ませておくのは `open` と同じ。
    pub fn open_with_ime(rect: Rect, title: &str) -> Result<FixtureWindow, String> {
        Self::open_impl(rect, title, Children::Ime)
    }

    fn open_impl(rect: Rect, title: &str, children: Children) -> Result<FixtureWindow, String> {
        // 容量 0 (ランデブー) にする。受け手が実際に受け取ったときだけ send が成功するので、
        // タイムアウトで受け手が去った後に作られたウィンドウは、作ったスレッドが必ず壊す
        let (tx, rx) = mpsc::sync_channel(0);
        let title = title.to_string();
        let thread = std::thread::spawn(move || window_thread(rect, title, children, tx));
        let hwnd = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| format!("フィクスチャのウィンドウが作られない: {e}"))??;
        Ok(FixtureWindow {
            hwnd,
            thread: Some(thread),
            alive: Arc::new(Mutex::new(true)),
        })
    }

    pub fn hwnd(&self) -> usize {
        self.hwnd
    }

    fn handle(&self) -> HWND {
        self.hwnd as HWND
    }

    /// ウィンドウ全体の矩形 (スクリーン座標)。
    pub fn window_rect(&self) -> Result<Rect, String> {
        screen_rect_of(self.handle())
    }

    /// 子つきのフィクスチャ (`open_with_controls`) の子の窓の矩形 (スクリーン座標、`GetWindowRect`)。
    /// `id` は `FIXTURE_BUTTON_ID` か `FIXTURE_EDIT_ID`。その ID の子が無ければ `Err` (D-088)。
    pub fn child_rect(&self, id: i32) -> Result<Rect, String> {
        // SAFETY: このフィクスチャのウィンドウのハンドルと整数の ID を渡すだけ。無ければ null が返り、メモリは参照しない。
        let child = unsafe { GetDlgItem(self.handle(), id) };
        if child.is_null() {
            return Err(format!(
                "ID {id} の子が無い (GetDlgItem が失敗。GetLastError={})",
                super::last_error()
            ));
        }
        screen_rect_of(child)
    }

    /// パスワード欄つきのフィクスチャ (`open_with_password`) の子のエディットの文字列を、同じプロセスから
    /// `WM_GETTEXT` で読む (テスト用。打った文字が届いたかを見る。D-090)。子が無ければ `Err`。
    /// `ES_PASSWORD` のエディットは別のプロセスからの `WM_GETTEXT` を断るが、同じプロセスからは返す。
    pub fn password_edit_text(&self) -> Result<String, String> {
        // SAFETY: このフィクスチャのウィンドウのハンドルと整数の ID を渡すだけ。無ければ null が返り、メモリは参照しない。
        let child = unsafe { GetDlgItem(self.handle(), FIXTURE_PASSWORD_ID) };
        if child.is_null() {
            return Err(format!(
                "ID {FIXTURE_PASSWORD_ID} の子が無い (GetDlgItem が失敗。GetLastError={})",
                super::last_error()
            ));
        }
        // SAFETY: 子の窓のハンドルに WM_GETTEXTLENGTH を送るだけ (wParam と lParam は 0)。窓は別のスレッドが持ち、
        //         そのスレッドはメッセージループを回しているので、処理されて戻る。
        let len = unsafe { SendMessageW(child, WM_GETTEXTLENGTH, 0, 0) };
        let cap = usize::try_from(len).unwrap_or(0) + 1;
        let mut buf = vec![0u16; cap];
        // SAFETY: buf は cap 個の u16 の書き込み可能な領域で、送り終わるまで生きている (SendMessageW は処理が終わる
        //         まで戻らない)。wParam に終端の NUL を含む容量を渡すので、エディットはその範囲だけに書く。
        let copied = unsafe { SendMessageW(child, WM_GETTEXT, cap, buf.as_mut_ptr() as LPARAM) };
        let n = usize::try_from(copied).unwrap_or(0).min(cap - 1);
        Ok(String::from_utf16_lossy(&buf[..n]))
    }

    /// クライアント領域の矩形 (スクリーン座標)。入力と撮影はこの中だけに限る。
    pub fn client_rect(&self) -> Result<Rect, String> {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: このフィクスチャのウィンドウのハンドルと、書き込み可能な RECT を渡す。
        let ok = unsafe { GetClientRect(self.handle(), &mut r) };
        if ok == 0 {
            return Err("GetClientRect が失敗".into());
        }
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: このフィクスチャのウィンドウのハンドルと、書き込み可能な POINT を渡す。
        let ok = unsafe { ClientToScreen(self.handle(), &mut origin) };
        if ok == 0 {
            return Err("ClientToScreen が失敗".into());
        }
        Ok(Rect::new(
            origin.x,
            origin.y,
            r.right.max(0) as u32,
            r.bottom.max(0) as u32,
        ))
    }

    /// スクリーン座標をクライアント座標にする。
    pub fn to_client(&self, p: Point) -> Result<Point, String> {
        let c = self.client_rect()?;
        Ok(Point::new(p.x - c.x, p.y - c.y))
    }

    /// `SetForegroundWindow` を試すだけ (D-020)。前面にできない条件では何も起きない。
    pub fn try_set_foreground(&self) -> bool {
        // SAFETY: このフィクスチャのウィンドウのハンドルを渡すだけ。
        unsafe { SetForegroundWindow(self.handle()) != 0 }
    }

    /// フィクスチャを SW_HIDE で隠す (タスクトレイに隠した窓を模す実機テスト用。D-074)。窓は別のスレッドが持つので、
    /// Microsoft Learn の勧めどおり `ShowWindowAsync` で置き、見えなくなるまで最大 1 秒待つ (隠れたかは
    /// 呼び出し側が `is_visible` で確かめる)。隠すのは自分のプロセスの窓だけで、入力は送らない。
    /// SW_HIDE は、隠した窓が前面なら前面を他の窓へ移しうる (前面を見るテストの後に呼ぶ)。
    pub fn hide(&self) {
        // SAFETY: このフィクスチャのウィンドウのハンドルと表示の定数を渡すだけ。持ち主のスレッドのキューに
        // 表示の変更を置いて待たずに戻る。既に無ければ失敗するだけで、メモリは参照しない。
        unsafe { ShowWindowAsync(self.handle(), SW_HIDE) };
        let start = std::time::Instant::now();
        while self.is_visible() && start.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `on` が真なら、WM_CLOSE を受けると窓を壊さず SW_HIDE で隠すだけにする (トレイに入るアプリを模す実機テスト用。
    /// D-096)。偽なら既定 (WM_CLOSE で壊れる) に戻す。Drop は自分で既定に戻してから閉じる。入力は送らない。
    pub fn set_hide_on_close(&self, on: bool) {
        // 毒されていても中身を使う (黙って何もしないと、テストが「hidden が false」の分かりにくい落ち方をする)
        let mut v = HIDE_ON_CLOSE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|h| *h != self.hwnd);
        if on {
            v.push(self.hwnd);
        }
    }

    /// フィクスチャに表示のアフィニティ (`WDA_NONE`、`WDA_EXCLUDEFROMCAPTURE` など) を付ける (D-077 の実機テスト用)。
    /// 戻り値は OS が受け付けたか。付けるのは自分のプロセスの窓だけで、入力は送らない。
    pub fn set_display_affinity(&self, affinity: u32) -> bool {
        // SAFETY: このフィクスチャ (このプロセスの窓) のハンドルと定数を渡すだけ。文書の条件はトップレベルで現在の
        // プロセスの窓であることだけで、窓を作ったスレッドから呼ぶ必要は無い。無効なハンドルなら 0 が返り、
        // メモリは参照しない。
        unsafe { SetWindowDisplayAffinity(self.handle(), affinity) != 0 }
    }

    /// フィクスチャに窓のリージョンを付ける (`FixtureRegion::Clear` なら外す。D-112 の実機テスト用)。戻り値は OS が
    /// 受け付けたか。`Rect` の座標は窓の左上 (`GetWindowRect`) を原点にした座標、`Ellipse` は窓全体に内接する楕円
    /// (窓の幅と高さは `GetWindowRect` で読む。読めなければ偽)。付けるのは自分のプロセスの窓だけで、入力は送らない。
    /// `SetWindowRgn` は窓に `WM_WINDOWPOSCHANGING` / `WM_WINDOWPOSCHANGED` を送るので、窓のスレッドが `stall` で
    /// 眠っている間は戻らない (眠らせている間に呼ばない)
    pub fn set_region(&self, region: FixtureRegion) -> bool {
        let rgn: HRGN = match region {
            FixtureRegion::Clear => std::ptr::null_mut(),
            FixtureRegion::Rect(r) => {
                let (Some(right), Some(bottom)) = (
                    i32::try_from(r.w).ok().and_then(|w| r.x.checked_add(w)),
                    i32::try_from(r.h).ok().and_then(|h| r.y.checked_add(h)),
                ) else {
                    return false;
                };
                // SAFETY: 整数の 4 値を渡すだけ。失敗したら null が返る (下で見る)。
                unsafe { CreateRectRgn(r.x, r.y, right, bottom) }
            }
            FixtureRegion::Ellipse => {
                let Ok(w) = self.window_rect() else {
                    return false;
                };
                let (Ok(ww), Ok(wh)) = (i32::try_from(w.w), i32::try_from(w.h)) else {
                    return false;
                };
                // SAFETY: 整数の 4 値 (窓の左上を原点にした外接矩形) を渡すだけ。失敗したら null が返る (下で見る)。
                unsafe { CreateEllipticRgn(0, 0, ww, wh) }
            }
        };
        if rgn.is_null() && !matches!(region, FixtureRegion::Clear) {
            return false;
        }
        // SAFETY: このフィクスチャ (このプロセスのトップレベルの窓) のハンドルと、上で作ったリージョン (Clear なら
        // null。null はリージョンを外す) を渡す。窓を作ったスレッドはメッセージループを回しているので、送られる
        // WM_WINDOWPOSCHANGING / WM_WINDOWPOSCHANGED は処理されて戻る。成功したらリージョンの持ち主は OS になり、
        // ここでは消さない (文書: 成功の後に消してはならない)。
        let ok = unsafe { SetWindowRgn(self.handle(), rgn, 1) } != 0;
        if !ok && !rgn.is_null() {
            // SAFETY: 上で作った有効なリージョンを 1 回だけ消す。文書が所有の移動を書くのは成功の後だけで、失敗時の
            // 扱いは明記が無い。持ち主は移らないと読む (通例)。
            unsafe { DeleteObject(rgn as HGDIOBJ) };
        }
        ok
    }

    /// フィクスチャを DWM で cloak する (`on` が偽なら戻す。D-099 の実機テスト用)。`IsWindowVisible` は真のまま、
    /// 画面には描かれない (別の仮想デスクトップの窓と同じく `DWMWA_CLOAKED` が非 0 になる)。戻り値は OS が
    /// 受け付けたか (HRESULT が成功)。付けるのは自分のプロセスの窓だけで、入力は送らない。
    pub fn set_cloak(&self, on: bool) -> bool {
        let value: windows_sys::core::BOOL = i32::from(on);
        // SAFETY: このフィクスチャ (このプロセスのトップレベルの窓) のハンドルと、BOOL の大きさの読み取り専用の
        // 領域とその大きさを渡す。DWMWA_CLOAK は BOOL を読むだけで、呼び出しの間 value は生きている。無効な
        // ハンドルなら失敗の HRESULT が返り、メモリは参照しない。
        let hr = unsafe {
            DwmSetWindowAttribute(
                self.handle(),
                DWMWA_CLOAK as u32,
                (&value as *const windows_sys::core::BOOL).cast(),
                std::mem::size_of::<windows_sys::core::BOOL>() as u32,
            )
        };
        hr >= 0
    }

    /// フィクスチャの IME を開け閉めし、読み戻した状態を返す (`open_with_ime` の測定用。D-133)。入力は送らない。
    /// 入力コンテキストが無い、窓が固まっている、2 秒で返らないときは `None`。処理は窓のスレッドで行う
    pub fn set_ime_open(&self, open: bool) -> Option<bool> {
        let mut result = 0usize;
        // SAFETY: このフィクスチャのウィンドウのハンドルに、整数だけを運ぶ自前のメッセージを送る。result は書き込み
        //         可能な usize で、呼び出しの間生きている。窓が固まっていれば待たずに 0 が返り、メモリは参照しない。
        let sent = unsafe {
            SendMessageTimeoutW(
                self.handle(),
                WM_FIXTURE_IME_OPEN,
                usize::from(open),
                0,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            )
        };
        if sent == 0 || result as LRESULT == -1 {
            return None;
        }
        Some(result != 0)
    }

    /// フィクスチャが見えているか (`IsWindowVisible`。読み取りだけ)。
    pub fn is_visible(&self) -> bool {
        // SAFETY: このフィクスチャのウィンドウのハンドルを渡すだけ。無効なハンドルなら 0 が返る。
        unsafe { IsWindowVisible(self.handle()) != 0 }
    }

    /// 別のスレッドからこのフィクスチャを眠らせる値を返す (生存の印を共有する。D-108)
    pub fn staller(&self) -> FixtureStaller {
        FixtureStaller {
            hwnd: self.hwnd,
            alive: Arc::clone(&self.alive),
        }
    }

    /// フィクスチャの窓のスレッドを `dur` だけ眠らせる。`FixtureStaller::stall` と同じ (本体と詳しい説明はそちら。
    /// D-105、D-108)
    pub fn stall(&self, dur: Duration) -> bool {
        self.staller().stall(dur)
    }

    /// フィクスチャの窓のスレッドが `stall` で眠っているか。`FixtureStaller::is_stalled` と同じ (本体はそちら。
    /// D-105、D-108)
    pub fn is_stalled(&self) -> bool {
        self.staller().is_stalled()
    }

    pub fn events(&self) -> Vec<FixtureEvent> {
        EVENTS
            .lock()
            .map(|v| {
                v.iter()
                    .filter(|(h, _)| *h == self.hwnd)
                    .map(|(_, e)| e.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn clear_events(&self) {
        if let Ok(mut v) = EVENTS.lock() {
            v.retain(|(h, _)| *h != self.hwnd);
        }
    }
}

impl Drop for FixtureWindow {
    fn drop(&mut self) {
        // 閉じる前に生存の印を偽にする (残った `FixtureStaller` が閉じた後の窓に置かないように。D-108)
        // 同じロックを取るので、`stall` が確かめてから置くまでの途中とは重ならない (置き終わってから偽にする)
        *lock_alive(&self.alive) = false;
        // 隠れるだけの設定を先に外す (外さないと WM_CLOSE で隠れるだけで、メッセージループが終わらず join が返らない)
        self.set_hide_on_close(false);
        // SAFETY: このフィクスチャのウィンドウに WM_CLOSE を送るだけ (既に無ければ失敗するだけ)。
        unsafe { PostMessageW(self.handle(), WM_CLOSE, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elements::normalize_newlines;

    // 窓は開かない。子の定数の形だけを見る (クライアント領域に収まることは tests/fixture_real.rs の実機テストで見る)

    #[test]
    fn fixture_child_rects_do_not_overlap() {
        assert!(
            !FIXTURE_BUTTON_RECT.intersects(&FIXTURE_EDIT_RECT),
            "button={FIXTURE_BUTTON_RECT:?} edit={FIXTURE_EDIT_RECT:?}"
        );
    }

    /// パスワード欄はクライアント領域 (640x400 の窓の内側) の左上に収まり、右と下に親の空いた所が残る (D-090)
    #[test]
    fn fixture_password_rect_leaves_parent_space() {
        let client = Rect::new(0, 0, 600, 360);
        assert!(
            client.contains_rect(&FIXTURE_PASSWORD_RECT),
            "{FIXTURE_PASSWORD_RECT:?}"
        );
        assert!(!FIXTURE_PASSWORD_RECT.contains(Point::new(400, 300)));
        assert_eq!(FIXTURE_PASSWORD_TEXT.chars().count(), 12);
    }

    /// 帯は窓の上端から高さの手前まで。窓より上の点は帯に入らず、上端が負の座標でも同じ (D-132)
    #[test]
    fn in_caption_band_covers_only_the_top_strip() {
        assert!(in_caption_band(100, 100));
        assert!(in_caption_band(100 + FIXTURE_CAPTION_HEIGHT - 1, 100));
        assert!(!in_caption_band(100 + FIXTURE_CAPTION_HEIGHT, 100));
        assert!(!in_caption_band(99, 100));
        assert!(in_caption_band(-90, -100));
    }

    /// 眠りを置いてよいのは、生存の印が真で、持ち主が自分の pid (0 でない) のときだけ (D-108)
    #[test]
    fn may_post_requires_alive_and_own_pid() {
        let me = std::process::id();
        assert!(may_post(true, me, me));
        assert!(!may_post(false, me, me));
        assert!(!may_post(true, me + 1, me));
        assert!(!may_post(true, 0, me));
        // pid 0 は読めない印なので、0 同士でも置かない
        assert!(!may_post(true, 0, 0));
    }

    #[test]
    fn fixture_edit_text_has_crlf_and_two_lines() {
        assert!(FIXTURE_EDIT_TEXT.contains("\r\n"));
        assert_eq!(
            normalize_newlines(FIXTURE_EDIT_TEXT),
            "一行目 fixture\n二行目 テキスト"
        );
    }
}
