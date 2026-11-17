//! 入力と観測の抽象 (trait)。実装は `backend` (Win32) と `testing` (モック)。
//! 論理 (計画、実行、ガード、停止) はこの trait 越しにだけ装置へ触れる (D-006)。

use std::fmt;

use serde::Serialize;

use crate::geometry::{Point, Rect};
use crate::script::Button;

/// 装置へ送る 1 つの入力。座標は物理ピクセルのスクリーン座標。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InputEvent {
    MoveTo {
        x: i32,
        y: i32,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// `delta` は WHEEL_DELTA (120) 単位の値をそのまま
    Wheel {
        delta: i32,
        horizontal: bool,
    },
    Key {
        vk: u16,
        down: bool,
        extended: bool,
    },
    /// UTF-16 の 1 単位 (`KEYEVENTF_UNICODE`)
    Unicode {
        unit: u16,
        down: bool,
    },
}

impl InputEvent {
    pub fn move_to(p: Point) -> Self {
        InputEvent::MoveTo { x: p.x, y: p.y }
    }

    /// 押下を解く方向のイベント (緊急停止の後始末で、範囲の確認なしに送ってよいもの) か。
    pub fn is_release(&self) -> bool {
        matches!(
            self,
            InputEvent::Button { down: false, .. }
                | InputEvent::Key { down: false, .. }
                | InputEvent::Unicode { down: false, .. }
        )
    }

    pub fn is_keyboard(&self) -> bool {
        matches!(self, InputEvent::Key { .. } | InputEvent::Unicode { .. })
    }
}

/// ウィンドウとその持ち主のプロセスの情報 (ガードの判定に使う)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowInfo {
    /// ルートのウィンドウのハンドルの値 (比較と表示にだけ使う)
    pub hwnd: usize,
    pub pid: u32,
    /// 実行ファイル名 (例 `notepad.exe`)。取れなければ空
    pub process: String,
    pub class: String,
    pub title: String,
    /// 管理者権限で動いているか。取れなければ None (ガードは拒否側に倒す)
    pub elevated: Option<bool>,
}

/// トップレベルのウィンドウ (`ai-cu windows` の一覧に使う)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopWindow {
    pub info: WindowInfo,
    /// 見えている枠の矩形 (物理ピクセル。影などの見えない縁を除く)。最小化していれば None
    pub rect: Option<Rect>,
    pub minimized: bool,
}

/// トップレベルのウィンドウの持ち主のプロセス (`windows` の `hidden_allowed` の材料。D-074)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowOwner {
    pub pid: u32,
    /// 実行ファイル名 (例 `example-app.exe`)。取れなければ空
    pub process: String,
}

/// 撮影の判定 (D-077) に使う、Z 順の窓 1 つ。`stack_windows` が前面から奥への順で返す。
/// `info.title` は常に空 (判定はプロセス名とクラスだけで行い、画面上の情報である題名は持たない)。
/// `info.elevated` は読むが、判定には使わない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackWindow {
    pub info: WindowInfo,
    /// 見えている枠の矩形 (物理ピクセル。DWM の枠で、影を除く)
    pub rect: Rect,
    /// 覆いとみなさないか: 下の窓が透けて写りうる (`WS_EX_LAYERED` で、層の属性が読めない、色キー、または alpha が
    /// 255 未満)、撮影から外れる (`WDA_EXCLUDEFROMCAPTURE`)、DWM の枠が取れない (矩形が見えない縁の分だけ大きい)、
    /// リージョン (`SetWindowRgn`) が枠をまるごと覆わない (D-112)
    /// のどれか。真なら覆いとみなさない (断る向き)
    pub see_through: bool,
    /// 応答しない窓か (`IsHungAppWindow`)。ゴーストの窓を deny に数えるかの判定に使う (D-115)
    pub hung: bool,
}

/// 撮影の隠れの判定 (D-142) に使う、覆う窓の性質 (`Desktop::cover_traits`)。設定は見ない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverTraits {
    /// 題名があり、ツールウィンドウ (`WS_EX_TOOLWINDOW`) でない (`top_level_windows_detailed` の列挙に入る形)。
    /// 偽ならツールチップやメニューのような、アプリの付属の表示
    pub plain_top: bool,
    /// 下の窓が透けて写りうる (`StackWindow::see_through` と同じ定義。D-077)
    pub see_through: bool,
}

/// 見えないトップレベルの窓 (`windows` の `hidden_windows`。D-099)。`cloaked` が偽なら `IsWindowVisible` が偽、
/// 真なら見えるが cloaked。cloaked の理由は別の仮想デスクトップに限らず、アプリ自身が隠した (`DWM_CLOAKED_APP`。
/// 中断中のストアのアプリの枠など) や、持ち主から受け継いだ (`DWM_CLOAKED_INHERITED`) もある (理由の値は出さない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HiddenWindow {
    pub info: WindowInfo,
    pub cloaked: bool,
}

/// `close` の後のウィンドウの状態 (`Desktop::window_presence`。D-062、D-096)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPresence {
    /// ハンドルが有効で、持ち主が同じ pid で、見えている
    Visible,
    /// ハンドルが有効で、持ち主が同じ pid のまま見えない (トレイに隠したアプリ)
    Hidden,
    /// ハンドルが無効か、持ち主の pid が違う (ハンドルが別のウィンドウに再利用された)
    Destroyed,
}

/// 窓の大きさの読み (`Desktop::window_geometry`。`resize` が使う。D-150)。矩形は物理ピクセルのスクリーン座標
/// (DPI 非対応の窓の `client` も物理で返ることを、倍率 1.25 の画面の実機テストで測った。D-150、docs/design-notes.md)。
/// 最小化は `find_window` の `TopWindow::minimized` で先に見て、送る直前にここの `minimized` でもう一度見る (2 重の確認)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowGeometry {
    /// `GetWindowRect` の外側の矩形 (見えない縁を含む)。`SetWindowPos` に渡す大きさはこれの基準
    pub outer: Rect,
    /// DWM の見えている枠 (`describe` と同じ読み)。取れなければ `outer`
    pub frame: Rect,
    /// 中身 (client area) を画面座標に直したもの (`GetClientRect` を `MapWindowPoints` で RECT として写す)
    pub client: Rect,
    /// 最大化しているか (`IsZoomed`)
    pub maximized: bool,
    /// 窓が主に載っている画面の有効 DPI (`MonitorFromWindow` + `GetDpiForMonitor(MDT_EFFECTIVE_DPI)`)。読めなければ None
    pub dpi: Option<u32>,
    /// 最小化しているか (`IsIconic`)。`find_window` の後に最小化された窓へ送らないために読む
    pub minimized: bool,
    /// 応答しない窓か (`IsHungAppWindow`)。要求が列に残って後から効くので、真なら送らない
    pub hung: bool,
}

/// クリップボードに文字を置いた結果 (`Desktop::set_clipboard_text`。D-157)。開く前の失敗 (形式の登録、置き場の確保、
/// 持ち主の窓) と、開いた後の `EmptyClipboard` 自身の失敗は `Err(BackendError::Os)` で、中身は変わらない
/// (`EmptyClipboard` の失敗で中身が残るかは文書に無いので、変わっていない扱いにする。推定)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardSet {
    /// 文と履歴・同期から外す印を置いた
    Set,
    /// 他のアプリが開いていて `OpenClipboard` が失敗した (中身は変わらない。試し直しは実行器が行う)。値はその失敗の
    /// `GetLastError` (実行器が試し直しを諦めたときの 8 の文に最後の値を添える)
    Busy(u32),
    /// `EmptyClipboard` の後に失敗し、空にして閉じた (人の元の中身は失われた)。値は理由
    Emptied(String),
}

/// RGBA 8 ビットの画像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// 開発シェルでの実入力の拒否 (D-022)
    Forbidden(String),
    /// フィクスチャ専用の装置で、範囲外への送信を拒否した (D-020)
    OutOfScope(String),
    /// OS の呼び出しの失敗
    Os(String),
    /// 入力デスクトップが使えない (画面のロック、UAC など。`GetCursorPos` と GetClipCursor (D-151) の `ERROR_ACCESS_DENIED`。D-066)
    DesktopUnavailable(String),
    /// OS の呼び出しの時間切れ (UIA の読み取りの時間切れ、返らないスレッドの上限、UIA 自身の時間切れ (どの HRESULT かは src/win32/uia.rs の `is_uia_timeout`。フォーカスの判定と、要素と木の読み。要素と木の読みは最初の時間切れで残りを読まずに返る (D-110))。D-104)。8 (`os_error`) に写す
    TimedOut(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::Forbidden(m) => write!(f, "実入力を拒否: {m}"),
            BackendError::OutOfScope(m) => write!(f, "フィクスチャの範囲外: {m}"),
            BackendError::Os(m) => write!(f, "OS の呼び出しに失敗: {m}"),
            BackendError::DesktopUnavailable(m) => write!(f, "入力デスクトップが使えない: {m}"),
            BackendError::TimedOut(m) => write!(f, "OS の呼び出しが時間切れ: {m}"),
        }
    }
}

/// 前面化の補助プロセスの結果 (D-057)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperOutcome {
    /// 子の `SetForegroundWindow` が 1 を返した (前面になったかは呼び出し側が確かめる)
    Accepted,
    /// 子の `SetForegroundWindow` が 0 を返した
    Refused,
    /// 制限時間を超えた (子は kill した)
    TimedOut,
    /// 起動できなかった、または子が想定外の終わり方をした
    Failed(String),
}

impl HelperOutcome {
    /// `error.detail` の `helper_result` に入れる名前
    pub fn label(&self) -> &'static str {
        match self {
            HelperOutcome::Accepted => "accepted",
            HelperOutcome::Refused => "refused",
            HelperOutcome::TimedOut => "timeout",
            HelperOutcome::Failed(_) => "failed",
        }
    }
}

/// 入力デスクトップの状態 (D-066)。読むだけ
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputDesktop {
    /// 自分のデスクトップ (Default) が入力を受けている
    Available,
    /// スクリーンセーバーが動いている (name は読めた入力デスクトップの名前)
    ScreenSaver { name: Option<String> },
    /// WTS のセッションのロック (D-067)。name は読めた入力デスクトップの名前 (ロックの幕 (LockApp) の間は
    /// `Default` と推定、資格情報の画面なら `Winlogon`、開けなければ None)
    Locked { name: Option<String> },
    /// 別のデスクトップ (WTS で読めなかったロック、UAC、Ctrl+Alt+Del の画面) か、開けない
    Other { name: Option<String> },
}

/// 押されたままの修飾キーとマウスボタン (D-066)。名前はキーの書式とボタンの名前
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldInputs {
    pub keys: Vec<&'static str>,
    pub buttons: Vec<&'static str>,
}

impl HeldInputs {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }
}

/// `OpenInputDesktop` の結果 (D-066)。Win32 の層が読み、振り分けは `classify_input_desktop` が行う
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenResult {
    /// 開けた。名前は `GetUserObjectInformationW(UOI_NAME)` で読めたもの (読めなければ None)
    Opened(Option<String>),
    /// `ERROR_ACCESS_DENIED` (5) で開けない (ロックや UAC の画面が前面のことが多い)
    AccessDenied,
    /// 5 以外で開けない (メッセージ)
    Failed(String),
}

/// 入力デスクトップの判定の材料 (生の値。D-066、D-067)。Win32 の層が集め、振り分けは
/// `classify_input_desktop` が行う。構造体ごと渡すので、backend の配線で値を取り違えない
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDesktopRaw {
    /// `SPI_GETSCREENSAVERRUNNING` が真 (読めなければ偽として扱う)
    pub saver_running: bool,
    /// WTS の SessionFlags で読んだセッションのロック。`Some(true)` はロック、`Some(false)` は非ロック、
    /// None は読めない (失敗か UNKNOWN。非ロックとして残りの判定に任せる)
    pub session_locked: Option<bool>,
    /// `OpenInputDesktop` と名前の読み取りの結果
    pub open: OpenResult,
}

/// 入力デスクトップの振り分け (純関数。D-066、D-067 の規則は 1 つ)。順は セーバー → ロック → 名前。
/// セーバーが動いていれば、開けた結果を問わず `ScreenSaver` (開けて読めた名前は入れる)。
/// そうでなく `session_locked` が `Some(true)` なら、開けた結果を問わず `Locked` (読めた名前は入れる。
/// ロックと読めたので、開けなくても Err にしない)。
/// どちらでもなければ、名前が `Default` (大文字小文字を無視) なら `Available`、別の名前か 5 なら `Other`。
/// 5 以外で開けないか、開けても名前を読めなければ `Err` (8。判定が読めないのを「ロック」と決めつけない)
pub fn classify_input_desktop(raw: InputDesktopRaw) -> Result<InputDesktop, String> {
    let InputDesktopRaw {
        saver_running,
        session_locked,
        open,
    } = raw;
    let opened_name = |open: OpenResult| match open {
        OpenResult::Opened(n) => n,
        OpenResult::AccessDenied | OpenResult::Failed(_) => None,
    };
    if saver_running {
        return Ok(InputDesktop::ScreenSaver {
            name: opened_name(open),
        });
    }
    if session_locked == Some(true) {
        return Ok(InputDesktop::Locked {
            name: opened_name(open),
        });
    }
    match open {
        OpenResult::Opened(Some(n)) if n.eq_ignore_ascii_case("Default") => {
            Ok(InputDesktop::Available)
        }
        OpenResult::Opened(Some(n)) => Ok(InputDesktop::Other { name: Some(n) }),
        OpenResult::AccessDenied => Ok(InputDesktop::Other { name: None }),
        OpenResult::Opened(None) => {
            Err("入力デスクトップを開けたが、名前 (UOI_NAME) を読めない".to_string())
        }
        OpenResult::Failed(m) => Err(m),
    }
}

/// 入力を送る先。1 回の `send` は 1 回の `SendInput` (原子的に挿入される) に対応する。
pub trait InputSink {
    fn send(&mut self, events: &[InputEvent]) -> Result<(), BackendError>;
}

/// 画面とウィンドウの観測。
pub trait Desktop {
    /// 仮想デスクトップ全体の矩形 (原点は負になりうる)
    fn virtual_screen(&self) -> Result<Rect, BackendError>;
    fn cursor_pos(&self) -> Result<Point, BackendError>;
    /// 点がどれかのモニターの上か (`MonitorFromPoint` の `MONITOR_DEFAULTTONULL`。モニターの間の隙間は偽。D-151)。
    /// 失敗の形は無いが、口は他の読みと同じく Result にそろえる
    fn on_monitor(&self, p: Point) -> Result<bool, BackendError>;
    /// カーソルの動く範囲 (`GetClipCursor`)。絞られていなければ `None` (読んだ矩形が仮想画面と同じときだけ
    /// 絞りなしとみなす。`backend::confined_clip`。D-151)。失敗の写しは `cursor_pos` と同じ
    /// (5 は `DesktopUnavailable`、ほかは `Os`)
    fn cursor_clip(&self) -> Result<Option<Rect>, BackendError>;
    fn foreground_window(&self) -> Result<Option<WindowInfo>, BackendError>;
    /// 座標の下のウィンドウ (ルートの親)
    fn window_at(&self, p: Point) -> Result<Option<WindowInfo>, BackendError>;
    fn capture(&self, rect: Rect) -> Result<Image, BackendError>;
    /// 見えているトップレベルのウィンドウ (前面から奥へ。最小化を含む)
    fn top_windows(&self) -> Result<Vec<TopWindow>, BackendError>;
    /// トップレベルのウィンドウ (見えないもの、ツールウィンドウ、cloaked を含む) の持ち主。pid ごとに 1 つ。読み取りだけ (D-074)
    fn window_owners(&self) -> Result<Vec<WindowOwner>, BackendError>;
    /// 見えていて cloaked でなく最小化していないトップレベルの窓すべて (題名の無いもの、ツールウィンドウを含む)。
    /// 前面から奥へ。題名は読まない (`info.title` は空)。撮影の判定 (D-077) に使う。読み取りだけ
    fn stack_windows(&self) -> Result<Vec<StackWindow>, BackendError>;
    /// 見えないトップレベルの窓 (`IsWindowVisible` が偽か、見えるが cloaked)。前面から奥へ (Z 順)。許可で絞らない
    /// (絞るのは呼ぶ側)。読み取りだけ (D-099)
    fn hidden_windows(&self) -> Result<Vec<HiddenWindow>, BackendError>;
    /// 点がウィンドウの題名の帯 (ボタンやタブではない、ドラッグで動かせる部分) か
    fn is_caption(&self, hwnd: usize, p: Point) -> bool;
    /// このプロセス自身が管理者権限で動いているか
    fn self_elevated(&self) -> bool;
    /// セッションの最後の入力 (人の操作。このプロセスの SendInput も含む) からの経過 (ミリ秒。D-049)
    fn idle_ms(&self) -> Result<u64, BackendError>;
    /// 入力デスクトップの状態 (ロック、セーバー、UAC など。D-066)。読むだけ
    fn input_desktop(&self) -> Result<InputDesktop, BackendError>;
    /// 押されたままの修飾キーとマウスボタン (D-066)。読むだけで、離すことはしない
    fn keys_held(&self) -> Result<HeldInputs, BackendError>;
    /// 実行ファイル名 (正規化して比べる) と、あれば題名の一部で、見えているトップレベルのウィンドウを
    /// 前面から探す (D-050)。ポップアップらしくない窓の中で最初に一致したもの、無ければポップアップらしい窓
    /// (ツールチップなど) の中で最初に一致したもの (D-143)。読み取りだけ。
    /// `admit` を通らない窓は無いものとして扱う (探してから捨てるのではなく、通る窓の中で最初の一致。D-139)
    fn find_window(
        &self,
        process: &str,
        title: Option<&str>,
        admit: &dyn Fn(&WindowInfo) -> bool,
    ) -> Result<Option<TopWindow>, BackendError>;
    /// `SetForegroundWindow` を 1 回呼ぶ (D-050)。戻り値は OS が受け付けたかで、前面になったかは
    /// `foreground_window` で別に確かめる。復元や回避策はしない
    fn set_foreground(&self, hwnd: usize) -> Result<bool, BackendError>;
    /// 使い捨ての補助プロセス (自分の実行ファイルの隠しのサブコマンド) に、`SetForegroundWindow` を 1 回だけ
    /// 呼ばせる (D-057)。常駐の MCP サーバーが前面化の権利を失っていて、前面がホストのときにだけ使う。
    /// 起動の失敗や時間切れも結果として返す (前面化の失敗の経路なので、8 にはしない)
    fn set_foreground_by_helper(&self, hwnd: usize) -> Result<HelperOutcome, BackendError>;
    /// `WM_CLOSE` を 1 回置く (D-062)。戻り値は置けたか。入力は送らず、前面も変えない
    fn post_close(&self, hwnd: usize) -> Result<bool, BackendError>;
    /// 窓の大きさの読み (外側、DWM の枠、中身、最大化、画面の DPI。D-150)。窓が無ければ `Ok(None)` (呼ぶ側は 12)。
    /// 窓はあるのに読めない (API の失敗や壊れた矩形) なら `Err(BackendError::Os)` (8。「窓が無い」と混ぜない)。読み取りだけ
    fn window_geometry(&self, hwnd: usize) -> Result<Option<WindowGeometry>, BackendError>;
    /// 窓の外側 (`GetWindowRect` の基準) の大きさを `outer_w` x `outer_h` にする要求を 1 回置く (D-150)。左上は動かさない。
    /// 戻り値は要求を置けたか (届いたかは `window_geometry` で読み直す)。窓が無い (無効なハンドル) なら `Ok(false)`
    /// (呼ぶ側は 12)、それ以外の OS の失敗は `Err(BackendError::Os)` (8)。入力は送らず、前面も Z 順も変えない
    fn set_window_size(
        &self,
        hwnd: usize,
        outer_w: u32,
        outer_h: u32,
    ) -> Result<bool, BackendError>;
    /// クリップボードに `text` (改行は呼ぶ側が CRLF にそろえたもの) を `CF_UNICODETEXT` で置き、Win+V の履歴と
    /// クラウドの同期から外す印を付ける (D-157)。入力は送らず、前面も Z 順も変えない。結果の形は `ClipboardSet`
    fn set_clipboard_text(&self, text: &str) -> Result<ClipboardSet, BackendError>;
    /// ハンドルのウィンドウの状態 (`close` の後に消えたかを見る)。`Destroyed` はハンドルが無効か、持ち主の pid が
    /// 違う (再利用された)。`Hidden` は同じ pid のまま見えない (トレイに隠したアプリ)。読み取りだけ。D-062、D-096
    fn window_presence(&self, hwnd: usize, pid: u32) -> Result<WindowPresence, BackendError>;
    /// ウィンドウが無効になっていない (モーダルのダイアログを開いていない) か (D-062)。読み取りだけ
    fn window_enabled(&self, hwnd: usize) -> Result<bool, BackendError>;
    /// 所有者の連鎖の根 (`GA_ROOTOWNER`)。所有者の無いウィンドウなら自分自身。モーダルダイアログが
    /// 対象に属するかの判定に使う (D-050)
    fn owner_root(&self, hwnd: usize) -> usize;
    /// 覆う窓の性質 (題名があり、ツールウィンドウでないか / 透けるか)。撮影の隠れの判定 (D-142) に使う。
    /// 読めなければ (見えない、閉じた、最小化、枠が取れない) None。読み取りだけ
    fn cover_traits(&self, hwnd: usize) -> Option<CoverTraits>;
    /// 窓を作ったスレッドの id (`GetWindowThreadProcessId` の戻り値)。0 (無効なハンドル) なら None。
    /// 撮影の隠れの判定 (D-142) に使う。読み取りだけ
    fn window_thread(&self, hwnd: usize) -> Option<u32>;
    /// UI Automation で、ウィンドウ自身とその子孫の要素を `query` で絞って返す (木の順。D-054)。
    /// 読み取りだけ。名前と文字列は画面上の情報なので、呼び出し側は撮影と同じ許可を求める
    fn ui_elements(
        &self,
        hwnd: usize,
        query: &crate::elements::ElementQuery,
    ) -> Result<crate::elements::Elements, BackendError>;
    /// UI Automation のコントロールビューの木を、ウィンドウ自身の要素 (深さ 0) から前順に、深さ `max_depth` で
    /// 切ってたどる (`elements::walk_tree`。試す要素は `max_nodes` まで。D-084)。読み取りだけ。許可は
    /// `ui_elements` と同じ (呼び出し側は撮影と同じ許可を求める)
    fn ui_tree(
        &self,
        hwnd: usize,
        max_depth: usize,
        max_nodes: usize,
    ) -> Result<crate::elements::UiTree, BackendError>;
    /// システム全体のキーボードフォーカスの要素が、パスワードの要素 (UIA の IsPassword) か。`Ok(None)` は
    /// 読めなかった (フォーカスの要素が無い、IsPassword が読めない)。入力の判定の一部で、画面の情報は返さない
    /// (D-090)。読み取りだけ
    fn focused_is_password(&self) -> Result<Option<bool>, BackendError>;
    /// 点 `p` (物理ピクセルの画面座標) の下の UIA の要素がパスワードの要素か。`Ok(None)` は読めなかった。
    /// 入力の判定の一部で画面の情報は返さない (D-149)。読み取りだけ
    fn point_is_password(&self, p: Point) -> Result<Option<bool>, BackendError>;
}

/// `find_window` の一致の規則 (純関数。実装置もモックもこれを使う)。
/// 実行ファイル名は `allow.processes` と同じ正規化で比べ、`title` は大文字小文字を無視した部分一致。
/// `admit` を通る窓の中で一致するもの (通らない窓は無いものと同じ。D-139) のうち、`popup_like` が偽の Z 順で
/// 最初のもの。無ければ `popup_like` が真の Z 順で最初のもの (ツールチップなどのポップアップを後に回す。外しは
/// しない。D-143)。`popup_like` は一致した窓にだけ、各 1 回まで呼ぶ (走査は 1 回で、ポップアップらしくない一致が
/// 出たところで止まる)
pub fn match_window(
    top: &[TopWindow],
    process: &str,
    title: Option<&str>,
    admit: &dyn Fn(&WindowInfo) -> bool,
    popup_like: &dyn Fn(&TopWindow) -> bool,
) -> Option<TopWindow> {
    let want = crate::config::normalize_process(process);
    let want_title = title.map(str::to_lowercase);
    let mut first_popup: Option<&TopWindow> = None;
    for w in top.iter().filter(|w| {
        admit(&w.info)
            && crate::config::normalize_process(&w.info.process) == want
            && want_title
                .as_deref()
                .is_none_or(|t| w.info.title.to_lowercase().contains(t))
    }) {
        if !popup_like(w) {
            return Some(w.clone());
        }
        first_popup.get_or_insert(w);
    }
    first_popup.cloned()
}

/// 画面の縁の枠と予告の音 (D-093)。実行器が、画面を変える操作の前に `show` し、終わりに `release` する。
/// 枠は知らせる手段で守りの層ではないので、`show` は値を返さない (出せなかったときの警告は実装置が書く)
pub trait Indicator {
    /// いま見えている ai-cu の枠 (自分のも他のプロセスのも) のうち、最も古いものが出てからの経過 (ミリ秒)。
    /// 見えている枠が無ければ None
    fn shown_for_ms(&self) -> Option<u64>;
    /// 枠を出す (出ていれば出したまま)。`deadline_ms` が過ぎたら、主スレッドが固まっていても枠は自分で消える
    fn show(&mut self, deadline_ms: u64);
    /// 実行の終わり。`linger_ms` だけ枠を残してから消す (0 ならすぐ消す)
    fn release(&mut self, linger_ms: u64);
    /// 見えていて解かれた (`release` 済みの) 自分の枠の終わりを、今から `linger_ms` 後まで延ばす
    /// (今の終わりより早めない)。見えていなければ何もしない (新しく出さない)。MCP の呼び出しの間、
    /// 枠を出し続けるのに使う (D-107)。延長は呼ぶたびに最大 `linger_ms` までで、主スレッドが固まれば
    /// 最後の延長から `linger_ms` 後に消える (`linger_ms` は 60 秒以下。`win32::overlay::EXTEND_MAX_MS`)。
    /// 借りる取っ手の転送漏れをコンパイラが止めるよう、既定の実装は置かない
    fn extend(&mut self, linger_ms: u64);
    /// 予告の音を 1 回鳴らす
    fn beep(&mut self);
    /// 枠の状態 (MCP の `status` の `indicator.frame`。D-103)。読むだけで枠を立てない。
    /// 既定は None (枠の装置を持たない)
    fn frame_state(&self) -> Option<FrameState> {
        None
    }
}

/// 枠の状態 (D-103)。`Indicator::frame_state` が返す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameState {
    /// このプロセスでまだ枠のスレッドを立てていない
    NotStarted,
    /// 枠のスレッドが動き、窓の組を持っている
    Running,
    /// 枠のスレッドの起動に失敗した。`retry_in_ms` は次の再試行までの残り (0 なら次の `show` で試す)。
    /// None は、時間切れで切り離した枠のスレッドがまだ動いており、それが片付くまで立て直さないときだけ
    /// (枠のスレッドがひとりでに終わったときは、次の `show` が記録する 60 秒を返す)
    StartFailed {
        error: String,
        retry_in_ms: Option<u64>,
    },
    /// 枠のスレッドは動いているが、作り直しに失敗して窓の組が無い (スレッドが 2 秒ごとに作り直しを試している)
    RebuildFailed { error: String },
}

/// 工場が持ち続ける枠を借りて渡すための実装 (`DeviceFactory::indicator` が `Box::new(&mut self.indicator)` を返す。D-093)
impl<T: Indicator + ?Sized> Indicator for &mut T {
    fn shown_for_ms(&self) -> Option<u64> {
        (**self).shown_for_ms()
    }

    fn show(&mut self, deadline_ms: u64) {
        (**self).show(deadline_ms);
    }

    fn release(&mut self, linger_ms: u64) {
        (**self).release(linger_ms);
    }

    fn extend(&mut self, linger_ms: u64) {
        (**self).extend(linger_ms);
    }

    fn beep(&mut self) {
        (**self).beep();
    }

    fn frame_state(&self) -> Option<FrameState> {
        (**self).frame_state()
    }
}

/// 何もしない枠 (D-093)。`shown_for_ms` は「十分前から出ている」を返すので、予告の間を待たない。
/// 枠の装置を持たない経路 (フィクスチャ限定の実機テスト) に使う
pub struct NoIndicator;

impl Indicator for NoIndicator {
    fn shown_for_ms(&self) -> Option<u64> {
        Some(u64::MAX)
    }

    fn show(&mut self, _deadline_ms: u64) {}

    fn release(&mut self, _linger_ms: u64) {}

    fn extend(&mut self, _linger_ms: u64) {}

    fn beep(&mut self) {}
}

/// プロセスをまたいだ入力の排他 (D-113)。実行器が、画面を変える操作 (入力のバッチ、`focus`、`close`) を含む実行の
/// 前に取り、終わり (押したままのキーとボタンを離し、枠を解いた後) に解く。取れなければ実行器が停止要因を見ながら
/// 試し直す (待つのは実行器で、この trait は待たない)。
/// 持ったスレッドで解く (実装置は名前付きミューテックスで、持ったスレッドでしか解けない)。
/// 実行器は 1 本のスレッドで `try_acquire` から `release` まで走る
pub trait InputLock {
    /// 待たずに取る試み。取れた (前の持ち主が解かずに終わった場合を含む) なら真、別の持ち主がいれば偽
    fn try_acquire(&mut self) -> Result<bool, BackendError>;
    /// 持っているときだけ解く (持っていなければ何もしない)
    fn release(&mut self);
}

/// 工場が持ち続ける錠を借りて渡すための実装 (`DeviceFactory::indicator_and_lock` が `&mut` を箱に入れて返す。D-113)
impl<T: InputLock + ?Sized> InputLock for &mut T {
    fn try_acquire(&mut self) -> Result<bool, BackendError> {
        (**self).try_acquire()
    }

    fn release(&mut self) {
        (**self).release();
    }
}

/// 何もしない錠 (D-113)。`try_acquire` は常に真を返し、`release` は何もしない。
/// 錠の装置を持たない経路 (フィクスチャ限定の実機テスト) と、錠を見ないテストに使う
pub struct NoInputLock;

impl InputLock for NoInputLock {
    fn try_acquire(&mut self) -> Result<bool, BackendError> {
        Ok(true)
    }

    fn release(&mut self) {}
}

/// 時計。テストでは進み方を制御する。
pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

pub struct SystemClock {
    start: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock {
            start: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(hwnd: usize, process: &str, title: &str) -> TopWindow {
        TopWindow {
            info: WindowInfo {
                hwnd,
                pid: 1,
                process: process.into(),
                class: "C".into(),
                title: title.into(),
                elevated: Some(false),
            },
            rect: Some(Rect::new(0, 0, 10, 10)),
            minimized: false,
        }
    }

    /// focus の対象の探し方 (D-050): 実行ファイル名は正規化、題名は大文字小文字を無視した部分一致、
    /// 複数あれば前面に近いもの (配列の先頭側)
    #[test]
    fn match_window_normalizes_process_and_filters_title_in_z_order() {
        let list = vec![
            top(0x1, "chrome.exe", "Docs - Google Chrome"),
            top(0x2, "Notepad.exe", "a.txt - メモ帳"),
            top(0x3, "notepad.exe", "B.TXT - メモ帳"),
        ];
        let hit = |p: &str, t: Option<&str>| {
            match_window(&list, p, t, &|_| true, &|_| false).map(|w| w.info.hwnd)
        };
        assert_eq!(hit("notepad", None), Some(0x2), "前面に近いもの");
        assert_eq!(hit("NOTEPAD.EXE", None), Some(0x2));
        assert_eq!(hit("C:\\Windows\\notepad.exe", None), Some(0x2));
        assert_eq!(
            hit("notepad.exe", Some("b.txt")),
            Some(0x3),
            "題名は大文字小文字を無視"
        );
        assert_eq!(hit("notepad.exe", Some("c.txt")), None);
        assert_eq!(hit("chrome", Some("google")), Some(0x1));
        assert_eq!(hit("explorer.exe", None), None);
    }

    /// 名指しの範囲 (D-139): 述語を通らない窓は無いものとして扱い、通る窓の中で前面から最初の一致を返す。
    /// 前面に近い通らない窓が、後ろの通る窓を隠さない
    #[test]
    fn match_window_skips_windows_not_admitted() {
        let list = vec![
            top(0xA, "notepad.exe", "secret"),
            top(0xB, "notepad.exe", "secret 2"),
        ];
        let hit = |admit: &dyn Fn(&WindowInfo) -> bool| {
            match_window(&list, "notepad.exe", Some("secret"), admit, &|_| false)
                .map(|w| w.info.hwnd)
        };
        assert_eq!(hit(&|w| w.hwnd != 0xA), Some(0xB), "A を通さなければ B");
        assert_eq!(hit(&|_| false), None, "どれも通さなければ無い");
        assert_eq!(hit(&|_| true), Some(0xA), "どれも通せば前面に近い A");
    }

    /// 名指しの 2 段の規則 (D-143): ポップアップらしい窓 (ツールチップなど) は、同じく一致する他の窓より後に回す。
    /// 外しはしない (ポップアップしか一致しなければそれに解ける)。`popup_like` は一致した窓にだけ、Z 順で最初の
    /// ポップアップらしくない一致までを各 1 回呼ぶ
    #[test]
    fn match_window_puts_popup_like_after_other_matches() {
        let list = vec![
            top(0x10, "notepad.exe", "PopupHost"),
            top(0x11, "notepad.exe", "PopupHost"),
            top(0x20, "chrome.exe", "Docs"),
            top(0x30, "notepad.exe", "hidden"),
            top(0x40, "notepad.exe", "a.txt - メモ帳"),
            top(0x50, "notepad.exe", "b.txt - メモ帳"),
        ];
        let popups = [0x10, 0x11];
        let popup_like = |w: &TopWindow| popups.contains(&w.info.hwnd);
        let all = |_: &WindowInfo| true;
        let hit = |t: Option<&str>, admit: &dyn Fn(&WindowInfo) -> bool| {
            match_window(&list, "notepad.exe", t, admit, &popup_like).map(|w| w.info.hwnd)
        };
        assert_eq!(hit(None, &all), Some(0x30), "process だけなら本体");
        assert_eq!(
            hit(Some("popuphost"), &all),
            Some(0x10),
            "ポップアップだけが題名に当たればポップアップ (Z 順で前のもの)"
        );
        assert_eq!(
            hit(Some("popuphost"), &|w| w.hwnd != 0x10),
            Some(0x11),
            "前のポップアップが admit を通らなければ次のポップアップ"
        );
        assert_eq!(
            hit(Some("メモ帳"), &|w| w.hwnd != 0x40),
            Some(0x50),
            "ポップアップらしくない一致の中で Z 順"
        );
        let no_body = |w: &WindowInfo| popups.contains(&w.hwnd);
        assert_eq!(
            hit(None, &no_body),
            Some(0x10),
            "本体が admit を通らなければポップアップ"
        );

        // popup_like の呼ばれ方: 一致した窓にだけ、最初のポップアップらしくない一致まで、各 1 回
        let calls = std::cell::RefCell::new(Vec::new());
        let recording = |w: &TopWindow| {
            calls.borrow_mut().push(w.info.hwnd);
            popups.contains(&w.info.hwnd)
        };
        let not_hidden = |w: &WindowInfo| w.hwnd != 0x30;
        let got = match_window(&list, "notepad.exe", None, &not_hidden, &recording);
        assert_eq!(got.map(|w| w.info.hwnd), Some(0x40));
        assert_eq!(
            *calls.borrow(),
            vec![0x10, 0x11, 0x40],
            "別の process (0x20) と admit を通らない窓 (0x30) と、それより後ろの一致 (0x50) には呼ばない"
        );
    }

    /// 振り分けの材料を組む (テスト用)
    fn classify(
        saver_running: bool,
        session_locked: Option<bool>,
        open: OpenResult,
    ) -> Result<InputDesktop, String> {
        classify_input_desktop(InputDesktopRaw {
            saver_running,
            session_locked,
            open,
        })
    }

    /// 入力デスクトップの振り分けの各通り (D-066)。セーバーは開けた結果を問わず、
    /// 判定が読めない (5 以外の失敗、名前が読めない) のは「ロック」と決めつけず Err。
    /// ロックが非ロック (`Some(false)`) か読めない (None) なら、今までどおり名前で決める (D-067)
    #[test]
    fn classify_input_desktop_covers_each_case() {
        use InputDesktop::*;
        use OpenResult::*;
        let named = |s: &str| Opened(Some(s.to_string()));
        for locked in [None, Some(false)] {
            assert_eq!(
                classify(true, locked, named("Default")),
                Ok(ScreenSaver {
                    name: Some("Default".into())
                }),
                "保護なしのセーバーは Default の上で動く"
            );
            assert_eq!(
                classify(true, locked, named("Screen-saver")),
                Ok(ScreenSaver {
                    name: Some("Screen-saver".into())
                })
            );
            assert_eq!(
                classify(true, locked, Failed("x".into())),
                Ok(ScreenSaver { name: None })
            );
            assert_eq!(
                classify(true, locked, AccessDenied),
                Ok(ScreenSaver { name: None })
            );
            assert_eq!(
                classify(true, locked, Opened(None)),
                Ok(ScreenSaver { name: None })
            );
            assert_eq!(classify(false, locked, named("Default")), Ok(Available));
            assert_eq!(
                classify(false, locked, named("Winlogon")),
                Ok(Other {
                    name: Some("Winlogon".into())
                })
            );
            assert_eq!(
                classify(false, locked, AccessDenied),
                Ok(Other { name: None })
            );
            assert_eq!(
                classify(false, locked, Failed("OpenInputDesktop が失敗".into())),
                Err("OpenInputDesktop が失敗".to_string())
            );
            assert!(classify(false, locked, Opened(None)).is_err());
        }
    }

    /// WTS でロックと読めたら、開けた結果を問わず `Locked` (読めた名前は入れ、開けなくても Err にしない)。
    /// セーバーと同時 (ロック付きのセーバー) なら先に分かるセーバー (D-067)
    #[test]
    fn classify_input_desktop_maps_session_lock() {
        use InputDesktop::*;
        use OpenResult::*;
        let named = |s: &str| Opened(Some(s.to_string()));
        assert_eq!(
            classify(false, Some(true), named("Default")),
            Ok(Locked {
                name: Some("Default".into())
            }),
            "ロックの幕の間は名前が Default のまま"
        );
        assert_eq!(
            classify(false, Some(true), named("Winlogon")),
            Ok(Locked {
                name: Some("Winlogon".into())
            })
        );
        assert_eq!(
            classify(false, Some(true), Opened(None)),
            Ok(Locked { name: None })
        );
        assert_eq!(
            classify(false, Some(true), AccessDenied),
            Ok(Locked { name: None })
        );
        assert_eq!(
            classify(false, Some(true), Failed("x".into())),
            Ok(Locked { name: None })
        );
        assert_eq!(
            classify(true, Some(true), named("Default")),
            Ok(ScreenSaver {
                name: Some("Default".into())
            }),
            "ロック付きのセーバーはセーバー"
        );
        assert_eq!(
            classify(true, Some(true), AccessDenied),
            Ok(ScreenSaver { name: None })
        );
    }

    /// `Default` の名前は大文字小文字を無視して比べる
    #[test]
    fn classify_input_desktop_ignores_case_of_default() {
        for n in ["default", "DEFAULT", "Default"] {
            assert_eq!(
                classify(false, None, OpenResult::Opened(Some(n.into()))),
                Ok(InputDesktop::Available),
                "{n}"
            );
        }
    }

    /// 枠の状態の既定は None (枠の装置を持たない) で、`&mut T` の転送は中身の値を返す (D-103)
    #[test]
    fn indicator_frame_state_defaults_to_none_and_forwards_through_mut_ref() {
        assert_eq!(NoIndicator.frame_state(), None);

        struct Running;
        impl Indicator for Running {
            fn shown_for_ms(&self) -> Option<u64> {
                None
            }
            fn show(&mut self, _deadline_ms: u64) {}
            fn release(&mut self, _linger_ms: u64) {}
            fn extend(&mut self, _linger_ms: u64) {}
            fn beep(&mut self) {}
            fn frame_state(&self) -> Option<FrameState> {
                Some(FrameState::Running)
            }
        }
        let mut inner = Running;
        let borrowed: Box<dyn Indicator + '_> = Box::new(&mut inner);
        assert_eq!(borrowed.frame_state(), Some(FrameState::Running));
    }

    #[test]
    fn held_inputs_is_empty_only_without_keys_and_buttons() {
        assert!(HeldInputs::default().is_empty());
        let k = HeldInputs {
            keys: vec!["shift"],
            buttons: vec![],
        };
        assert!(!k.is_empty());
        let b = HeldInputs {
            keys: vec![],
            buttons: vec!["left"],
        };
        assert!(!b.is_empty());
    }
}
