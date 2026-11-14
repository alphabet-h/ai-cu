//! 実際の装置 (Win32)。`unsafe` は `win32` の下にあり、ここは安全な層。
//!
//! 実入力の最下層の拒否点 (D-022): `RealSink` は、開発シェル (`AI_CU_DEV_FORBID_REAL_INPUT`) では
//! 作れず、送るたびにも確かめ直す。フィクスチャ専用のコンストラクタだけがこの変数の影響を受けず、
//! 代わりにすべてのイベントがフィクスチャの矩形と前面の条件を満たすことを、送る前に確かめる (D-020)。

use std::time::{Duration, Instant};

use crate::config::{EnvLookup, ProcessEnv, dev_forbid_active};
use crate::device::{
    BackendError, Desktop, FrameState, HeldInputs, HelperOutcome, HiddenWindow, Image, Indicator,
    InputDesktop, InputEvent, InputLock, InputSink, StackWindow, TopWindow, WindowInfo,
    WindowOwner, WindowPresence,
};
use crate::geometry::{Point, Rect};
use crate::win32;

/// フィクスチャのウィンドウと、入力を許す矩形 (クライアント領域。帯つきの drag の実機テストとその帯なしの対照だけ上へ広げる。D-036 の追記、D-132)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureScope {
    pub hwnd: usize,
    pub rect: Rect,
}

/// 範囲確認のための観測 (テストでは差し替える)。
pub trait ScopeProbe {
    fn cursor(&self) -> Result<Point, BackendError>;
    fn root_at(&self, p: Point) -> usize;
    fn foreground(&self) -> usize;
}

/// `GetCursorPos` の失敗の写し (1 か所だけに置く。D-066): 5 は入力デスクトップが使えない (14)、ほかは OS の失敗 (8)
impl From<win32::input::CursorError> for BackendError {
    fn from(e: win32::input::CursorError) -> Self {
        match e {
            win32::input::CursorError::AccessDenied(m) => BackendError::DesktopUnavailable(m),
            win32::input::CursorError::Other(m) => BackendError::Os(m),
        }
    }
}

struct Win32Probe;

impl ScopeProbe for Win32Probe {
    fn cursor(&self) -> Result<Point, BackendError> {
        win32::input::cursor_pos().map_err(BackendError::from)
    }

    fn root_at(&self, p: Point) -> usize {
        win32::window::root_at(p)
    }

    fn foreground(&self) -> usize {
        win32::window::foreground_hwnd()
    }
}

/// フィクスチャの範囲の確認。バッチ全体を送る前に確かめ、1 つでも外れたら何も送らない。
/// 押下 (ボタン、ホイール) はその時点のカーソルの位置で、キーの押下は前面のウィンドウで確かめる。
/// 離す方向のイベントは、押したままを解くために常に通す。
pub fn check_fixture_scope(
    scope: &FixtureScope,
    events: &[InputEvent],
    probe: &dyn ScopeProbe,
) -> Result<(), BackendError> {
    // 離す方向だけのバッチは、カーソルが読めなくても必ず通す (押したままを解けなくなるのを避ける)
    if events.iter().all(InputEvent::is_release) {
        return Ok(());
    }
    let mut pos = probe.cursor()?;
    let in_scope = |p: Point, what: &str| -> Result<(), BackendError> {
        if !scope.rect.contains(p) {
            return Err(BackendError::OutOfScope(format!(
                "{what}: ({}, {}) はフィクスチャの矩形 {} の外",
                p.x, p.y, scope.rect
            )));
        }
        let root = probe.root_at(p);
        if root != scope.hwnd {
            return Err(BackendError::OutOfScope(format!(
                "{what}: ({}, {}) の下はフィクスチャではない (hwnd {root:#x})",
                p.x, p.y
            )));
        }
        Ok(())
    };
    for e in events {
        if e.is_release() {
            continue;
        }
        match *e {
            InputEvent::MoveTo { x, y } => {
                pos = Point::new(x, y);
                in_scope(pos, "移動")?;
            }
            InputEvent::Button { .. } | InputEvent::Wheel { .. } => in_scope(pos, "押下")?,
            InputEvent::Key { .. } | InputEvent::Unicode { .. } => {
                let fg = probe.foreground();
                if fg != scope.hwnd {
                    return Err(BackendError::OutOfScope(format!(
                        "前面がフィクスチャではない (前面 hwnd {fg:#x})。キーは送らない"
                    )));
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    General,
    Fixture(FixtureScope),
}

/// `SendInput` を呼ぶ唯一の装置。
pub struct RealSink {
    mode: Mode,
}

impl RealSink {
    /// 通常の実入力の装置。開発シェルでは作れない (D-022)。
    pub fn new(env: &dyn EnvLookup) -> Result<RealSink, BackendError> {
        general_allowed(env)?;
        Ok(RealSink {
            mode: Mode::General,
        })
    }

    /// フィクスチャ専用の装置。開発シェルの拒否の影響を受けない代わりに、送る前に範囲を確かめる。
    pub fn for_fixture(scope: FixtureScope) -> RealSink {
        RealSink {
            mode: Mode::Fixture(scope),
        }
    }
}

/// 通常の実入力を許すか (開発シェルの変数が無いか)。純粋な判定なので単体でテストする。
pub fn general_allowed(env: &dyn EnvLookup) -> Result<(), BackendError> {
    if dev_forbid_active(env) {
        return Err(BackendError::Forbidden(format!(
            "開発シェル ({}) では実入力を送らない。--dry-run、validate、またはフィクスチャ限定のテストを使う",
            crate::config::ENV_DEV_FORBID
        )));
    }
    Ok(())
}

impl InputSink for RealSink {
    fn send(&mut self, events: &[InputEvent]) -> Result<(), BackendError> {
        match &self.mode {
            // 作った後に環境が変わっても止まるよう、送るたびに確かめ直す
            Mode::General => general_allowed(&ProcessEnv)?,
            Mode::Fixture(scope) => check_fixture_scope(scope, events, &Win32Probe)?,
        }
        let screen = win32::screen::virtual_screen().map_err(BackendError::Os)?;
        win32::input::send(events, screen).map_err(BackendError::Os)
    }
}

/// 撮影を許す範囲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// 通常の撮影。開発シェルでは拒否する (D-022 を撮影にも広げる。D-038)
    General,
    /// フィクスチャ専用。開発シェルでも、この矩形に丸ごと収まる撮影だけを許す
    Fixture(Rect),
}

/// 撮影してよいか。純粋な判定なので単体でテストする。
pub fn capture_allowed(
    mode: &CaptureMode,
    rect: Rect,
    env: &dyn EnvLookup,
) -> Result<(), BackendError> {
    match mode {
        CaptureMode::General => {
            if dev_forbid_active(env) {
                return Err(BackendError::Forbidden(format!(
                    "開発シェル ({}) では実画面を撮影しない。フィクスチャ限定のテストを使う",
                    crate::config::ENV_DEV_FORBID
                )));
            }
            Ok(())
        }
        CaptureMode::Fixture(scope) => {
            if scope.contains_rect(&rect) {
                Ok(())
            } else {
                Err(BackendError::OutOfScope(format!(
                    "撮影の矩形 {rect} がフィクスチャの矩形 {scope} からはみ出す"
                )))
            }
        }
    }
}

/// 実際の画面とウィンドウの観測。
pub struct Win32Desktop {
    capture: CaptureMode,
    /// フィクスチャ専用のとき、前面にしてよい唯一のウィンドウ (D-050)
    fixture_hwnd: Option<usize>,
}

impl Win32Desktop {
    /// 通常の観測。撮影は開発シェルでは拒否する。
    pub fn new() -> Self {
        Win32Desktop {
            capture: CaptureMode::General,
            fixture_hwnd: None,
        }
    }

    /// フィクスチャ専用の観測。撮影はフィクスチャの矩形の中だけ、前面にする対象もフィクスチャだけ。
    pub fn for_fixture(scope: FixtureScope) -> Self {
        Win32Desktop {
            capture: CaptureMode::Fixture(scope.rect),
            fixture_hwnd: Some(scope.hwnd),
        }
    }

    /// 窓の一覧と持ち主 (`top_windows`、`window_owners`、`hidden_windows` (D-099)) の共通の門。題名やプロセス名は画面上の情報に近いので、
    /// 撮影と同じく開発シェルでは出さず、フィクスチャ専用の装置では範囲を絞って出さない (D-074)。
    /// `what` は断るときの文に入れる対象の名前 (例 `ウィンドウの一覧`)。
    fn listing_allowed(&self, what: &str) -> Result<(), BackendError> {
        match self.capture {
            CaptureMode::General if dev_forbid_active(&ProcessEnv) => {
                Err(BackendError::Forbidden(format!(
                    "開発シェル ({}) では{what}を出さない",
                    crate::config::ENV_DEV_FORBID
                )))
            }
            CaptureMode::General => Ok(()),
            CaptureMode::Fixture(_) => Err(BackendError::OutOfScope(format!(
                "フィクスチャ専用の装置では{what}を出さない"
            ))),
        }
    }
}

impl Default for Win32Desktop {
    fn default() -> Self {
        Self::new()
    }
}

impl Desktop for Win32Desktop {
    fn virtual_screen(&self) -> Result<Rect, BackendError> {
        win32::screen::virtual_screen().map_err(BackendError::Os)
    }

    fn cursor_pos(&self) -> Result<Point, BackendError> {
        win32::input::cursor_pos().map_err(BackendError::from)
    }

    fn foreground_window(&self) -> Result<Option<WindowInfo>, BackendError> {
        Ok(win32::window::foreground())
    }

    fn window_at(&self, p: Point) -> Result<Option<WindowInfo>, BackendError> {
        Ok(win32::window::at_point(p))
    }

    fn capture(&self, rect: Rect) -> Result<Image, BackendError> {
        capture_allowed(&self.capture, rect, &ProcessEnv)?;
        win32::capture::capture(rect).map_err(BackendError::Os)
    }

    fn top_windows(&self) -> Result<Vec<TopWindow>, BackendError> {
        // 題名には画面上の情報 (開いている文書の名前など) が含まれうるので、撮影と同じく開発シェルでは出さない。
        // フィクスチャ専用の装置では、撮影と同じく範囲を絞る。一覧は使わないので出さない
        self.listing_allowed("ウィンドウの一覧")?;
        Ok(win32::window::top_level_windows_detailed())
    }

    fn window_owners(&self) -> Result<Vec<WindowOwner>, BackendError> {
        // プロセス名は画面上の情報に近いので、一覧 (top_windows) と同じ門を通す (D-074)
        self.listing_allowed("ウィンドウの持ち主")?;
        Ok(win32::window::window_owners())
    }

    fn hidden_windows(&self) -> Result<Vec<HiddenWindow>, BackendError> {
        // 題名とプロセス名は画面上の情報に近いので、一覧 (top_windows) と同じ門を通す (D-099)
        self.listing_allowed("隠れたウィンドウの一覧")?;
        Ok(win32::window::hidden_top_windows())
    }

    fn stack_windows(&self) -> Result<Vec<StackWindow>, BackendError> {
        match self.capture {
            // フィクスチャ専用の装置では、一覧の門 (OutOfScope) で断ると deny を書いた設定でフィクスチャの撮影まで
            // 止まるので、フィクスチャ自身だけを題名なしで返す (`find_window` と同じ形)。フィクスチャの上に重なる
            // 他の窓は見ない。フィクスチャが見えない (最小化、cloaked など) ときは空 (テストだけで使う装置の限界。D-077)
            CaptureMode::Fixture(_) => Ok(self
                .fixture_hwnd
                .and_then(win32::window::stack_window)
                .into_iter()
                .collect()),
            // プロセス名は画面上の情報に近いので、一覧 (top_windows) と同じ門を通す (開発シェルでは Forbidden)
            CaptureMode::General => {
                self.listing_allowed("重なりの一覧")?;
                Ok(win32::window::stack_windows())
            }
        }
    }

    fn is_caption(&self, hwnd: usize, p: Point) -> bool {
        win32::window::is_caption(hwnd, p)
    }

    fn self_elevated(&self) -> bool {
        win32::window::self_elevated()
    }

    fn idle_ms(&self) -> Result<u64, BackendError> {
        win32::input::idle_ms().map_err(BackendError::Os)
    }

    fn input_desktop(&self) -> Result<InputDesktop, BackendError> {
        // 読むだけで、デスクトップの名前は画面の中身を含まない (窓の題名を返す top_windows とは
        // 違う) ので、開発シェルでもフィクスチャ専用でも同じに読む。keys_held も同じ。
        // 生の値は構造体ごと渡す (D-067。値の渡し忘れや取り違えをしない)
        crate::device::classify_input_desktop(win32::desktop::input_desktop())
            .map_err(BackendError::Os)
    }

    fn keys_held(&self) -> Result<HeldInputs, BackendError> {
        Ok(win32::input::keys_held())
    }

    fn find_window(
        &self,
        process: &str,
        title: Option<&str>,
        admit: &dyn Fn(&WindowInfo) -> bool,
    ) -> Result<Option<TopWindow>, BackendError> {
        focus_allowed(self.fixture_hwnd, &ProcessEnv)?;
        // フィクスチャ専用では候補をフィクスチャだけにする (ツールウィンドウなので一覧には出ない)
        let candidates = match self.fixture_hwnd {
            Some(h) => win32::window::describe(h).into_iter().collect(),
            None => win32::window::top_level_windows_detailed(),
        };
        Ok(crate::device::match_window(
            &candidates,
            process,
            title,
            admit,
        ))
    }

    fn set_foreground(&self, hwnd: usize) -> Result<bool, BackendError> {
        focus_allowed(self.fixture_hwnd, &ProcessEnv)?;
        if let Some(h) = self.fixture_hwnd
            && h != hwnd
        {
            return Err(BackendError::OutOfScope(format!(
                "前面にする対象 (hwnd {hwnd:#x}) がフィクスチャ (hwnd {h:#x}) ではない"
            )));
        }
        Ok(win32::window::set_foreground(hwnd))
    }

    fn set_foreground_by_helper(&self, hwnd: usize) -> Result<HelperOutcome, BackendError> {
        focus_allowed(self.fixture_hwnd, &ProcessEnv)?;
        // フィクスチャ専用の装置は使わない (子は範囲を確かめないので、対象の限定を子に持ち込めない)
        if self.fixture_hwnd.is_some() {
            return Err(BackendError::OutOfScope(
                "フィクスチャ専用の装置では前面化の補助プロセスを起動しない".into(),
            ));
        }
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                return Ok(HelperOutcome::Failed(format!(
                    "自分の実行ファイルの場所が分からない: {e}"
                )));
            }
        };
        Ok(crate::focus_helper::spawn_and_wait(
            &exe,
            hwnd,
            crate::focus_helper::TIMEOUT_MS,
        ))
    }

    fn post_close(&self, hwnd: usize) -> Result<bool, BackendError> {
        close_allowed(self.fixture_hwnd, hwnd, &ProcessEnv)?;
        Ok(win32::window::post_close(hwnd))
    }

    fn window_presence(&self, hwnd: usize, pid: u32) -> Result<WindowPresence, BackendError> {
        focus_allowed(self.fixture_hwnd, &ProcessEnv)?;
        Ok(win32::window::presence_of(hwnd, pid))
    }

    fn window_enabled(&self, hwnd: usize) -> Result<bool, BackendError> {
        focus_allowed(self.fixture_hwnd, &ProcessEnv)?;
        Ok(win32::window::is_enabled(hwnd))
    }

    fn owner_root(&self, hwnd: usize) -> usize {
        win32::window::owner_root(hwnd)
    }

    fn ui_elements(
        &self,
        hwnd: usize,
        query: &crate::elements::ElementQuery,
    ) -> Result<crate::elements::Elements, BackendError> {
        uia_allowed(self.fixture_hwnd, hwnd, &ProcessEnv)?;
        win32::uia::elements(hwnd, query).map_err(uia_error)
    }

    fn ui_tree(
        &self,
        hwnd: usize,
        max_depth: usize,
        max_nodes: usize,
    ) -> Result<crate::elements::UiTree, BackendError> {
        uia_allowed(self.fixture_hwnd, hwnd, &ProcessEnv)?;
        win32::uia::tree(hwnd, max_depth, max_nodes).map_err(uia_error)
    }

    fn focused_is_password(&self) -> Result<Option<bool>, BackendError> {
        // uia_allowed (hwnd の門) と capture_enabled は通さない (D-090。focused_password_allowed の doc)
        focused_password_allowed(self.fixture_hwnd, &ProcessEnv)?;
        win32::uia::focused_is_password().map_err(uia_error)
    }
}

/// UIA の層の誤りを装置の層の誤りに写す (D-104)。時間切れは `TimedOut`、他は今までどおり `Os`
fn uia_error(e: win32::uia::UiaError) -> BackendError {
    match e {
        win32::uia::UiaError::Failed(m) => BackendError::Os(m),
        win32::uia::UiaError::TimedOut(m) => BackendError::TimedOut(m),
    }
}

/// UI Automation の最下層の拒否 (D-054)。要素の名前と文字列は画面上の情報なので、撮影と同じく開発シェルでは
/// 読まない。フィクスチャ専用の装置はフィクスチャの hwnd だけに応じる。純粋な判定なので単体でテストする。
pub fn uia_allowed(
    fixture_hwnd: Option<usize>,
    hwnd: usize,
    env: &dyn EnvLookup,
) -> Result<(), BackendError> {
    match fixture_hwnd {
        Some(f) if f == hwnd => Ok(()),
        Some(f) => Err(BackendError::OutOfScope(format!(
            "要素を読む対象 (hwnd {hwnd:#x}) がフィクスチャ (hwnd {f:#x}) ではない"
        ))),
        None if dev_forbid_active(env) => Err(BackendError::Forbidden(format!(
            "開発シェル ({}) では要素を読まない (名前と文字列は画面上の情報)。フィクスチャ限定のテストを使う",
            crate::config::ENV_DEV_FORBID
        ))),
        None => Ok(()),
    }
}

/// `focus` の最下層の拒否 (D-022 を前面化にも広げる。D-050)。フィクスチャ専用の装置は対象が限られているので通す。
/// 純粋な判定なので単体でテストする。
pub fn focus_allowed(fixture_hwnd: Option<usize>, env: &dyn EnvLookup) -> Result<(), BackendError> {
    if fixture_hwnd.is_none() && dev_forbid_active(env) {
        return Err(BackendError::Forbidden(format!(
            "開発シェル ({}) ではウィンドウを探したり前面にしたりしない。フィクスチャ限定のテストを使う",
            crate::config::ENV_DEV_FORBID
        )));
    }
    Ok(())
}

/// フォーカスの要素が IsPassword かを読む最下層の拒否 (D-090)。開発シェルでは読まない (入力の装置
/// `RealSink::new` の `general_allowed` が先に断るので、開発シェルでキーが送られる経路は無く、断っても守りは外れない)。
/// フィクスチャ専用の装置では hwnd で絞らずに読む: `GetFocusedElement` は hwnd を取らないので `uia_allowed` の
/// hwnd の門を当てられず、読むのは真偽 1 つで窓の中身を返さない (D-054 の例外。D-090)。
/// 純粋な判定なので単体でテストする。
pub fn focused_password_allowed(
    fixture_hwnd: Option<usize>,
    env: &dyn EnvLookup,
) -> Result<(), BackendError> {
    if fixture_hwnd.is_none() && dev_forbid_active(env) {
        return Err(BackendError::Forbidden(format!(
            "開発シェル ({}) ではフォーカスの要素を読まない。フィクスチャ限定のテストを使う",
            crate::config::ENV_DEV_FORBID
        )));
    }
    Ok(())
}

/// `close` の最下層の拒否 (D-062)。開発シェルでは閉じない。フィクスチャ専用の装置はフィクスチャの hwnd だけに送る。
/// 純粋な判定なので単体でテストする。
pub fn close_allowed(
    fixture_hwnd: Option<usize>,
    hwnd: usize,
    env: &dyn EnvLookup,
) -> Result<(), BackendError> {
    match fixture_hwnd {
        Some(f) if f == hwnd => Ok(()),
        Some(f) => Err(BackendError::OutOfScope(format!(
            "閉じる対象 (hwnd {hwnd:#x}) がフィクスチャ (hwnd {f:#x}) ではない"
        ))),
        None if dev_forbid_active(env) => Err(BackendError::Forbidden(format!(
            "開発シェル ({}) ではウィンドウを閉じない。フィクスチャ限定のテストを使う",
            crate::config::ENV_DEV_FORBID
        ))),
        None => Ok(()),
    }
}

/// 実の枠と予告の音 (D-093)。最初の `show` で枠のスレッド (`OverlayThread`) を立て、以後は持ち続ける
/// (MCP の常駐で、実行の後に枠を残す時間を効かせるため)。枠は知らせる手段で守りの層ではないので、
/// 立てられない (起動時の WDA の失敗を含む) ときや作り直しに失敗したときは、標準エラーに警告を 1 行書いて続ける
/// (同じ文の警告は 1 回だけ)。立てられなかったら、最後の失敗から 60 秒たつまで立て直さない (起動の時間切れは
/// 最大 5 秒待つので、毎回試すと呼び出しのたびに待ちが乗る)。時間切れの失敗では、切り離したスレッドが終わるまで
/// 立て直さない (試すたびに固まったスレッドを増やさない。D-103)。警告の書き先は既定で標準エラーで、
/// MCP では `set_warn_sink` で `--log` のファイルにも載る書き先 (`mcp::log`) に替える (D-061)。
/// 開発シェルの判定は持たない (実入力は `RealSink::new` が先に断る。判定を 2 か所にしない)
pub struct RealIndicator {
    /// 枠のスレッドと、起動の失敗の記録 (D-103)
    keeper: FrameKeeper<win32::overlay::OverlayThread>,
    /// 書いた警告の文 (同じ失敗を繰り返し書かない)
    warned: Vec<String>,
    /// 警告の 1 行 (「ai-cu: 」の後ろ) を書く先
    warn_sink: fn(&str),
}

impl Default for RealIndicator {
    fn default() -> Self {
        Self {
            keeper: FrameKeeper::new(),
            warned: Vec::new(),
            warn_sink: warn_to_stderr,
        }
    }
}

/// 既定の警告の書き先: 標準エラーに「ai-cu: 」を付けて 1 行
fn warn_to_stderr(body: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "ai-cu: {body}");
}

impl RealIndicator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 警告の書き先を替える (MCP では `mcp::log`。標準エラーと `--log` のファイルの両方に載る)
    pub fn set_warn_sink(&mut self, sink: fn(&str)) {
        self.warn_sink = sink;
    }

    fn warn(&mut self, msg: String) {
        if let Some(body) = warn_once(&mut self.warned, msg) {
            (self.warn_sink)(&body);
        }
    }
}

/// 初めての文なら覚えて、書く 1 行 (「ai-cu: 」の後ろ) を返す。書いたことのある文なら None
fn warn_once(warned: &mut Vec<String>, msg: String) -> Option<String> {
    if warned.contains(&msg) {
        return None;
    }
    let body = format!("警告: 画面の縁の枠を出せない ({msg})。枠なしで続ける");
    warned.push(msg);
    Some(body)
}

/// 枠のスレッドの起動に失敗した後、立て直すまでの間隔 (D-103)。最後の失敗からこれだけたてば、次の `show` で試す
pub const INDICATOR_START_RETRY: Duration = Duration::from_secs(60);

/// 枠のスレッドがひとりでに終わったときの失敗の文 (D-103)
const FRAME_THREAD_ENDED: &str = "枠のスレッドが終わった";

/// 時間切れで切り離したスレッドがまだ動いているかを読む閉包
type StillRunning = Box<dyn Fn() -> bool>;

/// 起動の失敗を `FrameKeeper` が受け取る形 (`StartError` に依らず、テストで閉包を差し込めるように)
struct StartFailed {
    message: String,
    /// 時間切れで切り離したスレッドがまだ動いているか。他の失敗では None
    still_running: Option<StillRunning>,
}

/// 最後の起動の失敗の記録
struct StartFailure {
    at: Instant,
    message: String,
    still_running: Option<StillRunning>,
}

/// 枠のスレッドの型に依らない保持役 (D-103)。スレッドを持つか、最後の起動の失敗を覚え、
/// 再試行の判断 (`ensure`) と状態 (`state`) を同じ `retry_in` から決める
struct FrameKeeper<T> {
    thread: Option<T>,
    start_failure: Option<StartFailure>,
}

impl<T> FrameKeeper<T> {
    fn new() -> Self {
        Self {
            thread: None,
            start_failure: None,
        }
    }

    /// 再試行までの残り。記録が無いか、切り離したスレッドがまだ動いていれば None (0 なら次で試す)
    fn retry_in(&self, now: Instant) -> Option<Duration> {
        let failure = self.start_failure.as_ref()?;
        if failure.still_running.as_ref().is_some_and(|alive| alive()) {
            return None;
        }
        Some(INDICATOR_START_RETRY.saturating_sub(now.saturating_duration_since(failure.at)))
    }

    /// スレッドが無ければ (再試行の時刻なら) 立てる。スレッドがひとりでに終わっていれば捨てて失敗として記録する
    /// (その呼び出しでは立て直さない)。失敗の文を Err で返す (呼び手が警告する)
    fn ensure(
        &mut self,
        now: Instant,
        finished: impl Fn(&T) -> bool,
        start: impl FnOnce() -> Result<T, StartFailed>,
    ) -> Result<(), String> {
        if let Some(t) = &self.thread {
            if !finished(t) {
                return Ok(());
            }
            self.thread = None;
            self.start_failure = Some(StartFailure {
                at: now,
                message: FRAME_THREAD_ENDED.to_string(),
                still_running: None,
            });
            return Err(FRAME_THREAD_ENDED.to_string());
        }
        if self.start_failure.is_some() && self.retry_in(now) != Some(Duration::ZERO) {
            return Ok(());
        }
        match start() {
            Ok(t) => {
                self.thread = Some(t);
                self.start_failure = None;
                Ok(())
            }
            Err(e) => {
                self.start_failure = Some(StartFailure {
                    at: now,
                    message: e.message.clone(),
                    still_running: e.still_running,
                });
                Err(e.message)
            }
        }
    }

    /// 今の枠の状態。終わったスレッドは `StartFailed` と読むだけで、捨てて記録するのは次の `ensure`。
    /// その `ensure` が今の時刻で記録するのに合わせ、retry_in_ms は `INDICATOR_START_RETRY` にする
    /// (None は時間切れで切り離したスレッドを待つ間だけ。D-103)
    fn state(
        &self,
        now: Instant,
        finished: impl Fn(&T) -> bool,
        build_error: impl FnOnce(&T) -> Option<String>,
    ) -> FrameState {
        if let Some(t) = &self.thread {
            if finished(t) {
                return FrameState::StartFailed {
                    error: FRAME_THREAD_ENDED.to_string(),
                    retry_in_ms: Some(millis(INDICATOR_START_RETRY)),
                };
            }
            return match build_error(t) {
                Some(error) => FrameState::RebuildFailed { error },
                None => FrameState::Running,
            };
        }
        match &self.start_failure {
            Some(f) => FrameState::StartFailed {
                error: f.message.clone(),
                retry_in_ms: self.retry_in(now).map(millis),
            },
            None => FrameState::NotStarted,
        }
    }
}

/// `FrameState` の retry_in_ms に入れるミリ秒 (収まらなければ u64::MAX)
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// 起動の誤りを `FrameKeeper` の形にする。切り離したスレッドは `is_finished` の否定で「まだ動いている」と読む
fn start_failure_of(e: win32::overlay::StartError) -> StartFailed {
    StartFailed {
        message: e.message,
        still_running: e
            .detached
            .map(|h| Box::new(move || !h.is_finished()) as StillRunning),
    }
}

impl Indicator for RealIndicator {
    fn shown_for_ms(&self) -> Option<u64> {
        win32::overlay::overlay_shown_for_ms()
    }

    fn show(&mut self, deadline_ms: u64) {
        let ensured = self.keeper.ensure(
            Instant::now(),
            win32::overlay::OverlayThread::is_finished,
            || win32::overlay::OverlayThread::start().map_err(start_failure_of),
        );
        if let Err(e) = ensured {
            self.warn(e);
        }
        let Some(t) = &self.keeper.thread else {
            return;
        };
        // モニターの構成の変化で作り直したときの失敗 (枠のスレッドが積んだもの) を読んで警告する
        let mut errors = Vec::new();
        while let Some(e) = t.take_error() {
            errors.push(e);
        }
        t.show(deadline_ms);
        for e in errors {
            self.warn(e);
        }
    }

    fn release(&mut self, linger_ms: u64) {
        if let Some(t) = &self.keeper.thread {
            t.release(linger_ms);
        }
    }

    fn extend(&mut self, linger_ms: u64) {
        // 枠のスレッドが無ければ何もしない (ensure を呼ばない。延長で枠のスレッドを立てない。D-107)
        if let Some(t) = &self.keeper.thread {
            t.extend(linger_ms);
        }
    }

    fn frame_state(&self) -> Option<FrameState> {
        Some(self.keeper.state(
            Instant::now(),
            win32::overlay::OverlayThread::is_finished,
            win32::overlay::OverlayThread::build_error,
        ))
    }

    fn beep(&mut self) {
        win32::overlay::beep();
    }
}

/// 実の入力の錠 (D-113)。名前付きミューテックス (`win32::input_mutex`) を最初の `try_acquire` で作るか開き、
/// 以後は工場が持ち続ける (`Drop` で閉じる)。持っている数を数え、`release` は持っているときだけ 1 回解く
/// (同じスレッドの取り直しは再帰で数が増える)。持ったスレッドで解く (実行器は 1 本のスレッドで取ってから解くまで走る)
pub struct RealInputLock {
    name: String,
    mutex: Option<win32::input_mutex::InputMutex>,
    held: u32,
    /// 解けない警告の 1 行 (「ai-cu: 」の後ろ) を書く先。既定は標準エラーで、MCP では `mcp::log` (D-061)
    warn_sink: fn(&str),
}

impl Default for RealInputLock {
    fn default() -> Self {
        Self::with_name(win32::input_mutex::INPUT_LOCK_NAME)
    }
}

impl RealInputLock {
    /// 錠の名前を差し替える (テスト用。テストは pid を入れた名前を使い、利用者の ai-cu の錠を握らない)
    pub fn with_name(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mutex: None,
            held: 0,
            warn_sink: warn_to_stderr,
        }
    }

    /// 警告の書き先を替える (MCP では `mcp::log`。標準エラーと `--log` のファイルの両方に載る。D-061)
    pub fn set_warn_sink(&mut self, sink: fn(&str)) {
        self.warn_sink = sink;
    }

    /// 錠の名前 (開かずに読む)
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// 錠を作れない・開けないときの `BackendError` (8 に写る)。`ERROR_ACCESS_DENIED` は、権限の違う ai-cu が
/// 先に作った錠であることが分かる文にする (やり直しても直らない。D-113)
fn input_lock_open_error(name: &str, e: &std::io::Error) -> BackendError {
    if win32::input_mutex::is_access_denied(e) {
        BackendError::Os(format!(
            "権限の違うプロセス (管理者権限の ai-cu など) が先に作った入力の錠を開けない (D-113)。やり直しても直らないので人間に伝える: {name}: {e}"
        ))
    } else if win32::input_mutex::is_invalid_handle(e) {
        BackendError::Os(format!(
            "入力の錠と同じ名前の、ミューテックスでない物が先にある (D-113)。やり直しても直らないので人間に伝える: {name}: {e}"
        ))
    } else {
        BackendError::Os(format!("入力の錠 {name} を作れない (D-113): {e}"))
    }
}

impl InputLock for RealInputLock {
    fn try_acquire(&mut self) -> Result<bool, BackendError> {
        let m = match &mut self.mutex {
            Some(m) => &*m,
            slot @ None => &*slot.insert(
                win32::input_mutex::InputMutex::open(&self.name)
                    .map_err(|e| input_lock_open_error(&self.name, &e))?,
            ),
        };
        let got = m.try_lock().map_err(|e| {
            BackendError::Os(format!("入力の錠 {} を試せない (D-113): {e}", self.name))
        })?;
        if got {
            self.held += 1;
        }
        Ok(got)
    }

    fn release(&mut self) {
        if self.held == 0 {
            return;
        }
        if let Some(m) = &self.mutex
            && let Err(e) = m.unlock()
        {
            (self.warn_sink)(&format!(
                "警告: 入力の錠 {} を解けない ({e})。続ける",
                self.name
            ));
        }
        self.held -= 1;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::config::{ENV_DEV_FORBID, MapEnv};
    use crate::device::Desktop;
    use crate::script::Button;

    /// UIA の Failed は Os、TimedOut は TimedOut に写り、文はそのまま (D-104)
    #[test]
    fn uia_error_maps_failed_to_os_and_timed_out_to_timed_out() {
        use crate::win32::uia::UiaError;
        assert_eq!(
            uia_error(UiaError::Failed("FindAll".into())),
            BackendError::Os("FindAll".into())
        );
        assert_eq!(
            uia_error(UiaError::TimedOut("30 秒で時間切れ".into())),
            BackendError::TimedOut("30 秒で時間切れ".into())
        );
    }

    /// 枠を出せない警告は、同じ文なら 1 回だけ書く。違う文はそれぞれ 1 回 (D-093)
    #[test]
    fn real_indicator_warns_each_failure_once() {
        let mut warned = Vec::new();
        let first = warn_once(&mut warned, "WDA を付けられない".into()).unwrap();
        assert!(first.starts_with("警告: "), "{first}");
        assert!(first.contains("WDA を付けられない"), "{first}");
        assert!(first.contains("枠なしで続ける"), "{first}");
        assert_eq!(warn_once(&mut warned, "WDA を付けられない".into()), None);
        assert!(warn_once(&mut warned, "作り直しに失敗".into()).is_some());
        assert_eq!(warn_once(&mut warned, "作り直しに失敗".into()), None);
        assert_eq!(warned.len(), 2);
    }

    thread_local! {
        static SUNK: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_warning(body: &str) {
        SUNK.with(|s| s.borrow_mut().push(body.to_string()));
    }

    /// 警告は差し替えた書き先 (MCP では `mcp::log`) に、同じ文なら 1 回だけ渡る (D-093、D-061)
    #[test]
    fn real_indicator_warns_through_the_injected_sink() {
        SUNK.with(|s| s.borrow_mut().clear());
        let mut ind = RealIndicator::new();
        ind.set_warn_sink(record_warning);
        ind.warn("WDA を付けられない".into());
        ind.warn("WDA を付けられない".into());
        let got = SUNK.with(|s| s.borrow().clone());
        assert_eq!(
            got,
            vec!["警告: 画面の縁の枠を出せない (WDA を付けられない)。枠なしで続ける".to_string()]
        );
    }

    /// 別のスレッドから同じ名前の錠を試す (取れたら、そのスレッドで解いてから真を返す)
    fn other_thread_can_take(name: &str) -> bool {
        let name = name.to_string();
        std::thread::spawn(move || {
            let m = win32::input_mutex::InputMutex::open(&name).expect("開ける");
            let got = m.try_lock().expect("試める");
            if got {
                m.unlock().expect("持ったスレッドで解ける");
            }
            got
        })
        .join()
        .expect("相手のスレッド")
    }

    /// 錠を作れない・開けない誤りの文は、権限の違うプロセス (5)、同じ名前の別の種類の物 (6)、それ以外で分ける。
    /// 前の 2 つはやり直しても直らないと言う (D-113、win32-reviewer の指摘)
    #[test]
    fn input_lock_open_error_names_access_denied_and_wrong_kind() {
        let msg = |code: i32| match input_lock_open_error(
            "n",
            &std::io::Error::from_raw_os_error(code),
        ) {
            BackendError::Os(m) => m,
            other => panic!("{other:?}"),
        };
        let denied = msg(5);
        assert!(denied.contains("権限の違うプロセス"), "{denied}");
        assert!(denied.contains("やり直しても直らない"), "{denied}");
        let wrong = msg(6);
        assert!(wrong.contains("ミューテックスでない物"), "{wrong}");
        assert!(wrong.contains("やり直しても直らない"), "{wrong}");
        let other = msg(8);
        assert!(other.contains("を作れない"), "{other}");
        assert!(!other.contains("やり直しても直らない"), "{other}");
    }

    /// 実の錠は、取ると別のスレッドから取れず、`release` の後は本当に空く (ReleaseMutex を呼ぶ)。
    /// 持っていないときの `release` は何もしない (解きに行かず、警告も書かない。D-113)
    #[test]
    fn real_input_lock_release_frees_the_mutex() {
        SUNK.with(|s| s.borrow_mut().clear());
        let name = format!("Local\\ai-cu-input-test-{}-real", std::process::id());
        let mut lock = RealInputLock::with_name(&name);
        lock.set_warn_sink(record_warning);
        assert_eq!(lock.try_acquire(), Ok(true));
        assert_eq!(lock.held, 1);
        assert!(
            !other_thread_can_take(&name),
            "持っている錠が別のスレッドから取れた"
        );
        lock.release();
        assert_eq!(lock.held, 0);
        assert!(other_thread_can_take(&name), "release の後も錠が空かない");
        lock.release();
        assert_eq!(lock.held, 0);
        assert!(
            SUNK.with(|s| s.borrow().is_empty()),
            "持っていない release が警告した"
        );
        assert_eq!(lock.try_acquire(), Ok(true), "解いた錠を取り直せない");
        lock.release();
        assert!(
            other_thread_can_take(&name),
            "取り直した錠が release で空かない"
        );
    }

    /// 錠を解けないときは、差し替えた書き先 (MCP では `mcp::log`) に警告を 1 行書いて続ける (D-113、D-061)
    #[test]
    fn real_input_lock_unlock_failure_warns_through_the_injected_sink() {
        SUNK.with(|s| s.borrow_mut().clear());
        let name = format!("Local\\ai-cu-input-test-{}-warn", std::process::id());
        let mut lock = RealInputLock::with_name(&name);
        lock.set_warn_sink(record_warning);
        assert_eq!(lock.try_acquire(), Ok(true));
        lock.release();
        // 持っていない錠を持っていると数えさせ、ReleaseMutex を失敗させる
        lock.held = 1;
        lock.release();
        assert_eq!(lock.held, 0);
        let got = SUNK.with(|s| s.borrow().clone());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].starts_with(&format!("警告: 入力の錠 {name} を解けない (")),
            "{got:?}"
        );
        assert!(got[0].ends_with("。続ける"), "{got:?}");
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn never_finished(_: &u32) -> bool {
        false
    }

    fn no_build_error(_: &u32) -> Option<String> {
        None
    }

    fn fail(message: &str) -> Result<u32, StartFailed> {
        Err(StartFailed {
            message: message.to_string(),
            still_running: None,
        })
    }

    fn must_not_start() -> Result<u32, StartFailed> {
        panic!("start を呼んではいけない");
    }

    fn state_of(k: &FrameKeeper<u32>, now: Instant) -> FrameState {
        k.state(now, never_finished, no_build_error)
    }

    /// 起動の失敗の後は 60 秒たつまで立て直さず、ちょうど 60 秒で試す。成功の後は立て直さない (D-103)
    #[test]
    fn frame_keeper_retries_start_only_after_interval() {
        let t0 = Instant::now();
        let mut k = FrameKeeper::<u32>::new();
        assert_eq!(
            k.ensure(t0, never_finished, || fail("WDA")),
            Err("WDA".to_string())
        );
        assert_eq!(
            state_of(&k, t0),
            FrameState::StartFailed {
                error: "WDA".into(),
                retry_in_ms: Some(60_000)
            }
        );
        assert_eq!(
            k.ensure(t0 + secs(59), never_finished, must_not_start),
            Ok(())
        );
        assert_eq!(
            state_of(&k, t0 + secs(59)),
            FrameState::StartFailed {
                error: "WDA".into(),
                retry_in_ms: Some(1000)
            }
        );
        let started = Cell::new(false);
        let start = || {
            started.set(true);
            Ok(7)
        };
        assert_eq!(k.ensure(t0 + secs(60), never_finished, start), Ok(()));
        assert!(started.get(), "ちょうど 60 秒で試す");
        assert_eq!(state_of(&k, t0 + secs(60)), FrameState::Running);
        assert!(k.start_failure.is_none(), "成功で失敗の記録が消える");
        assert_eq!(
            k.ensure(t0 + secs(61), never_finished, must_not_start),
            Ok(())
        );
        assert_eq!(state_of(&k, t0 + secs(61)), FrameState::Running);
        assert_eq!(k.thread, Some(7));
    }

    /// 再試行の失敗は時刻と文を新しくする (D-103)
    #[test]
    fn frame_keeper_records_latest_failure_time() {
        let t0 = Instant::now();
        let mut k = FrameKeeper::<u32>::new();
        let _ = k.ensure(t0, never_finished, || fail("WDA"));
        let t1 = t0 + secs(60);
        assert_eq!(
            k.ensure(t1, never_finished, || fail("WDA")),
            Err("WDA".to_string())
        );
        assert_eq!(
            state_of(&k, t1),
            FrameState::StartFailed {
                error: "WDA".into(),
                retry_in_ms: Some(60_000)
            }
        );
        assert_eq!(
            k.ensure(t1 + secs(59), never_finished, must_not_start),
            Ok(())
        );
        let t2 = t1 + secs(60);
        let _ = k.ensure(t2, never_finished, || fail("モニターが無い"));
        assert_eq!(
            state_of(&k, t2 + secs(1)),
            FrameState::StartFailed {
                error: "モニターが無い".into(),
                retry_in_ms: Some(59_000)
            }
        );
    }

    /// 時間切れで切り離したスレッドが動いている間は何時間たっても試さず、終われば 60 秒の規則に戻る (D-103)
    #[test]
    fn frame_keeper_waits_for_timed_out_thread() {
        let t0 = Instant::now();
        let alive = std::rc::Rc::new(Cell::new(true));
        let probe = std::rc::Rc::clone(&alive);
        let mut k = FrameKeeper::<u32>::new();
        let _ = k.ensure(t0, never_finished, || {
            Err(StartFailed {
                message: "枠の窓が作られない".into(),
                still_running: Some(Box::new(move || probe.get())),
            })
        });
        let later = t0 + secs(3600);
        assert_eq!(
            state_of(&k, later),
            FrameState::StartFailed {
                error: "枠の窓が作られない".into(),
                retry_in_ms: None
            }
        );
        assert_eq!(k.ensure(later, never_finished, must_not_start), Ok(()));
        alive.set(false);
        assert_eq!(
            state_of(&k, later),
            FrameState::StartFailed {
                error: "枠の窓が作られない".into(),
                retry_in_ms: Some(0)
            }
        );
        let started = Cell::new(false);
        let start = || {
            started.set(true);
            Ok(1)
        };
        assert_eq!(k.ensure(later, never_finished, start), Ok(()));
        assert!(started.get(), "切り離したスレッドが終われば試す");
    }

    /// 切り離したスレッドの `JoinHandle` を「まだ動いているか」に変える (D-103)。窓は作らない普通のスレッド
    #[test]
    fn start_failure_of_reads_detached_thread_liveness() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        let failed = start_failure_of(win32::overlay::StartError {
            message: "枠の窓が作られない".into(),
            detached: Some(handle),
        });
        assert_eq!(failed.message, "枠の窓が作られない");
        let still_running = failed.still_running.expect("切り離したスレッドがある");
        assert!(still_running(), "放す前は動いている");
        tx.send(()).unwrap();
        let deadline = Instant::now() + secs(5);
        while still_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!still_running(), "放せば終わる");
        let plain = start_failure_of(win32::overlay::StartError {
            message: "WDA".into(),
            detached: None,
        });
        assert!(plain.still_running.is_none());
    }

    /// 何もしていなければ NotStarted。スレッドがあれば作り直しの失敗で RebuildFailed、無ければ Running (D-103)
    #[test]
    fn frame_keeper_state_reports_rebuild_failure_and_not_started() {
        let t0 = Instant::now();
        let mut k = FrameKeeper::<u32>::new();
        assert_eq!(state_of(&k, t0), FrameState::NotStarted);
        assert_eq!(k.ensure(t0, never_finished, || Ok(3)), Ok(()));
        assert_eq!(
            k.state(t0, never_finished, |_| Some("作り直しに失敗".into())),
            FrameState::RebuildFailed {
                error: "作り直しに失敗".into()
            }
        );
        assert_eq!(state_of(&k, t0), FrameState::Running);
    }

    /// 枠のスレッドがひとりでに終わったら Running と言わず、捨てて失敗として記録し、60 秒後に立て直す (D-103)
    #[test]
    fn frame_keeper_treats_finished_thread_as_start_failure() {
        let t0 = Instant::now();
        let ended = Cell::new(false);
        let finished = |_: &u32| ended.get();
        let mut k = FrameKeeper::<u32>::new();
        assert_eq!(k.ensure(t0, finished, || Ok(5)), Ok(()));
        assert_eq!(k.state(t0, finished, no_build_error), FrameState::Running);
        ended.set(true);
        let t = t0 + secs(10);
        assert_eq!(
            k.state(t, finished, no_build_error),
            FrameState::StartFailed {
                error: FRAME_THREAD_ENDED.into(),
                retry_in_ms: Some(60_000)
            },
            "終わっただけのスレッドを、時間切れの待ち (None) と読ませない"
        );
        assert_eq!(
            k.ensure(t, finished, must_not_start),
            Err(FRAME_THREAD_ENDED.to_string())
        );
        assert_eq!(k.thread, None, "終わったスレッドは捨てる");
        assert_eq!(
            k.state(t, finished, no_build_error),
            FrameState::StartFailed {
                error: FRAME_THREAD_ENDED.into(),
                retry_in_ms: Some(60_000)
            }
        );
        assert_eq!(k.ensure(t + secs(59), finished, must_not_start), Ok(()));
        ended.set(false);
        let started = Cell::new(false);
        let start = || {
            started.set(true);
            Ok(6)
        };
        assert_eq!(k.ensure(t + secs(60), finished, start), Ok(()));
        assert!(started.get(), "60 秒後に立て直す");
        assert_eq!(
            k.state(t + secs(60), finished, no_build_error),
            FrameState::Running
        );
    }

    /// UI Automation の最下層 (D-054): 開発シェルでは拒否、フィクスチャ専用の装置ならフィクスチャの hwnd だけ
    #[test]
    fn uia_allowed_refuses_in_dev_shell_and_limits_fixture_to_its_window() {
        let dev = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        assert!(matches!(
            uia_allowed(None, 0x10, &dev),
            Err(BackendError::Forbidden(_))
        ));
        assert!(uia_allowed(None, 0x10, &MapEnv::default()).is_ok());
        assert!(
            uia_allowed(Some(0x10), 0x10, &dev).is_ok(),
            "フィクスチャ自身"
        );
        assert!(matches!(
            uia_allowed(Some(0x10), 0x11, &dev),
            Err(BackendError::OutOfScope(_))
        ));
    }

    struct Probe {
        cursor: Cell<Point>,
        fixture: usize,
        fixture_rect: Rect,
        foreground: usize,
    }

    impl ScopeProbe for Probe {
        fn cursor(&self) -> Result<Point, BackendError> {
            Ok(self.cursor.get())
        }
        fn root_at(&self, p: Point) -> usize {
            if self.fixture_rect.contains(p) {
                self.fixture
            } else {
                0xDEAD
            }
        }
        fn foreground(&self) -> usize {
            self.foreground
        }
    }

    fn scope() -> FixtureScope {
        FixtureScope {
            hwnd: 0x10,
            rect: Rect::new(100, 100, 200, 100),
        }
    }

    fn probe(foreground: usize) -> Probe {
        Probe {
            cursor: Cell::new(Point::new(5, 5)),
            fixture: 0x10,
            fixture_rect: Rect::new(100, 100, 200, 100),
            foreground,
        }
    }

    #[test]
    fn forbid_real_input_general_sink_refused_in_dev_shell() {
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        assert!(matches!(
            RealSink::new(&env),
            Err(BackendError::Forbidden(_))
        ));
        assert!(matches!(
            general_allowed(&env),
            Err(BackendError::Forbidden(_))
        ));
    }

    #[test]
    fn forbid_real_input_capture_refused_in_dev_shell_except_fixture_rect() {
        let dev = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        let r = Rect::new(10, 10, 50, 50);
        assert!(matches!(
            capture_allowed(&CaptureMode::General, r, &dev),
            Err(BackendError::Forbidden(_))
        ));
        assert!(capture_allowed(&CaptureMode::General, r, &MapEnv::default()).is_ok());
        let fixture = CaptureMode::Fixture(Rect::new(0, 0, 100, 100));
        assert!(capture_allowed(&fixture, r, &dev).is_ok());
        assert!(matches!(
            capture_allowed(&fixture, Rect::new(60, 60, 50, 50), &dev),
            Err(BackendError::OutOfScope(_))
        ));
    }

    /// フォーカスの要素を読む最下層 (D-090): 開発シェルでは読まない。フィクスチャ専用は hwnd で絞らずに通す。
    /// 門だけを撃つ (`Win32Desktop` のメソッドは本物の UIA に届くので呼ばない)
    #[test]
    fn forbid_real_input_focused_password_refused_in_dev_shell_except_fixture() {
        let dev = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        assert!(matches!(
            focused_password_allowed(None, &dev),
            Err(BackendError::Forbidden(m)) if m.contains("フォーカスの要素を読まない")
        ));
        assert!(focused_password_allowed(Some(0x10), &dev).is_ok());
        assert!(focused_password_allowed(None, &MapEnv::default()).is_ok());
    }

    /// focus の最下層 (D-050): 開発シェルでは探すことも前面にすることもしない。フィクスチャ専用は通す
    #[test]
    fn forbid_real_input_focus_refused_in_dev_shell_except_fixture() {
        let dev = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        assert!(matches!(
            focus_allowed(None, &dev),
            Err(BackendError::Forbidden(_))
        ));
        assert!(focus_allowed(None, &MapEnv::default()).is_ok());
        assert!(focus_allowed(Some(0x10), &dev).is_ok());
        // フィクスチャ専用の装置は、フィクスチャ以外を前面にしない (Win32 を呼ぶ前に断る)
        let desktop = Win32Desktop::for_fixture(scope());
        assert!(matches!(
            desktop.set_foreground(0xDEAD),
            Err(BackendError::OutOfScope(_))
        ));
    }

    /// close の最下層 (D-062): 開発シェルでは閉じない。フィクスチャ専用の装置はフィクスチャの hwnd だけ
    #[test]
    fn forbid_real_input_close_refused_in_dev_shell_and_limited_to_fixture() {
        let dev = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        assert!(matches!(
            close_allowed(None, 0x10, &dev),
            Err(BackendError::Forbidden(_))
        ));
        assert!(close_allowed(None, 0x10, &MapEnv::default()).is_ok());
        assert!(close_allowed(Some(0x10), 0x10, &dev).is_ok());
        assert!(matches!(
            close_allowed(Some(0x10), 0x20, &MapEnv::default()),
            Err(BackendError::OutOfScope(_))
        ));
        // フィクスチャ専用の装置は、フィクスチャ以外に WM_CLOSE を置かない (Win32 を呼ぶ前に断る)
        let desktop = Win32Desktop::for_fixture(scope());
        assert!(matches!(
            desktop.post_close(0xDEAD),
            Err(BackendError::OutOfScope(_))
        ));
    }

    /// 前面化の補助プロセス (D-057) は、フィクスチャ専用の装置では起動しない (対象がフィクスチャでも。子は範囲を
    /// 確かめられないので)
    #[test]
    fn forbid_real_input_focus_helper_not_spawned_by_fixture_desktop() {
        let s = scope();
        let desktop = Win32Desktop::for_fixture(s);
        for hwnd in [s.hwnd, 0xDEAD] {
            assert!(matches!(
                desktop.set_foreground_by_helper(hwnd),
                Err(BackendError::OutOfScope(_))
            ));
        }
    }

    /// 窓の持ち主 (D-074) は、一覧 (top_windows) と同じく、フィクスチャ専用の装置では出さない (Win32 を呼ぶ前に断る)
    #[test]
    fn window_owners_refused_by_fixture_desktop_like_top_windows() {
        let desktop = Win32Desktop::for_fixture(scope());
        assert!(matches!(
            desktop.window_owners(),
            Err(BackendError::OutOfScope(_))
        ));
        assert!(matches!(
            desktop.top_windows(),
            Err(BackendError::OutOfScope(_))
        ));
    }

    /// 隠れた窓の一覧 (D-099) も、一覧 (top_windows) と同じく、フィクスチャ専用の装置では出さない (Win32 を呼ぶ前に断る)
    #[test]
    fn hidden_windows_refused_by_fixture_desktop_like_top_windows() {
        let desktop = Win32Desktop::for_fixture(scope());
        assert!(matches!(
            desktop.hidden_windows(),
            Err(BackendError::OutOfScope(_))
        ));
        assert!(matches!(
            desktop.top_windows(),
            Err(BackendError::OutOfScope(_))
        ));
    }

    #[test]
    fn forbid_real_input_allowed_only_without_dev_variable() {
        assert!(general_allowed(&MapEnv::default()).is_ok());
        assert!(general_allowed(&MapEnv::new(&[(ENV_DEV_FORBID, "")])).is_err());
        assert!(general_allowed(&MapEnv::new(&[(ENV_DEV_FORBID, "0")])).is_err());
        assert!(general_allowed(&MapEnv::new(&[(ENV_DEV_FORBID, "yes")])).is_err());
    }

    #[test]
    fn foreground_guard_fixture_scope_rejects_moves_outside_rect() {
        let ev = [
            InputEvent::MoveTo { x: 150, y: 150 },
            InputEvent::MoveTo { x: 301, y: 150 },
        ];
        let r = check_fixture_scope(&scope(), &ev, &probe(0x10));
        assert!(matches!(r, Err(BackendError::OutOfScope(_))));
    }

    #[test]
    fn foreground_guard_fixture_scope_tracks_cursor_within_batch() {
        let ok = [
            InputEvent::MoveTo { x: 150, y: 150 },
            InputEvent::Button {
                button: Button::Left,
                down: true,
            },
            InputEvent::Button {
                button: Button::Left,
                down: false,
            },
        ];
        assert!(check_fixture_scope(&scope(), &ok, &probe(0x10)).is_ok());
        // 移動なしの押下は、現在のカーソル (5, 5) で確かめる → 範囲外
        let press = [InputEvent::Button {
            button: Button::Left,
            down: true,
        }];
        assert!(check_fixture_scope(&scope(), &press, &probe(0x10)).is_err());
        // 離すだけなら常に通す
        let release = [InputEvent::Button {
            button: Button::Left,
            down: false,
        }];
        assert!(check_fixture_scope(&scope(), &release, &probe(0x10)).is_ok());
    }

    /// GetCursorPos の 5 は DesktopUnavailable、ほかは Os。文はそのまま運ぶ (D-066)
    #[test]
    fn cursor_error_maps_access_denied_to_desktop_unavailable() {
        let denied = BackendError::from(win32::input::cursor_error(5));
        match &denied {
            BackendError::DesktopUnavailable(m) => {
                assert!(m.starts_with("GetCursorPos が失敗 (GetLastError=5)"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            BackendError::from(win32::input::cursor_error(87)),
            BackendError::Os("GetCursorPos が失敗 (GetLastError=87)".into())
        );
    }

    struct BrokenCursor;

    impl ScopeProbe for BrokenCursor {
        fn cursor(&self) -> Result<Point, BackendError> {
            Err(BackendError::Os("GetCursorPos".into()))
        }
        fn root_at(&self, _: Point) -> usize {
            0x10
        }
        fn foreground(&self) -> usize {
            0x10
        }
    }

    #[test]
    fn foreground_guard_fixture_scope_passes_release_only_batches_without_cursor() {
        let release = [
            InputEvent::Key {
                vk: 0x11,
                down: false,
                extended: false,
            },
            InputEvent::Button {
                button: Button::Left,
                down: false,
            },
        ];
        assert!(check_fixture_scope(&scope(), &release, &BrokenCursor).is_ok());
        let press = [InputEvent::Button {
            button: Button::Left,
            down: true,
        }];
        assert!(check_fixture_scope(&scope(), &press, &BrokenCursor).is_err());
    }

    #[test]
    fn foreground_guard_fixture_scope_rejects_keys_when_not_foreground() {
        let keys = [
            InputEvent::Key {
                vk: 0x41,
                down: true,
                extended: false,
            },
            InputEvent::Key {
                vk: 0x41,
                down: false,
                extended: false,
            },
        ];
        assert!(check_fixture_scope(&scope(), &keys, &probe(0x99)).is_err());
        assert!(check_fixture_scope(&scope(), &keys, &probe(0x10)).is_ok());
        let up_only = [InputEvent::Key {
            vk: 0x41,
            down: false,
            extended: false,
        }];
        assert!(check_fixture_scope(&scope(), &up_only, &probe(0x99)).is_ok());
    }
}
