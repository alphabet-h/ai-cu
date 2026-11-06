//! テスト用の装置のモックと補助。実デスクトップには一切触れない (D-006)。
//! 結合テストからも使えるように公開している。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::config::EnvLookup;
use crate::device::{
    BackendError, Clock, Desktop, FrameState, HeldInputs, HelperOutcome, HiddenWindow, Image,
    Indicator, InputDesktop, InputEvent, InputLock, InputSink, StackWindow, TopWindow, WindowInfo,
    WindowOwner, WindowPresence,
};
use crate::geometry::{Point, Rect};
use crate::run::ImageWriter;
use crate::runner::DeviceFactory;

/// モックの世界の状態。sink と desktop で共有する。
#[derive(Debug)]
pub struct World {
    pub screen: Rect,
    pub cursor: Point,
    /// 送られたバッチ (1 回の send が 1 要素)
    pub sent: Vec<Vec<InputEvent>>,
    pub foreground: Option<WindowInfo>,
    /// 座標の下のウィンドウ。最初に一致した矩形のものを返す。どれにも一致しなければ `default_window`
    pub windows_at: Vec<(Rect, WindowInfo)>,
    /// window_at が windows_at を見る時刻の範囲 [from, until) (重なりが現れる・消える。D-082)。None は制限なし
    pub windows_at_from_ms: Option<u64>,
    /// `windows_at_from_ms` と組の、範囲の終わり (この時刻から重なりが消える。D-082)。None は制限なし
    pub windows_at_until_ms: Option<u64>,
    pub default_window: Option<WindowInfo>,
    pub self_elevated: bool,
    /// send がこのエラーを返す (送られた記録は残らない)
    pub refuse: Option<BackendError>,
    /// n 回目 (0 始まり) の send の後に、利用者がカーソルをこの位置へ動かしたことにする
    pub user_moves: Vec<(usize, Point)>,
    /// n 回目 (0 始まり) の send の後に、前面をこの窓に差し替える (type の途中で前面が変わるのを模す。D-100)
    pub foreground_after_sends: Vec<(usize, WindowInfo)>,
    /// 時計: 現在のミリ秒と、眠った合計
    pub now_ms: u64,
    /// トップレベルのウィンドウの一覧 (前面から奥へ)
    pub top: Vec<TopWindow>,
    /// 題名の帯 (WM_NCHITTEST で HTCAPTION になる範囲) をウィンドウごとに
    pub captions: Vec<(usize, Rect)>,
    /// 最後の入力からの経過 (ミリ秒)。既定は十分に長い (人は操作していない)
    pub idle_ms: u64,
    /// `set_foreground` を OS が受け付けるか (受け付けると、その `top` のウィンドウが前面になる)。D-050
    pub focus_accepts: bool,
    /// 受け付けたと返すが前面は変わらない (タスクバーの点滅だけの場合)
    pub focus_no_effect: bool,
    /// 受け付けると、対象の代わりにこのウィンドウが前面になる (同じプロセスのモーダルダイアログなど)
    pub focus_redirect: Option<WindowInfo>,
    /// 受け付けると同時に対象が閉じられる (一覧から消え、前面は変わらない)
    pub focus_closes: bool,
    /// `set_foreground` が呼ばれた後、前面のウィンドウはこの回数だけ読め、その次から失敗する (D-056 の `refused_unknown`)
    pub foreground_reads_after_focus: Option<usize>,
    /// `set_foreground` が呼ばれた後に前面のウィンドウを読んだ回数
    pub foreground_reads: usize,
    /// 所有関係 (ウィンドウ, 所有者の根)。無ければ自分自身が根
    pub owners: Vec<(usize, usize)>,
    /// `set_foreground` に渡されたハンドル (呼ばれた順)
    pub focus_calls: Vec<usize>,
    /// 前面化の補助プロセスの結果 (D-057)。`Accepted` なら、その `top` のウィンドウが前面になる。既定は `Refused`
    pub helper: HelperOutcome,
    /// 補助が `Accepted` を返しても前面は変わらない (タスクバーの点滅だけの場合)
    pub helper_no_effect: bool,
    /// 補助の間に対象が閉じられる (一覧から消え、前面は変わらない)
    pub helper_closes: bool,
    /// `set_foreground_by_helper` に渡されたハンドル (呼ばれた順)
    pub helper_calls: Vec<usize>,
    /// UI Automation の要素 (ウィンドウの hwnd, 要素)。木の順 (D-054)
    pub elements: Vec<(usize, crate::elements::UiElement)>,
    /// `post_close` に渡されたハンドル (呼ばれた順。D-062)
    pub close_calls: Vec<usize>,
    /// `WM_CLOSE` を置けたと返すが、ウィンドウは残る (保存の確認を出すアプリ)
    pub close_kept: bool,
    /// `post_close` が置けなかった (false) と返す
    pub close_fails: bool,
    /// `WM_CLOSE` を置くと窓は一覧 (`top`) から消えるが、ハンドルは残って隠れる (トレイに入るアプリ。D-096)
    pub close_hides: bool,
    /// 隠れた窓 (`top` から外した要素をそのまま持つ。また見えるときは `top` に戻す。D-096)
    pub hidden: Vec<TopWindow>,
    /// `close_hides` で最後に窓を隠した時刻 (モックの時計。`post_close` が書く)
    pub hidden_at_ms: Option<u64>,
    /// Some(d) なら、隠してから d ミリ秒を過ぎた後の `window_presence` の読みで `hidden` から外す (隠れた後に壊れる)
    pub hidden_then_destroyed_after_ms: Option<u64>,
    /// Some(d) なら、隠してから d ミリ秒を過ぎた後の `window_presence` の読みで `top` に戻す (隠れた後にまた見える)
    pub hidden_then_visible_after_ms: Option<u64>,
    /// 無効になっている (モーダルのダイアログを開いている) ウィンドウ
    pub disabled: Vec<usize>,
    /// `input_desktop` が返すもの (D-066)。既定は `Ok(Available)`
    pub input_desktop: Result<InputDesktop, BackendError>,
    /// `input_desktop` が呼ばれた回数
    pub input_desktop_reads: usize,
    /// `(n, r)`: 最初の n 回の読みは `input_desktop` を返し、それより後の読みは r を返す
    /// (途中のステップの前でロックやセーバーになったことを模す。D-067)
    pub input_desktop_after_reads: Option<(usize, Result<InputDesktop, BackendError>)>,
    /// `keys_held` が返すもの (D-066)。既定は空
    pub held: HeldInputs,
    /// `keys_held` が呼ばれた回数
    pub keys_held_calls: usize,
    /// `cursor_pos` が呼ばれた回数 (失敗した読みも数える)
    pub cursor_reads: usize,
    /// `(n, e)`: 最初の n 回の読みは成功し、それより後の読みは e を返す (`(0, e)` なら最初の読みから失敗)
    pub cursor_fail_after_reads: Option<(usize, BackendError)>,
    /// `(n, e)`: 最初の n 回の読みは e を返し、その後は読める (ロックの解除を模す)
    pub cursor_fail_until_reads: Option<(usize, BackendError)>,
    /// `(n, p)`: n 回の読みの後 (0 始まりで n 番目の読みの前) に、カーソルを p へ動かす
    /// (send の無い操作列で人の移動を起こす)
    pub cursor_moves_after_reads: Vec<(usize, Point)>,
    /// `find_window` が呼ばれた回数 (D-068)
    pub find_window_calls: usize,
    /// `(n, list)`: n 回の `find_window` の後 (0 始まりで n 番目の呼び出しの前) に `top` をこの一覧に置き換える
    /// (待っている間にウィンドウが現れる / 消えるのを模す。D-068)
    pub top_after_finds: Option<(usize, Vec<TopWindow>)>,
    /// `ui_elements` が呼ばれた回数 (失敗した読みも数える。D-068)
    pub ui_element_calls: usize,
    /// `(n, list)`: n 回の `ui_elements` の後 (0 始まりで n 番目の呼び出しの前) に `elements` をこの一覧に置き換える
    /// (待っている間に要素が現れる / 消えるのを模す。D-068)
    pub elements_after_reads: Option<(usize, Vec<(usize, crate::elements::UiElement)>)>,
    /// `(n, e)`: n 回目 (0 始まり) の `ui_elements` だけ e で失敗する (読む直前にウィンドウが閉じられた競合を模す。D-068)
    pub ui_elements_fail_at: Option<(usize, BackendError)>,
    /// `idle_ms` がこのエラーを返す (ロック中の読み取りの失敗を模す。D-073)。既定は None
    pub idle_ms_err: Option<BackendError>,
    /// `keys_held` がこのエラーを返す (D-073)。既定は None
    pub keys_held_err: Option<BackendError>,
    /// `foreground_window` がこのエラーを返す (D-073)。既定は None
    pub foreground_err: Option<BackendError>,
    /// トップレベルのウィンドウ (見えないものを含む) の持ち主 (D-074)。既定は空
    pub window_owners: Vec<WindowOwner>,
    /// 見えないトップレベルの窓 (`hidden_windows` が返す。前面から奥へ。D-099)。既定は空。close の `hidden` とは別
    pub hidden_listing: Vec<HiddenWindow>,
    /// `capture` が呼ばれた回数 (撮る前に断ったことを確かめる。D-075)
    pub capture_calls: usize,
    /// 撮影の判定 (D-077) に使う Z 順の窓 (前面から奥へ。`stack_windows` が返す)。既定は空
    pub stack: Vec<StackWindow>,
    /// `stack_windows` が呼ばれた回数 (deny の無い設定では列挙しないことを確かめる。D-077)
    pub stack_calls: usize,
    /// n 回目 (0 始まり) の `capture` は、先頭から `frames[n]` 点 (4 バイト単位) を 0x00、残りを 0x7f にした画像
    /// (n が範囲外なら最後の値、空なら全点 0x7f。画面の待ちを模す。D-081)
    pub frames: Vec<usize>,
    /// `capture` に渡された矩形 (呼ばれた順。D-081)
    pub capture_rects: Vec<Rect>,
    /// UI Automation の木 (ウィンドウの hwnd, 深さ, 要素)。hwnd ごとに前順で、根が深さ 0 (D-084)
    pub tree: Vec<(usize, usize, crate::elements::UiElement)>,
    /// `describe` が失敗する要素 (その hwnd の木の中の添字。D-084)
    pub tree_unreadable: Vec<usize>,
    /// `describe` が時間切れを返す要素 (その hwnd の木の中の添字。`ui_tree` は `BackendError::TimedOut` になる。D-110)
    pub tree_timed_out: Vec<usize>,
    /// `ui_tree` が呼ばれた回数 (失敗した読みも数える。D-084)
    pub ui_tree_calls: usize,
    /// Some なら `ui_tree` が毎回このエラーを返す (D-084)
    pub ui_tree_fail: Option<BackendError>,
    /// `focused_is_password` が返すもの (D-090)。既定は `Ok(None)` (読めなかった)
    pub focused_password: Result<Option<bool>, BackendError>,
    /// `focused_is_password` が呼ばれた回数 (失敗した読みも数える。D-090)
    pub focused_password_calls: usize,
    /// Some なら `focused_is_password` を読んだときに前面をこの窓に差し替える (読む間に前面が変わるのを模す。D-090)
    pub foreground_after_password_read: Option<WindowInfo>,
    /// Some((n, r)) なら、n 回呼ばれた後の `focused_is_password` は `focused_password` の代わりに r を返す
    /// (途中でフォーカスがパスワード欄に移るのを模す。D-090)
    pub password_after_calls: Option<(usize, Result<Option<bool>, BackendError>)>,
    /// Some(t) なら、`idle_ms` は固定の `idle_ms` の代わりに `now_ms - t` を返す (最後の入力の時刻。D-093)
    pub last_input_at_ms: Option<u64>,
    /// Some(t) なら、時計が t を過ぎた後の `idle_ms` の読みで `last_input_at_ms` を t にする
    /// (予告の間に人が触れたことを模す。t はモックの時計の時刻。D-093)
    pub input_at_lead_ms: Option<u64>,
    /// `MockIndicator::shown_for_ms` が返すもの (D-093)。既定は `Some(u64::MAX)` (枠が十分前から出ている = 予告を省く)
    pub indicator_shown_for: Option<u64>,
    /// `shown_for_ms` が読まれた回数
    pub indicator_reads: u32,
    /// `show` に渡された期限 (呼ばれた順)
    pub shows: Vec<u64>,
    /// `release` に渡された残す時間 (呼ばれた順)
    pub releases: Vec<u64>,
    /// `extend` に渡された残す時間 (呼ばれた順。D-107)
    pub extends: Vec<u64>,
    /// 枠への呼び出しの順 (`"show"` / `"release"` / `"extend"`。D-107)
    pub indicator_calls: Vec<&'static str>,
    /// `beep` が呼ばれた回数
    pub beeps: u32,
    /// `MockIndicator::frame_state` が返すもの (D-103)。既定は None (枠の装置を持たない)
    pub frame_state: Option<FrameState>,
    /// `MockInputLock::try_acquire` が偽 (別の持ち主がいる) を返す残りの回数 (D-113)。試みのたびに 1 減り、0 なら取れる。
    /// `usize::MAX` ならずっと取れない。既定は 0
    pub lock_busy: usize,
    /// `try_acquire` が呼ばれた回数 (Err を返した呼び出しも数える。D-113)
    pub lock_try_calls: usize,
    /// Some なら `try_acquire` がこのエラーを返す (D-113)。既定は None
    pub lock_err: Option<BackendError>,
    /// 錠への呼び出しの記録 (呼ばれた順。D-113)。Err を返した `try_acquire` は記録しない
    pub lock_events: Vec<LockEvent>,
}

/// `MockInputLock` への呼び出しの記録 (D-113)。呼ばれた時点のモックの時計の `now_ms`、送ったバッチの数 (`sent.len()`)、
/// `keys_held_calls`、枠への呼び出しの数 (`indicator_calls.len()`) を写す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockEvent {
    Acquire {
        got: bool,
        now_ms: u64,
        sent: usize,
        keys_held_calls: usize,
        indicator_calls: usize,
    },
    Release {
        now_ms: u64,
        sent: usize,
        keys_held_calls: usize,
        indicator_calls: usize,
    },
}

impl World {
    pub fn new() -> Self {
        World {
            screen: Rect::new(0, 0, 3840, 2160),
            cursor: Point::new(100, 100),
            sent: Vec::new(),
            foreground: Some(window("target.exe", "TargetClass")),
            windows_at: Vec::new(),
            windows_at_from_ms: None,
            windows_at_until_ms: None,
            default_window: Some(window("target.exe", "TargetClass")),
            self_elevated: false,
            refuse: None,
            user_moves: Vec::new(),
            foreground_after_sends: Vec::new(),
            now_ms: 0,
            top: Vec::new(),
            captions: Vec::new(),
            idle_ms: 60_000,
            focus_accepts: true,
            focus_no_effect: false,
            focus_redirect: None,
            focus_closes: false,
            foreground_reads_after_focus: None,
            foreground_reads: 0,
            owners: Vec::new(),
            focus_calls: Vec::new(),
            helper: HelperOutcome::Refused,
            helper_no_effect: false,
            helper_closes: false,
            helper_calls: Vec::new(),
            elements: Vec::new(),
            close_calls: Vec::new(),
            close_kept: false,
            close_fails: false,
            close_hides: false,
            hidden: Vec::new(),
            hidden_at_ms: None,
            hidden_then_destroyed_after_ms: None,
            hidden_then_visible_after_ms: None,
            disabled: Vec::new(),
            input_desktop: Ok(InputDesktop::Available),
            input_desktop_reads: 0,
            input_desktop_after_reads: None,
            held: HeldInputs::default(),
            keys_held_calls: 0,
            cursor_reads: 0,
            cursor_fail_after_reads: None,
            cursor_fail_until_reads: None,
            cursor_moves_after_reads: Vec::new(),
            find_window_calls: 0,
            top_after_finds: None,
            ui_element_calls: 0,
            elements_after_reads: None,
            ui_elements_fail_at: None,
            idle_ms_err: None,
            keys_held_err: None,
            foreground_err: None,
            window_owners: Vec::new(),
            hidden_listing: Vec::new(),
            capture_calls: 0,
            stack: Vec::new(),
            stack_calls: 0,
            frames: Vec::new(),
            capture_rects: Vec::new(),
            tree: Vec::new(),
            tree_unreadable: Vec::new(),
            tree_timed_out: Vec::new(),
            ui_tree_calls: 0,
            ui_tree_fail: None,
            focused_password: Ok(None),
            focused_password_calls: 0,
            password_after_calls: None,
            foreground_after_password_read: None,
            last_input_at_ms: None,
            input_at_lead_ms: None,
            indicator_shown_for: Some(u64::MAX),
            indicator_reads: 0,
            shows: Vec::new(),
            releases: Vec::new(),
            extends: Vec::new(),
            indicator_calls: Vec::new(),
            beeps: 0,
            frame_state: None,
            lock_busy: 0,
            lock_try_calls: 0,
            lock_err: None,
            lock_events: Vec::new(),
        }
    }

    pub fn shared(self) -> Rc<RefCell<World>> {
        Rc::new(RefCell::new(self))
    }

    /// 送られたイベントを平らにしたもの
    pub fn all_events(&self) -> Vec<InputEvent> {
        self.sent.concat()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

pub fn window(process: &str, class: &str) -> WindowInfo {
    WindowInfo {
        hwnd: 0x1000,
        pid: 4242,
        process: process.to_string(),
        class: class.to_string(),
        title: "mock".to_string(),
        elevated: Some(false),
    }
}

pub struct MockSink(pub Rc<RefCell<World>>);

impl InputSink for MockSink {
    fn send(&mut self, events: &[InputEvent]) -> Result<(), BackendError> {
        let mut w = self.0.borrow_mut();
        if let Some(e) = w.refuse.clone() {
            return Err(e);
        }
        for e in events {
            if let InputEvent::MoveTo { x, y } = e {
                w.cursor = Point::new(*x, *y);
            }
        }
        w.sent.push(events.to_vec());
        let n = w.sent.len() - 1;
        let moves: Vec<Point> = w
            .user_moves
            .iter()
            .filter(|(k, _)| *k == n)
            .map(|(_, p)| *p)
            .collect();
        if let Some(p) = moves.last() {
            w.cursor = *p;
        }
        let fg = w
            .foreground_after_sends
            .iter()
            .rfind(|(k, _)| *k == n)
            .map(|(_, info)| info.clone());
        if let Some(fg) = fg {
            w.foreground = Some(fg);
        }
        Ok(())
    }
}

pub struct MockDesktop(pub Rc<RefCell<World>>);

impl Desktop for MockDesktop {
    fn virtual_screen(&self) -> Result<Rect, BackendError> {
        Ok(self.0.borrow().screen)
    }

    fn cursor_pos(&self) -> Result<Point, BackendError> {
        let mut w = self.0.borrow_mut();
        let n = w.cursor_reads;
        w.cursor_reads += 1;
        let moved = w
            .cursor_moves_after_reads
            .iter()
            .rev()
            .find(|(k, _)| *k == n)
            .map(|(_, p)| *p);
        if let Some(p) = moved {
            w.cursor = p;
        }
        if let Some((until, e)) = &w.cursor_fail_until_reads
            && n < *until
        {
            return Err(e.clone());
        }
        if let Some((after, e)) = &w.cursor_fail_after_reads
            && n >= *after
        {
            return Err(e.clone());
        }
        Ok(w.cursor)
    }

    fn foreground_window(&self) -> Result<Option<WindowInfo>, BackendError> {
        let mut w = self.0.borrow_mut();
        if let Some(e) = &w.foreground_err {
            return Err(e.clone());
        }
        if let Some(limit) = w.foreground_reads_after_focus
            && !w.focus_calls.is_empty()
        {
            w.foreground_reads += 1;
            if w.foreground_reads > limit {
                return Err(BackendError::Os("GetForegroundWindow".into()));
            }
        }
        Ok(w.foreground.clone())
    }

    fn window_at(&self, p: Point) -> Result<Option<WindowInfo>, BackendError> {
        let w = self.0.borrow();
        // 時刻の範囲 [from, until) の外では重なりが無い (D-082)
        let active = w.windows_at_from_ms.is_none_or(|from| w.now_ms >= from)
            && w.windows_at_until_ms.is_none_or(|until| w.now_ms < until);
        Ok(w.windows_at
            .iter()
            .filter(|_| active)
            .find(|(r, _)| r.contains(p))
            .map(|(_, info)| info.clone())
            .or_else(|| w.default_window.clone()))
    }

    fn capture(&self, rect: Rect) -> Result<Image, BackendError> {
        let mut w = self.0.borrow_mut();
        let call = w.capture_calls;
        w.capture_calls += 1;
        w.capture_rects.push(rect);
        let dark = w.frames.get(call).or(w.frames.last()).copied().unwrap_or(0);
        let n = (rect.w as usize) * (rect.h as usize) * 4;
        let mut rgba = vec![0x7f; n];
        let k = dark.saturating_mul(4).min(n);
        rgba[..k].fill(0x00);
        Ok(Image {
            width: rect.w,
            height: rect.h,
            rgba,
        })
    }

    fn self_elevated(&self) -> bool {
        self.0.borrow().self_elevated
    }

    fn top_windows(&self) -> Result<Vec<TopWindow>, BackendError> {
        Ok(self.0.borrow().top.clone())
    }

    fn window_owners(&self) -> Result<Vec<WindowOwner>, BackendError> {
        Ok(self.0.borrow().window_owners.clone())
    }

    fn hidden_windows(&self) -> Result<Vec<HiddenWindow>, BackendError> {
        Ok(self.0.borrow().hidden_listing.clone())
    }

    fn stack_windows(&self) -> Result<Vec<StackWindow>, BackendError> {
        let mut w = self.0.borrow_mut();
        w.stack_calls += 1;
        Ok(w.stack.clone())
    }

    fn is_caption(&self, hwnd: usize, p: Point) -> bool {
        self.0
            .borrow()
            .captions
            .iter()
            .any(|(h, r)| *h == hwnd && r.contains(p))
    }

    fn idle_ms(&self) -> Result<u64, BackendError> {
        let mut w = self.0.borrow_mut();
        if let Some(e) = &w.idle_ms_err {
            return Err(e.clone());
        }
        // 予告の間の人の入力 (D-093): 時計がその時刻を過ぎていれば、最後の入力をその時刻にする
        if let Some(t) = w.input_at_lead_ms
            && w.now_ms >= t
            && w.last_input_at_ms.is_none_or(|last| last < t)
        {
            w.last_input_at_ms = Some(t);
        }
        Ok(match w.last_input_at_ms {
            Some(last) => w.now_ms.saturating_sub(last),
            None => w.idle_ms,
        })
    }

    fn input_desktop(&self) -> Result<InputDesktop, BackendError> {
        let mut w = self.0.borrow_mut();
        let n = w.input_desktop_reads;
        w.input_desktop_reads += 1;
        if let Some((after, r)) = &w.input_desktop_after_reads
            && n >= *after
        {
            return r.clone();
        }
        w.input_desktop.clone()
    }

    fn keys_held(&self) -> Result<HeldInputs, BackendError> {
        let mut w = self.0.borrow_mut();
        w.keys_held_calls += 1;
        match &w.keys_held_err {
            Some(e) => Err(e.clone()),
            None => Ok(w.held.clone()),
        }
    }

    fn find_window(
        &self,
        process: &str,
        title: Option<&str>,
    ) -> Result<Option<TopWindow>, BackendError> {
        let mut w = self.0.borrow_mut();
        let n = w.find_window_calls;
        w.find_window_calls += 1;
        if w.top_after_finds
            .as_ref()
            .is_some_and(|(after, _)| n >= *after)
            && let Some((_, list)) = w.top_after_finds.take()
        {
            w.top = list;
        }
        Ok(crate::device::match_window(&w.top, process, title))
    }

    fn set_foreground(&self, hwnd: usize) -> Result<bool, BackendError> {
        let mut w = self.0.borrow_mut();
        w.focus_calls.push(hwnd);
        if !w.focus_accepts {
            return Ok(false);
        }
        if w.focus_no_effect {
            return Ok(true);
        }
        if w.focus_closes {
            w.top.retain(|t| t.info.hwnd != hwnd);
            return Ok(true);
        }
        if let Some(r) = w.focus_redirect.clone() {
            w.foreground = Some(r);
            return Ok(true);
        }
        let info = w
            .top
            .iter()
            .find(|t| t.info.hwnd == hwnd)
            .map(|t| t.info.clone());
        if let Some(info) = info {
            w.foreground = Some(info);
        }
        Ok(true)
    }

    fn set_foreground_by_helper(&self, hwnd: usize) -> Result<HelperOutcome, BackendError> {
        let mut w = self.0.borrow_mut();
        w.helper_calls.push(hwnd);
        let outcome = w.helper.clone();
        if w.helper_closes {
            w.top.retain(|t| t.info.hwnd != hwnd);
            return Ok(outcome);
        }
        if outcome == HelperOutcome::Accepted && !w.helper_no_effect {
            let info = w
                .top
                .iter()
                .find(|t| t.info.hwnd == hwnd)
                .map(|t| t.info.clone());
            if let Some(info) = info {
                w.foreground = Some(info);
            }
        }
        Ok(outcome)
    }

    fn post_close(&self, hwnd: usize) -> Result<bool, BackendError> {
        let mut w = self.0.borrow_mut();
        w.close_calls.push(hwnd);
        if w.close_fails {
            return Ok(false);
        }
        if !w.close_kept {
            if w.close_hides {
                let now = w.now_ms;
                let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut w.top)
                    .into_iter()
                    .partition(|t| t.info.hwnd == hwnd);
                w.top = kept;
                w.hidden.extend(gone);
                w.hidden_at_ms = Some(now);
            } else {
                w.top.retain(|t| t.info.hwnd != hwnd);
            }
            if w.foreground.as_ref().is_some_and(|f| f.hwnd == hwnd) {
                w.foreground = None;
            }
        }
        Ok(true)
    }

    fn window_presence(&self, hwnd: usize, pid: u32) -> Result<WindowPresence, BackendError> {
        let mut w = self.0.borrow_mut();
        if let Some(at) = w.hidden_at_ms {
            let since = w.now_ms.saturating_sub(at);
            if w.hidden_then_destroyed_after_ms.is_some_and(|d| since >= d) {
                w.hidden.clear();
            } else if w.hidden_then_visible_after_ms.is_some_and(|d| since >= d) {
                let back = std::mem::take(&mut w.hidden);
                w.top.extend(back);
            }
        }
        let matches = |t: &TopWindow| t.info.hwnd == hwnd && t.info.pid == pid;
        Ok(if w.top.iter().any(matches) {
            WindowPresence::Visible
        } else if w.hidden.iter().any(matches) {
            WindowPresence::Hidden
        } else {
            WindowPresence::Destroyed
        })
    }

    fn window_enabled(&self, hwnd: usize) -> Result<bool, BackendError> {
        Ok(!self.0.borrow().disabled.contains(&hwnd))
    }

    fn owner_root(&self, hwnd: usize) -> usize {
        self.0
            .borrow()
            .owners
            .iter()
            .find(|(h, _)| *h == hwnd)
            .map_or(hwnd, |(_, root)| *root)
    }

    fn ui_elements(
        &self,
        hwnd: usize,
        query: &crate::elements::ElementQuery,
    ) -> Result<crate::elements::Elements, BackendError> {
        let mut w = self.0.borrow_mut();
        let n = w.ui_element_calls;
        w.ui_element_calls += 1;
        if w.elements_after_reads
            .as_ref()
            .is_some_and(|(after, _)| n >= *after)
            && let Some((_, list)) = w.elements_after_reads.take()
        {
            w.elements = list;
        }
        if let Some((at, e)) = &w.ui_elements_fail_at
            && n == *at
        {
            return Err(e.clone());
        }
        let mut out = crate::elements::Elements::default();
        for (h, e) in &w.elements {
            if *h != hwnd || !query.matches_element(e) {
                continue;
            }
            if query.skips_text_of_password(e.password) {
                // 実装置と同じ: パスワードの要素は文字列を持っていても読まずに飛ばして数える (D-090)
                out.password_skipped += 1;
                continue;
            }
            if query.with_text {
                // 実装置と同じ: 文字列を持つ最初の要素で打ち切る
                if e.text.is_some() {
                    let mut e = e.clone();
                    // 実装置と同じ: 文字列を読む問い合わせでは状態を読まない (D-089)
                    if !query.reads_states() {
                        e.toggle = None;
                        e.selected = None;
                        e.expanded = None;
                    }
                    out.elements.push(e);
                    break;
                }
                continue;
            }
            if out.elements.len() >= query.limit {
                out.truncated = true;
                break;
            }
            let mut e = e.clone();
            e.text = None;
            out.elements.push(e);
        }
        Ok(out)
    }

    fn ui_tree(
        &self,
        hwnd: usize,
        max_depth: usize,
        max_nodes: usize,
    ) -> Result<crate::elements::UiTree, BackendError> {
        let mut w = self.0.borrow_mut();
        w.ui_tree_calls += 1;
        if let Some(e) = &w.ui_tree_fail {
            return Err(e.clone());
        }
        // 実装置と同じ判定 (`walk_tree`) を、その hwnd の木の配列の上で通す
        let mut src = VecTreeSource::new(
            w.tree
                .iter()
                .filter(|(h, _, _)| *h == hwnd)
                .map(|(_, d, e)| (*d, e.clone()))
                .collect(),
        );
        if src.items.is_empty() {
            return Err(BackendError::Os(format!(
                "ウィンドウ {hwnd:#x} の要素の木が無い (モック)"
            )));
        }
        src.unreadable = w.tree_unreadable.clone();
        src.timed_out = w.tree_timed_out.clone();
        // 実装置と同じ写し (D-110): 時間切れは TimedOut、他の失敗は Os
        crate::elements::walk_tree(&src, 0, max_depth, max_nodes).map_err(|e| match e {
            crate::elements::ReadError::Failed(m) => BackendError::Os(m),
            crate::elements::ReadError::TimedOut(m) => BackendError::TimedOut(m),
        })
    }

    fn focused_is_password(&self) -> Result<Option<bool>, BackendError> {
        let mut w = self.0.borrow_mut();
        let n = w.focused_password_calls;
        w.focused_password_calls += 1;
        // 読む間に前面が変わったことを模す (D-090)
        if let Some(fg) = w.foreground_after_password_read.clone() {
            w.foreground = Some(fg);
        }
        match &w.password_after_calls {
            Some((after, r)) if n >= *after => r.clone(),
            _ => w.focused_password.clone(),
        }
    }
}

pub struct MockClock(pub Rc<RefCell<World>>);

impl Clock for MockClock {
    fn now_ms(&self) -> u64 {
        self.0.borrow().now_ms
    }

    fn sleep_ms(&mut self, ms: u64) {
        self.0.borrow_mut().now_ms += ms;
    }
}

/// モックの枠 (D-093)。読んだ回数、show の期限、release の残す時間、extend の残す時間、
/// 枠への呼び出しの順 (D-107)、音の回数を World に残す
pub struct MockIndicator(pub Rc<RefCell<World>>);

impl Indicator for MockIndicator {
    fn shown_for_ms(&self) -> Option<u64> {
        let mut w = self.0.borrow_mut();
        w.indicator_reads += 1;
        w.indicator_shown_for
    }

    fn show(&mut self, deadline_ms: u64) {
        let mut w = self.0.borrow_mut();
        w.shows.push(deadline_ms);
        w.indicator_calls.push("show");
    }

    fn release(&mut self, linger_ms: u64) {
        let mut w = self.0.borrow_mut();
        w.releases.push(linger_ms);
        w.indicator_calls.push("release");
    }

    fn extend(&mut self, linger_ms: u64) {
        let mut w = self.0.borrow_mut();
        w.extends.push(linger_ms);
        w.indicator_calls.push("extend");
    }

    fn beep(&mut self) {
        self.0.borrow_mut().beeps += 1;
    }

    fn frame_state(&self) -> Option<FrameState> {
        self.0.borrow().frame_state.clone()
    }
}

/// モックの入力の錠 (D-113)。`World::lock_busy` の回数だけ取れず、呼び出しを `World::lock_events` に残す。
/// 持っているかは数えない (実行器は取れたときだけ `release` を呼ぶ。呼ばれた事実をそのまま記録する)
pub struct MockInputLock(pub Rc<RefCell<World>>);

impl InputLock for MockInputLock {
    fn try_acquire(&mut self) -> Result<bool, BackendError> {
        let mut w = self.0.borrow_mut();
        w.lock_try_calls += 1;
        if let Some(e) = w.lock_err.clone() {
            return Err(e);
        }
        let got = w.lock_busy == 0;
        if !got && w.lock_busy != usize::MAX {
            w.lock_busy -= 1;
        }
        let ev = LockEvent::Acquire {
            got,
            now_ms: w.now_ms,
            sent: w.sent.len(),
            keys_held_calls: w.keys_held_calls,
            indicator_calls: w.indicator_calls.len(),
        };
        w.lock_events.push(ev);
        Ok(got)
    }

    fn release(&mut self) {
        let mut w = self.0.borrow_mut();
        let ev = LockEvent::Release {
            now_ms: w.now_ms,
            sent: w.sent.len(),
            keys_held_calls: w.keys_held_calls,
            indicator_calls: w.indicator_calls.len(),
        };
        w.lock_events.push(ev);
    }
}

/// モックの装置の工場。実入力の工場の代わりに、開発シェルの判定だけ本物と同じにする。
pub struct MockDevices(pub Rc<RefCell<World>>);

impl DeviceFactory for MockDevices {
    fn input(&mut self, env: &dyn EnvLookup) -> Result<Box<dyn InputSink>, BackendError> {
        crate::backend::general_allowed(env)?;
        Ok(Box::new(MockSink(self.0.clone())))
    }

    fn desktop(&mut self) -> Box<dyn Desktop> {
        Box::new(MockDesktop(self.0.clone()))
    }

    fn screen(&mut self) -> Option<Rect> {
        Some(self.0.borrow().screen)
    }

    fn indicator(&mut self) -> Box<dyn Indicator + '_> {
        Box::new(MockIndicator(self.0.clone()))
    }

    fn indicator_and_lock(&mut self) -> (Box<dyn Indicator + '_>, Box<dyn InputLock + '_>) {
        (
            Box::new(MockIndicator(self.0.clone())),
            Box::new(MockInputLock(self.0.clone())),
        )
    }
}

/// 呼ばれたら失敗させる装置の工場 (装置に一切触れないことの確認に使う)。
/// 画面の大きさ (読み取りだけ。D-058) は「読めない」として返す
pub struct NoDevices;

impl DeviceFactory for NoDevices {
    fn input(&mut self, _: &dyn EnvLookup) -> Result<Box<dyn InputSink>, BackendError> {
        panic!("このテストでは装置を作らない");
    }

    fn desktop(&mut self) -> Box<dyn Desktop> {
        panic!("このテストでは装置を作らない");
    }

    fn screen(&mut self) -> Option<Rect> {
        None
    }

    fn indicator(&mut self) -> Box<dyn Indicator + '_> {
        panic!("このテストでは装置を作らない");
    }

    fn indicator_and_lock(&mut self) -> (Box<dyn Indicator + '_>, Box<dyn InputLock + '_>) {
        panic!("このテストでは装置を作らない");
    }
}

/// 画面の観測だけを許し、入力の装置を作ろうとしたら失敗させる工場。
pub struct DesktopOnly(pub Rc<RefCell<World>>);

impl DeviceFactory for DesktopOnly {
    fn input(&mut self, _: &dyn EnvLookup) -> Result<Box<dyn InputSink>, BackendError> {
        panic!("このテストでは入力の装置を作らない");
    }

    fn desktop(&mut self) -> Box<dyn Desktop> {
        Box::new(MockDesktop(self.0.clone()))
    }

    fn screen(&mut self) -> Option<Rect> {
        Some(self.0.borrow().screen)
    }

    fn indicator(&mut self) -> Box<dyn Indicator + '_> {
        Box::new(MockIndicator(self.0.clone()))
    }

    fn indicator_and_lock(&mut self) -> (Box<dyn Indicator + '_>, Box<dyn InputLock + '_>) {
        (
            Box::new(MockIndicator(self.0.clone())),
            Box::new(MockInputLock(self.0.clone())),
        )
    }
}

/// 画像をメモリに溜める。
#[derive(Default)]
pub struct MemoryImages {
    pub written: Vec<(String, Image)>,
}

impl ImageWriter for MemoryImages {
    fn write_png(&mut self, path: &str, image: &Image) -> Result<String, String> {
        self.written.push((path.to_string(), image.clone()));
        Ok(path.to_string())
    }
}

/// テスト用の木 (`elements::TreeSource`。D-084)。前順の `(深さ, 要素)` の配列で、要素は添字で指す (根は添字 0)。
/// 子は「すぐ次の添字の深さが 1 深い」なら取れ、兄弟は「後ろで最初に深さが同じか浅い添字」が同じ深さなら取れる。
/// 失敗を差し込む添字の表: `unreadable` は `describe` が `ReadError::Failed`、`child_fail` は `first_child` が `Failed`、
/// `sibling_fail` は `next_sibling` が `Failed`。時間切れを差し込む添字の表 (D-110): `timed_out` は `describe` が
/// `ReadError::TimedOut`、`child_timeout` は `first_child` が `WalkStep::TimedOut`、`sibling_timeout` は `next_sibling` が
/// `WalkStep::TimedOut`。`describes` は `describe` を呼んだ回数 (試した要素の数)
#[derive(Debug, Default)]
pub struct VecTreeSource {
    pub items: Vec<(usize, crate::elements::UiElement)>,
    pub unreadable: Vec<usize>,
    pub child_fail: Vec<usize>,
    pub sibling_fail: Vec<usize>,
    pub timed_out: Vec<usize>,
    pub child_timeout: Vec<usize>,
    pub sibling_timeout: Vec<usize>,
    pub describes: std::cell::Cell<usize>,
}

impl VecTreeSource {
    pub fn new(items: Vec<(usize, crate::elements::UiElement)>) -> VecTreeSource {
        VecTreeSource {
            items,
            ..Default::default()
        }
    }
}

impl crate::elements::TreeSource for VecTreeSource {
    type Node = usize;

    fn describe(
        &self,
        n: &usize,
    ) -> Result<crate::elements::UiElement, crate::elements::ReadError> {
        use crate::elements::ReadError;
        self.describes.set(self.describes.get() + 1);
        if self.timed_out.contains(n) {
            return Err(ReadError::TimedOut(format!(
                "要素 {n} の読みが時間切れ (テスト)"
            )));
        }
        if self.unreadable.contains(n) {
            return Err(ReadError::Failed(format!("要素 {n} は読めない (テスト)")));
        }
        self.items
            .get(*n)
            .map(|(_, e)| e.clone())
            .ok_or_else(|| ReadError::Failed(format!("要素 {n} は無い (テスト)")))
    }

    fn first_child(&self, n: &usize) -> crate::elements::WalkStep<usize> {
        use crate::elements::WalkStep;
        if self.child_timeout.contains(n) {
            return WalkStep::TimedOut(format!("要素 {n} の子の取得が時間切れ (テスト)"));
        }
        if self.child_fail.contains(n) {
            return WalkStep::Failed;
        }
        match (self.items.get(*n), self.items.get(n + 1)) {
            (Some((d, _)), Some((c, _))) if *c == d + 1 => WalkStep::Next(n + 1),
            _ => WalkStep::End,
        }
    }

    fn next_sibling(&self, n: &usize) -> crate::elements::WalkStep<usize> {
        use crate::elements::WalkStep;
        if self.sibling_timeout.contains(n) {
            return WalkStep::TimedOut(format!("要素 {n} の兄弟の取得が時間切れ (テスト)"));
        }
        if self.sibling_fail.contains(n) {
            return WalkStep::Failed;
        }
        let Some((d, _)) = self.items.get(*n) else {
            return WalkStep::End;
        };
        match self
            .items
            .iter()
            .enumerate()
            .skip(n + 1)
            .find(|(_, (e, _))| e <= d)
        {
            Some((j, (e, _))) if e == d => WalkStep::Next(j),
            _ => WalkStep::End,
        }
    }
}

/// target/test-tmp の項目は、最後の更新からこの長さを過ぎたら古いとみなす (D-087)
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// 1 プロセスの掃除にかける時間の予算 (D-087)
const SWEEP_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// テストの一時の場所の根 `<CARGO_MANIFEST_DIR>/target/test-tmp`
fn test_tmp_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
}

/// `target/test-tmp/<名前>-<プロセス ID>-<時刻>` を作って返す。テストは毎回新しい場所を使う。
/// プロセスの最初の呼び出しで、target/test-tmp の 24 時間より古いものを 2 秒まで消す。
/// 失敗したテストの場所は 1 日残る (D-087)。
pub fn unique_temp_dir(name: &str) -> PathBuf {
    static SWEEP: std::sync::Once = std::sync::Once::new();
    let root = test_tmp_root();
    SWEEP.call_once(|| {
        if let Some(cutoff) = std::time::SystemTime::now().checked_sub(STALE_AFTER) {
            let _ = sweep_stale_entries(&root, cutoff, SWEEP_BUDGET);
        }
    });
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = root.join(format!("{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("target/test-tmp を作れない");
    dir
}

/// target/test-tmp の掃除用。`dir` の直下だけを見て、更新時刻が `cutoff` より前の項目を消す。
/// リンク (ジャンクションを含む) はたどらず消さない。読めない項目と消せなかった項目は黙って飛ばす。
/// 始めてから `budget` を過ぎたら次の項目に進まずに止める。返り値は消せた項目の数 (D-087)
fn sweep_stale_entries(
    dir: &std::path::Path,
    cutoff: std::time::SystemTime,
    budget: std::time::Duration,
) -> usize {
    let start = std::time::Instant::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries {
        if start.elapsed() >= budget {
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }
        let result = if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if result.is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 実の時計 (`runner::execute` の `SystemClock`) で走る `wait_for` の時間切れ (`timeout_ms` 300) の判定の回数を断言する。
/// 最初の 250 ms の眠りが 300 ms を超えると、判定は 2 回で期限に届く (D-085)。
/// 回数の完全一致は src/run.rs のモックの時計のテストが見る。
#[track_caller]
pub fn assert_real_clock_timeout_checks(detail: &serde_json::Value) {
    let checks = detail["checks"].as_u64();
    assert!(
        checks.is_some_and(|c| (2..=3).contains(&c)),
        "実の時計の時間切れの checks は 2 以上 3 以下: {detail}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 既定は「使える」「押されたままのものは無い」(D-066)。keys_held は呼ばれた回数を数える
    #[test]
    fn mock_input_desktop_and_keys_held_defaults() {
        let w = World::new().shared();
        let d = MockDesktop(w.clone());
        assert_eq!(d.input_desktop(), Ok(InputDesktop::Available));
        assert_eq!(d.keys_held(), Ok(HeldInputs::default()));
        assert_eq!(d.keys_held().map(|h| h.is_empty()), Ok(true));
        assert_eq!(w.borrow().keys_held_calls, 2);
        assert_eq!(w.borrow().input_desktop_reads, 1);
    }

    /// `input_desktop_after_reads = (n, r)` は n 回目までは `input_desktop` を返し、その後は r を返す
    #[test]
    fn mock_input_desktop_after_reads_changes_after_n_reads() {
        let w = World::new().shared();
        let locked = Ok(InputDesktop::Locked {
            name: Some("Default".into()),
        });
        w.borrow_mut().input_desktop_after_reads = Some((1, locked.clone()));
        let d = MockDesktop(w.clone());
        assert_eq!(d.input_desktop(), Ok(InputDesktop::Available));
        assert_eq!(d.input_desktop(), locked);
        assert_eq!(d.input_desktop(), locked);
        assert_eq!(w.borrow().input_desktop_reads, 3);
    }

    /// `frames` (D-081): n 回目の capture は先頭 frames[n] 点が 0x00 で残りが 0x7f。範囲外は最後の値。
    /// 空なら全点 0x7f。撮った矩形は capture_rects に残る
    #[test]
    fn mock_capture_frames_and_rects() {
        let r = Rect::new(10, 20, 3, 2);
        let empty = MockDesktop(World::new().shared());
        assert_eq!(
            empty.capture(r).unwrap().rgba,
            vec![0x7f; 24],
            "frames が空"
        );

        let w = World::new().shared();
        w.borrow_mut().frames = vec![0, 2];
        let d = MockDesktop(w.clone());
        let mut want = vec![0x00; 8];
        want.extend(vec![0x7f; 16]);
        assert_eq!(d.capture(r).unwrap().rgba, vec![0x7f; 24], "1 回目は 0 点");
        assert_eq!(d.capture(r).unwrap().rgba, want, "2 回目は先頭 2 点");
        let third = d.capture(r).unwrap();
        assert_eq!((third.width, third.height), (3, 2));
        assert_eq!(third.rgba, want, "範囲外は最後の値");
        assert_eq!(w.borrow().capture_rects, vec![r, r, r]);
        assert_eq!(w.borrow().capture_calls, 3);
    }

    /// `cursor_fail_after_reads = (n, e)` は n 回目までは読め、その後は e を返す
    #[test]
    fn mock_cursor_fail_after_reads_succeeds_n_times() {
        let w = World::new().shared();
        let e = BackendError::DesktopUnavailable("lock".into());
        w.borrow_mut().cursor_fail_after_reads = Some((2, e.clone()));
        let d = MockDesktop(w.clone());
        assert_eq!(d.cursor_pos(), Ok(Point::new(100, 100)));
        assert_eq!(d.cursor_pos(), Ok(Point::new(100, 100)));
        assert_eq!(d.cursor_pos(), Err(e.clone()));
        assert_eq!(d.cursor_pos(), Err(e.clone()));
        assert_eq!(w.borrow().cursor_reads, 4);

        let w0 = World::new().shared();
        w0.borrow_mut().cursor_fail_after_reads = Some((0, e.clone()));
        assert_eq!(
            MockDesktop(w0).cursor_pos(),
            Err(e),
            "(0, e) は最初から失敗"
        );
    }

    /// `cursor_fail_until_reads = (n, e)` は最初の n 回だけ失敗し (ロックの解除)、
    /// `cursor_moves_after_reads = (n, p)` は n 回の読みの後に p へ動かす
    #[test]
    fn mock_cursor_fail_until_reads_and_moves_after_reads() {
        let w = World::new().shared();
        let e = BackendError::DesktopUnavailable("lock".into());
        w.borrow_mut().cursor_fail_until_reads = Some((2, e.clone()));
        w.borrow_mut().cursor_moves_after_reads = vec![(3, Point::new(900, 900))];
        let d = MockDesktop(w);
        assert_eq!(d.cursor_pos(), Err(e.clone()));
        assert_eq!(d.cursor_pos(), Err(e));
        assert_eq!(d.cursor_pos(), Ok(Point::new(100, 100)), "解除された");
        assert_eq!(
            d.cursor_pos(),
            Ok(Point::new(900, 900)),
            "3 回の読みの後に動いた"
        );
    }

    fn tree_el(name: &str) -> crate::elements::UiElement {
        crate::elements::UiElement {
            name: name.into(),
            control_type: "button".into(),
            automation_id: String::new(),
            class: String::new(),
            rect: None,
            enabled: true,
            offscreen: false,
            focused: false,
            password: false,
            toggle: None,
            selected: None,
            expanded: None,
            text: None,
        }
    }

    /// `ui_tree` はその hwnd の木だけを `walk_tree` でたどり、深さで切る。`tree_unreadable` は skipped に、
    /// `ui_tree_fail` はそのまま Err に、木の無い hwnd は Os の Err になる。呼んだ回数を数える (D-084)
    #[test]
    fn mock_ui_tree_filters_by_hwnd_and_cuts_depth() {
        let w = World::new().shared();
        // 別の窓の木を先に置く (絞らなければ、こちらの根から読んでしまう)
        w.borrow_mut().tree = vec![
            (0x20, 0, tree_el("B")),
            (0x20, 1, tree_el("B1")),
            (0x10, 0, tree_el("A")),
            (0x10, 1, tree_el("A1")),
            (0x10, 2, tree_el("A11")),
            (0x10, 1, tree_el("A2")),
        ];
        let d = MockDesktop(w.clone());
        let t = d.ui_tree(0x10, 1, 100).unwrap();
        let got: Vec<(usize, String)> = t
            .nodes
            .iter()
            .map(|n| (n.depth, n.element.name.clone()))
            .collect();
        assert_eq!(
            got,
            vec![(0, "A".into()), (1, "A1".into()), (1, "A2".into())]
        );
        assert_eq!((t.depth_cut, t.skipped, t.node_cap_hit), (1, 0, false));
        assert_eq!(w.borrow().ui_tree_calls, 1);

        // 0x10 の木の添字 3 (A2) が読めない
        w.borrow_mut().tree_unreadable = vec![3];
        let t = d.ui_tree(0x10, 5, 100).unwrap();
        assert_eq!(t.nodes.len(), 3, "{t:?}");
        assert_eq!(t.skipped, 1);

        assert!(matches!(d.ui_tree(0x30, 5, 100), Err(BackendError::Os(_))));
        let e = BackendError::Os("gone".into());
        w.borrow_mut().ui_tree_fail = Some(e.clone());
        assert_eq!(d.ui_tree(0x10, 5, 100), Err(e));
        assert_eq!(w.borrow().ui_tree_calls, 4);
    }

    /// `tree_timed_out` の要素の時間切れは、`ui_tree` で `BackendError::TimedOut` に写る (D-110)
    #[test]
    fn mock_ui_tree_maps_timeout_to_timed_out() {
        let w = World::new().shared();
        w.borrow_mut().tree = vec![
            (0x10, 0, tree_el("A")),
            (0x10, 1, tree_el("A1")),
            (0x10, 1, tree_el("A2")),
        ];
        let d = MockDesktop(w.clone());
        assert!(d.ui_tree(0x10, 5, 100).is_ok());
        w.borrow_mut().tree_timed_out = vec![1];
        assert!(
            matches!(d.ui_tree(0x10, 5, 100), Err(BackendError::TimedOut(_))),
            "時間切れが TimedOut に写らない"
        );
        // 根の時間切れも同じ
        w.borrow_mut().tree_timed_out = vec![0];
        assert!(matches!(
            d.ui_tree(0x10, 5, 100),
            Err(BackendError::TimedOut(_))
        ));
    }

    use std::time::{Duration, SystemTime};

    /// root の兄弟 (`<root の名前>-<suffix>`) のパス。test-tmp の直下に置き、24 時間後に本番の掃除で消える
    fn sibling(root: &std::path::Path, suffix: &str) -> PathBuf {
        let name = root.file_name().unwrap().to_string_lossy();
        root.parent().unwrap().join(format!("{name}-{suffix}"))
    }

    /// 掃除の場所を作る: root の下にフォルダ 2 つ (中にファイル 1 つずつ) とファイル 1 つ、root の兄弟に目印のファイル
    fn sweep_fixture(name: &str) -> (PathBuf, PathBuf) {
        let root = unique_temp_dir(name);
        for d in ["a", "b"] {
            std::fs::create_dir(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("f.txt"), "x").unwrap();
        }
        std::fs::write(root.join("c.txt"), "x").unwrap();
        let marker = sibling(&root, "sibling.txt");
        std::fs::write(&marker, "x").unwrap();
        (root, marker)
    }

    fn entry_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    /// 境より古い項目 (フォルダもファイルも) を消し、root 自体と root の兄弟は残す (D-087)
    #[test]
    fn sweep_stale_entries_removes_old_dirs_and_files_only_under_dir() {
        let (root, marker) = sweep_fixture("sweep-removes");
        let cutoff = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(
            sweep_stale_entries(&root, cutoff, Duration::from_secs(10)),
            3
        );
        assert_eq!(entry_count(&root), 0, "root の直下が空");
        assert!(root.is_dir(), "root 自体は残る");
        assert!(marker.is_file(), "root の兄弟は残る");
    }

    /// 境より新しい項目は消さない
    #[test]
    fn sweep_stale_entries_keeps_entries_newer_than_cutoff() {
        let (root, _) = sweep_fixture("sweep-keeps");
        let cutoff = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(
            sweep_stale_entries(&root, cutoff, Duration::from_secs(10)),
            0
        );
        assert_eq!(entry_count(&root), 3);
    }

    /// 予算が 0 なら、全部が古くても 1 つも消さずに止まる
    #[test]
    fn sweep_stale_entries_stops_at_zero_budget() {
        let (root, _) = sweep_fixture("sweep-budget");
        let cutoff = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(sweep_stale_entries(&root, cutoff, Duration::ZERO), 0);
        assert_eq!(entry_count(&root), 3);
    }

    /// 無い場所は 0
    #[test]
    fn sweep_stale_entries_returns_zero_for_missing_dir() {
        let root = unique_temp_dir("sweep-missing");
        let cutoff = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(
            sweep_stale_entries(&root.join("none"), cutoff, Duration::from_secs(10)),
            0
        );
    }

    /// 直下のジャンクションは数えず消さず、先の中身も残す (std の `symlink_metadata` はジャンクションを
    /// リンクと見る)
    #[test]
    fn sweep_stale_entries_skips_junction_and_keeps_its_target() {
        let root = unique_temp_dir("sweep-junction");
        let target = sibling(&root, "target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "x").unwrap();
        let link = root.join("link");
        let status = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("cmd を起動できない");
        assert!(status.success(), "ジャンクションを作れない: {status}");
        std::fs::create_dir(root.join("old")).unwrap();

        let cutoff = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(
            sweep_stale_entries(&root, cutoff, Duration::from_secs(10)),
            1,
            "ジャンクションは数えない"
        );
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "ジャンクションそのものが残る"
        );
        assert!(!root.join("old").exists(), "古いフォルダは消える");
        assert!(target.join("keep.txt").is_file(), "先の中身が残る");
    }
}
