//! 前面の切り替わりの見張り (D-166)。専用のスレッドで `SetWinEventHook(EVENT_SYSTEM_FOREGROUND,
//! WINEVENT_OUTOFCONTEXT)` を受け、前面のルートの窓が移るたびに `ForegroundLog` へ 1 件ずつ足す。
//! 前面が無くなる移り (ロック、安全なデスクトップ、前面の窓が消えて次が無い) ではイベントが来ないので、同じスレッドの
//! タイマー (既定 500 ms) で `GetForegroundWindow` を読み直して補う (重複は `record` が落とす)。
//!
//! スレッドの立て方と止め方は画面の縁の枠のスレッド (`overlay.rs`) と同じ型: キューを作ってから ready で成否を送り
//! 返し (フックとタイマーと initial の記録まで済ませてから)、`stop` は印を立てて WM_QUIT を投げ、done を時間切れ
//! つきで待つ。`WINEVENT_OUTOFCONTEXT` のコールバックは掛けたスレッドのメッセージの輪の中で呼ばれるので、記録の先は
//! スレッドローカルに置く。記録は題名を読まない (`window::root_info_untitled`)。

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND as WinHwnd;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetForegroundWindow, GetMessageW, KillTimer, MSG,
    OBJID_WINDOW, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetTimer, WINEVENT_OUTOFCONTEXT,
    WM_QUIT, WM_TIMER, WM_USER,
};

use crate::foreground_log::{ForegroundLog, WatchState, now_unix_ms};

/// 前面が無くなる移りを補う読み直しの間隔 (本番の既定)
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 起動 (フック、タイマー、initial の記録) を待つ上限
const START_TIMEOUT: Duration = Duration::from_secs(1);
/// `stop` で輪の終わりを待つ上限
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

thread_local! {
    /// 見張りのスレッドの記録の先。コールバックとタイマーがここから読む (見張りのスレッドにだけ置く)
    static LOG: RefCell<Option<Arc<ForegroundLog>>> = const { RefCell::new(None) };
}

/// 見張りの立て方。本番は `spawn` (フックとタイマーの両方)。実機のテストがフックとタイマーを分けて撃つために使う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchOptions {
    /// `SetWinEventHook` を掛けるか
    pub hook: bool,
    /// 前面を読み直すタイマーの間隔。None ならタイマーを立てない
    pub poll: Option<Duration>,
}

/// 立っている見張り。`stop` で止める。`Drop` では何もしない (常駐の MCP はプロセスの終わりまで持つ)
#[derive(Debug)]
pub struct ForegroundWatch {
    log: Arc<ForegroundLog>,
    thread_id: u32,
    stopping: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

/// 本番の見張りを立てる (フックと 500 ms のタイマー)
pub fn spawn(log: Arc<ForegroundLog>) -> Result<ForegroundWatch, String> {
    spawn_with(
        log,
        WatchOptions {
            hook: true,
            poll: Some(POLL_INTERVAL),
        },
    )
}

/// 見張りを立て、フック・タイマー・起動の直後の前面 (initial) の記録が済んで状態が `Running` になるまで待つ
/// (最大 1 秒)。立てられなければ状態を `Failed` にして理由を返す。時間切れで去ったスレッドは、先に立った `Failed`
/// を見たところか、ready の送りに失敗したところで自分で片付く
pub fn spawn_with(log: Arc<ForegroundLog>, opts: WatchOptions) -> Result<ForegroundWatch, String> {
    // 容量 0 (ランデブー): 受け手が去った後に ready を送ったスレッドは、送りの失敗で自分で片付く
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let (done_tx, done_rx) = mpsc::channel();
    let stopping = Arc::new(AtomicBool::new(false));
    let (thread_log, thread_stopping) = (Arc::clone(&log), Arc::clone(&stopping));
    let spawned = std::thread::Builder::new()
        .name("ai-cu-foreground".into())
        .spawn(move || {
            let panic_log = Arc::clone(&thread_log);
            let r = catch_unwind(AssertUnwindSafe(|| {
                watch_thread(thread_log, opts, &thread_stopping, ready_tx, done_tx)
            }));
            if r.is_err() {
                panic_log.set_state(WatchState::Failed(
                    "前面の見張りのスレッドが panic で終わった".into(),
                ));
            }
        });
    let fail = |m: String| {
        log.set_state(WatchState::Failed(m.clone()));
        Err(m)
    };
    let handle = match spawned {
        Ok(h) => h,
        Err(e) => return fail(format!("前面の見張りのスレッドを立てられない: {e}")),
    };
    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(thread_id)) => Ok(ForegroundWatch {
            log: Arc::clone(&log),
            thread_id,
            stopping,
            done: done_rx,
            handle: Some(handle),
        }),
        Ok(Err(e)) => {
            // スレッドは Err を送った後に片付けて抜ける
            let _ = handle.join();
            fail(e)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => fail("前面の見張りの起動が時間切れ".into()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            fail("前面の見張りのスレッドが起動の途中で終わった".into())
        }
    }
}

impl ForegroundWatch {
    /// 見張りを止める。「止めた」の印を立て、輪が終わるまで最大 1 秒待って join する。止まって join できれば true、
    /// WM_QUIT を投げられない・時間内に終わらないときは join せずに切り離して false。どの終わり方でも、返った後の
    /// 状態は `Failed` (切り離したスレッドが記録を続けても、since_ms は返さない)
    pub fn stop(mut self) -> bool {
        self.stopping.store(true, Ordering::SeqCst);
        let stopped = self.stop_thread();
        // 正常に止まればスレッドが既に Failed にしている (一方向なので理由は上書きされない)
        self.log
            .set_state(WatchState::Failed("前面の見張りを止められない".into()));
        stopped
    }

    fn stop_thread(&mut self) -> bool {
        let Some(handle) = self.handle.take() else {
            return false;
        };
        if handle.is_finished() {
            return handle.join().is_ok();
        }
        // SAFETY: spawn_with が ready を受けた (キューを作り終えた) スレッドの ID に、窓に結びつかない WM_QUIT を
        //         置くだけ。handle (JoinHandle) がスレッドのハンドルを開いたままなので、スレッドが終わっていても ID は
        //         別のスレッドに再利用されず、無関係なスレッドに届くことは無い (終わっていれば 0 が返るだけ)。
        //         他のスレッドからは PostQuitMessage を呼べないので、overlay.rs と同じく WM_QUIT を直に置く。
        let posted = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if posted == 0 {
            return false;
        }
        if self.done.recv_timeout(STOP_TIMEOUT).is_ok() {
            handle.join().is_ok()
        } else {
            false
        }
    }
}

/// 立てたタイマーとフックを外す (無いものは飛ばす)
fn release(timer: usize, hook: Option<HWINEVENTHOOK>) {
    if timer != 0 {
        // SAFETY: このスレッドが hwnd なしで立てたタイマーの ID を消す。既に無ければ失敗するだけ。
        unsafe { KillTimer(std::ptr::null_mut(), timer) };
    }
    if let Some(h) = hook {
        // SAFETY: このスレッドの SetWinEventHook が返した有効なフックを 1 回だけ外す。
        unsafe {
            let _ = UnhookWinEvent(h);
        }
    }
}

fn set_thread_log(log: Option<Arc<ForegroundLog>>) {
    let _ = LOG.try_with(|l| {
        if let Ok(mut g) = l.try_borrow_mut() {
            *g = log;
        }
    });
}

fn watch_thread(
    log: Arc<ForegroundLog>,
    opts: WatchOptions,
    stopping: &AtomicBool,
    ready: mpsc::SyncSender<Result<u32, String>>,
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
    set_thread_log(Some(Arc::clone(&log)));
    let fail = |e: String, timer: usize, hook: Option<HWINEVENTHOOK>| {
        release(timer, hook);
        log.set_state(WatchState::Failed(e.clone()));
        set_thread_log(None);
        let _ = ready.send(Err(e));
        let _ = done.send(());
    };
    let hook = if opts.hook {
        // SAFETY: コールバックは 'static の関数で、panic を外へ出さない (catch_unwind)。OUTOFCONTEXT なので他の
        //         プロセスにコードを入れず、このスレッドのメッセージの輪の中で呼ばれる。モジュールは要らない (None)。
        let h = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if h.is_invalid() {
            fail("SetWinEventHook が失敗".into(), 0, None);
            return;
        }
        Some(h)
    } else {
        None
    };
    let timer = match opts.poll {
        Some(every) => {
            let ms = u32::try_from(every.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: hwnd なしのスレッドのタイマー (ID は OS が選ぶ)。コールバックは無く、WM_TIMER を輪で受ける。
            let id = unsafe { SetTimer(std::ptr::null_mut(), 0, ms, None) };
            if id == 0 {
                let err = super::last_error();
                fail(
                    format!("前面を読み直すタイマーを張れない (SetTimer、GetLastError={err})"),
                    0,
                    hook,
                );
                return;
            }
            id
        }
        None => 0,
    };
    // フックを掛けてから読むので、その間の切り替わりは輪の中で後から届き、届いたときに読み直す (同じ窓なら重複で落ちる)。
    // initial を記録してから Running にするので、Running を見た読み手は必ず initial を持つ
    record_current(true);
    log.set_state(WatchState::Running);
    if let WatchState::Failed(e) = log.state() {
        // initial の記録で panic したか、spawn_with が時間切れで先に Failed にした (状態は一方向なので Running にならない)
        fail(e, timer, hook);
        return;
    }
    if ready.send(Ok(thread_id)).is_err() {
        // 呼び出し側が待つのをやめた (時間切れ)。Running → Failed は許された移り
        log.set_state(WatchState::Failed("前面の見張りの起動が時間切れ".into()));
        release(timer, hook);
        set_thread_log(None);
        let _ = done.send(());
        return;
    }
    loop {
        // SAFETY: msg は書き込み可能な MSG。このスレッドのメッセージを受け取る (WinEvent のコールバックもこの中で呼ばれる)。
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        // 0 は WM_QUIT、-1 は誤り
        if r <= 0 {
            break;
        }
        if msg.hwnd.is_null() && msg.message == WM_TIMER && timer != 0 && msg.wParam == timer {
            record_current(false);
            continue;
        }
        // SAFETY: GetMessageW が埋めた有効な MSG を渡す。
        unsafe { DispatchMessageW(&msg) };
    }
    release(timer, hook);
    // 止めた後も Running のまま残すと since_ms を返してしまうので、どちらの終わりでも Failed にする
    let reason = if stopping.load(Ordering::SeqCst) {
        "見張りを止めた"
    } else {
        "見張りのスレッドが終わった"
    };
    log.set_state(WatchState::Failed(reason.into()));
    set_thread_log(None);
    let _ = done.send(());
}

/// 今の前面を記録する (イベント、タイマー、initial)
fn record_current(initial: bool) {
    record_guarded(
        || {
            // SAFETY: 引数なし。前面のウィンドウのハンドル (無ければ null) を返すだけ。
            let h = unsafe { GetForegroundWindow() };
            h as usize
        },
        initial,
    );
}

/// `hwnd` を読んで (0 なら前面なし) スレッドローカルの記録に足す。panic は外へ出さず、状態を `Failed` にする
/// (1 件を落としたまま「保った」と言わないため)
fn record_guarded(hwnd: impl FnOnce() -> usize, initial: bool) {
    let log = LOG
        .try_with(|l| l.try_borrow().ok().and_then(|g| g.clone()))
        .ok()
        .flatten();
    let Some(log) = log else {
        return;
    };
    let r = catch_unwind(AssertUnwindSafe(|| {
        let window = super::window::root_info_untitled(hwnd());
        log.record(window, Instant::now(), now_unix_ms(), initial);
    }));
    if r.is_err() {
        log.set_state(WatchState::Failed("前面の記録で panic".into()));
    }
}

/// `EVENT_SYSTEM_FOREGROUND` のコールバック。窓そのもの (`OBJID_WINDOW`) の知らせを、前面を読み直す合図として使う。
/// イベントの hwnd は生まれた時点の前面で、タイマーと initial の `GetForegroundWindow` の読みとの前後は保証されない
/// (OUTOFCONTEXT のイベントどうしの順だけが保証される)。hwnd をそのまま記録すると、読み直しで C を記録した後に
/// 遅れて届いた B→C のイベントが C, B, C の偽の移りを作るので、記録は常に今の前面の読みにそろえる (一瞬だけの前面は
/// 落ちうる。win32-reviewer の指摘。D-166)
extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: WinHwnd,
    id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND && id_object == OBJID_WINDOW {
        record_current(false);
    }
}
