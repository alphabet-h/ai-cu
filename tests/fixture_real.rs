//! フィクスチャ限定の実機テスト (D-006、D-020)。`cargo test -- --ignored --nocapture --test-threads=1` で、
//! 主エージェントだけが直列に走らせる。
//!
//! 守っていること:
//! - 入力は自前のフィクスチャウィンドウ (最前面) のクライアント領域の中だけ。実バックエンドはフィクスチャ専用の
//!   コンストラクタで作り、送る前にすべてのイベントについて「矩形の中」「座標の下がフィクスチャ」を確かめる
//! - キーと文字は、前面がフィクスチャであることを確かめてから送る。前面にできなければ、キーを 1 つも送らずに失敗にする
//! - 前面化は `SetForegroundWindow` を試し、だめならフィクスチャの矩形の中をクリックする。Alt の空打ちと
//!   `SystemParametersInfo` の変更はしない。Win+、Alt+F4、Ctrl+Alt+ 系は送らない (組み込みの拒否でも止まる)
//! - 撮影はフィクスチャの領域だけ。画像は target/ の下に保存し、テストの中で画素を確かめる
//! - 各テストにウォッチドッグを置き、期限を過ぎたらプロセスごと終わらせる
//! - 予告の間 (D-093) のテストだけは、人の手の代わりに別スレッドからフィクスチャの中へ入力を送る (D-095)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ai_cu::backend::{FixtureScope, RealIndicator, RealSink, Win32Desktop};
use ai_cu::config::{Config, IndicatorConfig, Layer};
use ai_cu::device::{
    BackendError, Desktop, HeldInputs, Indicator, InputDesktop, InputEvent, InputLock, InputSink,
    NoIndicator, NoInputLock, StackWindow, SystemClock, WindowPresence,
};
use ai_cu::elements::{
    ElementQuery, Elements, ReadError, TREE_DEPTH_DEFAULT, TREE_LIMIT_DEFAULT, TREE_MAX_NODES,
    UiElement, UiTree, first_text, normalize_newlines, render_tree,
};
use ai_cu::error::{AppError, ErrorKind};
use ai_cu::estop::StopSet;
use ai_cu::geometry::{Point, Rect};
use ai_cu::hint::{LEAD_USER_ACTIVE_HINT, Surface};
use ai_cu::image::{PngFileWriter, decode_png, frame_diff, thumb};
use ai_cu::policy::Policy;
use ai_cu::run::{self, ExecOptions, Executor, RunReport};
use ai_cu::runner::{DeviceFactory, RealDevices};
use ai_cu::script::{Button, Script};
use ai_cu::win32::fixture_window::{
    FIXTURE_BUTTON_ID, FIXTURE_BUTTON_TEXT, FIXTURE_EDIT_ID, FIXTURE_EDIT_TEXT,
    FIXTURE_PASSWORD_ID, FIXTURE_PASSWORD_TEXT, FIXTURE_RGB, FixtureEvent, FixtureRegion,
    FixtureWindow,
};
use ai_cu::win32::input_mutex::InputMutex;
use ai_cu::win32::overlay::{
    OVERLAY_CLASS, OverlayThread, OverlayWindow, overlay_shown_for_ms, overlay_windows,
};
use ai_cu::win32::uia::{
    FocusProbe, ReadProbe, UiaError, elements as uia_elements, focused_is_password, probe_elements,
    probe_focused_password, probe_tree,
};
use ai_cu::win32::{dpi, input, screen, window};

const WATCHDOG: Duration = Duration::from_secs(30);

struct Watchdog {
    done: Arc<AtomicBool>,
    scope: Arc<Mutex<Option<FixtureScope>>>,
}

/// 押したままかもしれない修飾キーとボタンを離すイベント (離す方向だけ。余計に離すのは無害)
fn release_all() -> Vec<InputEvent> {
    let mut ev: Vec<InputEvent> = [0x11u16, 0x10, 0x12]
        .iter()
        .map(|&vk| InputEvent::Key {
            vk,
            down: false,
            extended: false,
        })
        .collect();
    for button in [Button::Left, Button::Right, Button::Middle] {
        ev.push(InputEvent::Button {
            button,
            down: false,
        });
    }
    ev
}

impl Watchdog {
    fn start(name: &'static str) -> Watchdog {
        let done = Arc::new(AtomicBool::new(false));
        let scope: Arc<Mutex<Option<FixtureScope>>> = Arc::new(Mutex::new(None));
        let (flag, scope2) = (done.clone(), scope.clone());
        std::thread::spawn(move || {
            let start = Instant::now();
            while !flag.load(Ordering::SeqCst) {
                if start.elapsed() > WATCHDOG {
                    eprintln!(
                        "WATCHDOG: {name} が {WATCHDOG:?} を超えた。押したままを離してから終了する"
                    );
                    if let Some(s) = scope2.lock().ok().and_then(|g| *g) {
                        let _ = RealSink::for_fixture(s).send(&release_all());
                    }
                    std::process::exit(70);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        Watchdog { done, scope }
    }

    fn arm(&self, scope: FixtureScope) {
        if let Ok(mut g) = self.scope.lock() {
            *g = Some(scope);
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// フィクスチャの子の種類
#[derive(Clone, Copy)]
enum Kids {
    None,
    Controls,
    Password,
}

/// フィクスチャを開いている間、画面の縁の枠を出すか (D-094)。自分の枠を数えるテストは Hide
#[derive(Clone, Copy)]
enum Frame {
    Show,
    Hide,
}

/// フィクスチャの枠を見せる期限 (ウォッチドッグと同じ長さ。テストが固まっても枠は自分で消える。D-094)
const FRAME_SHOW_MS: u64 = WATCHDOG.as_millis() as u64;
const _: () = assert!(FRAME_SHOW_MS >= WATCHDOG.as_secs() * 1000);

/// プロセスで最初に枠を出したフィクスチャが、始まりの知らせを出したか (D-094)
static NOTICE_GIVEN: AtomicBool = AtomicBool::new(false);

/// `Show` なら枠のスレッドを立てて見せる。プロセスで最初に出せたときだけ、始まりの知らせを出して既定の予告の間
/// (`IndicatorConfig::default().lead_ms`) 待つ。音は鳴らさない。立てられなければ枠なしで続ける (知らせも省く。D-094)
fn start_frame(frame: Frame) -> Option<OverlayThread> {
    match frame {
        Frame::Hide => None,
        Frame::Show => match OverlayThread::start() {
            Err(e) => {
                eprintln!("FIXTURE 枠を出せない ({e})。枠なしで続ける");
                None
            }
            Ok(t) => {
                t.show(FRAME_SHOW_MS);
                if !NOTICE_GIVEN.swap(true, Ordering::SeqCst) {
                    let lead = IndicatorConfig::default().lead_ms;
                    println!(
                        "FIXTURE notice: 実入力のテストを始める。画面の縁の枠が出ている間はマウスとキーボードから手を離す (触れるとテストが落ちることがある。{lead} ms 後に始める)"
                    );
                    std::thread::sleep(Duration::from_millis(lead));
                }
                Some(t)
            }
        },
    }
}

/// フィクスチャ用の方針: このテストのプロセスと、フィクスチャのクライアント領域 `client` だけを許可する (D-020)。
/// `edit` で設定を足してから層にする (既定の `Fixture::policy` は何も足さない。D-090 のテストは password_input を足す)
fn fixture_policy(client: Rect, edit: impl FnOnce(&mut Config)) -> Policy {
    let exe = std::env::current_exe().unwrap();
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    let mut cfg = Config::default();
    cfg.general.input_enabled = true;
    cfg.general.capture_enabled = true;
    cfg.allow.processes = vec![exe_name];
    cfg.allow.regions = vec![client.to_string()];
    // 前面化 (activate) の注入したクリックも「最後の入力」に数えられるので、focus の無操作の判定は外す
    // (判定そのものは run のモックのテストで確かめる。D-050)
    cfg.capture.min_idle_ms = 0;
    edit(&mut cfg);
    Policy::new(vec![Layer::from_config(&cfg, "fixture").unwrap()])
}

struct Fixture {
    win: FixtureWindow,
    client: Rect,
    policy: Policy,
    /// 開いている間の画面の縁の枠 (D-094)。drop で枠のスレッドが窓を壊す
    _frame: Option<OverlayThread>,
    _watchdog: Watchdog,
}

impl Fixture {
    fn open(name: &'static str) -> Fixture {
        Fixture::open_at(name, 0)
    }

    /// `open` と同じ位置に、枠を出さずに開く (自分の枠を数える実機テスト用。D-094)
    fn open_without_frame(name: &'static str) -> Fixture {
        Fixture::open_inner(name, 0, Kids::None, Frame::Hide)
    }

    /// `dx` だけ右にずらして開く (2 つ目のフィクスチャを 1 つ目と重ねないため)
    fn open_at(name: &'static str, dx: i32) -> Fixture {
        Fixture::open_inner(name, dx, Kids::None, Frame::Show)
    }

    /// BUTTON と読み取り専用の EDIT を子に持つフィクスチャを開く (UIA の実機テスト用。D-088)。子の記録は取らない。
    /// D-089 のテストはフォーカスを移す手段として子をクリックする
    fn open_with_controls(name: &'static str) -> Fixture {
        Fixture::open_inner(name, 0, Kids::Controls, Frame::Show)
    }

    /// `ES_PASSWORD` の EDIT を 1 つ子に持つフィクスチャを開く (D-090)。子の記録は取らない。
    /// 親の空いた所の左クリックでフォーカスが親に戻る (`FixtureWindow::open_with_password`)
    fn open_with_password(name: &'static str) -> Fixture {
        Fixture::open_inner(name, 0, Kids::Password, Frame::Show)
    }

    /// 主モニタの左上の角に接して開く (枠の帯の上の点がフィクスチャに重なるように。D-093 の実機テスト)。
    /// 枠を出さない (自分の枠を数える実機テスト用。D-094)
    fn open_at_corner(name: &'static str) -> Fixture {
        Fixture::open_rect(
            name,
            |primary| Rect::new(primary.x, primary.y, 640, 400),
            Kids::None,
            Frame::Hide,
        )
    }

    fn open_inner(name: &'static str, dx: i32, kids: Kids, frame: Frame) -> Fixture {
        Fixture::open_rect(
            name,
            |primary| Rect::new(primary.w as i32 / 4 + dx, primary.h as i32 / 4, 640, 400),
            kids,
            frame,
        )
    }

    /// 主モニタの矩形から `place` で決めた窓の矩形に開く。`frame` が `Show` なら、開いている間ずっと画面の縁の枠を
    /// 出す (D-094)
    fn open_rect(
        name: &'static str,
        place: impl FnOnce(Rect) -> Rect,
        kids: Kids,
        frame: Frame,
    ) -> Fixture {
        let watchdog = Watchdog::start(name);
        dpi::ensure_per_monitor_v2().expect("Per-Monitor V2 を宣言できない");
        // 始まりの知らせの待ち (最初の 1 回だけ) もウォッチドッグの期限に数えられるが、収まる
        let frame = start_frame(frame);
        let primary = screen::primary_screen().expect("主モニタ");
        let rect = place(primary);
        let title = format!("ai-cu fixture {name} {}", std::process::id());
        let win = match kids {
            Kids::None => FixtureWindow::open(rect, &title),
            Kids::Controls => FixtureWindow::open_with_controls(rect, &title),
            Kids::Password => FixtureWindow::open_with_password(rect, &title),
        }
        .expect("フィクスチャを開けない");
        std::thread::sleep(Duration::from_millis(300));
        let client = win.client_rect().expect("クライアント領域");
        assert!(
            primary.contains_rect(&client),
            "フィクスチャが主モニタからはみ出す: {client}"
        );
        let policy = fixture_policy(client, |_| {});
        println!("fixture: hwnd={:#x} client={client}", win.hwnd());
        watchdog.arm(FixtureScope {
            hwnd: win.hwnd(),
            rect: client,
        });
        Fixture {
            win,
            client,
            policy,
            _frame: frame,
            _watchdog: watchdog,
        }
    }

    fn scope(&self) -> FixtureScope {
        FixtureScope {
            hwnd: self.win.hwnd(),
            rect: self.client,
        }
    }

    /// クライアント座標をスクリーン座標にする
    fn at(&self, x: i32, y: i32) -> Point {
        Point::new(self.client.x + x, self.client.y + y)
    }

    fn execute(&self, actions: &str) -> Result<RunReport, AppError> {
        self.execute_with(&self.policy, actions)
    }

    /// `execute` と同じ流れで、方針だけを `policy` に差し替えて実行する (D-090。`fixture_policy` で作った方針を渡す)
    fn execute_with(&self, policy: &Policy, actions: &str) -> Result<RunReport, AppError> {
        let script = Script::from_json(&format!("{{\"version\":1,\"actions\":[{actions}]}}"))
            .expect("操作列");
        let mut sink = RealSink::for_fixture(self.scope());
        let desktop = Win32Desktop::for_fixture(self.scope());
        let mut clock = SystemClock::new();
        let mut stops = StopSet::new();
        let mut images = PngFileWriter;
        Executor {
            policy,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut stops,
            images: &mut images,
            // 実行器には枠を渡さない。テストの間の枠はフィクスチャが持つ (D-094)。枠の窓そのものは overlay の実機テストが見る (D-093)
            indicator: &mut NoIndicator,
            // 実機の錠 (D-113) は錠の実機テストだけが使う
            input_lock: &mut NoInputLock,
        }
        .execute(&script)
    }

    /// `execute_with` と同じ装置で、実行器に枠 `indicator` を渡し、予告の間を入れて実行する (D-093 の予告の間を
    /// 実機で撃つテスト用。D-095)。呼ばれ方は CLI、残す時間は `linger_ms`
    fn execute_lead(
        &self,
        policy: &Policy,
        actions: &str,
        indicator: &mut dyn Indicator,
        linger_ms: u64,
    ) -> Result<RunReport, AppError> {
        self.execute_locked(policy, actions, indicator, &mut NoInputLock, linger_ms)
    }

    /// `execute_lead` と同じ流れで、実行器に入力の錠 `input_lock` も渡す (D-113 の錠を実機で撃つテスト用)
    fn execute_locked(
        &self,
        policy: &Policy,
        actions: &str,
        indicator: &mut dyn Indicator,
        input_lock: &mut dyn InputLock,
        linger_ms: u64,
    ) -> Result<RunReport, AppError> {
        let script = Script::from_json(&format!("{{\"version\":1,\"actions\":[{actions}]}}"))
            .expect("操作列");
        let mut sink = RealSink::for_fixture(self.scope());
        let desktop = Win32Desktop::for_fixture(self.scope());
        let mut clock = SystemClock::new();
        let mut stops = StopSet::new();
        let mut images = PngFileWriter;
        Executor {
            policy,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut stops,
            images: &mut images,
            indicator,
            input_lock,
        }
        .execute_on(
            &script,
            &ExecOptions {
                self_window: None,
                host_pids: &[],
                surface: Surface::Cli,
                linger_ms,
                skip_lead: false,
            },
        )
    }

    fn is_foreground(&self) -> bool {
        window::foreground_hwnd() == self.win.hwnd()
    }

    fn wait_foreground(&self, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if self.is_foreground() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.is_foreground()
    }

    /// 前面化 (D-020)。前面にできなければ、キーを送らずに失敗にする。
    fn activate(&self) {
        self.win.try_set_foreground();
        if !self.wait_foreground(Duration::from_millis(300)) {
            let c = self.client.center();
            let (x, y) = (c.x, c.y);
            self.execute(&format!(r#"{{"type":"click","x":{x},"y":{y}}}"#))
                .expect("フィクスチャの矩形の中のクリック");
        }
        assert!(
            self.wait_foreground(Duration::from_secs(2)),
            "NOT_FOREGROUND: 2 秒以内にフィクスチャを前面にできなかった。キーは 1 つも送っていない"
        );
        std::thread::sleep(Duration::from_millis(100));
        self.win.clear_events();
    }

    /// 条件を満たすまで (最大 2 秒) 記録を待つ
    fn events_until(&self, pred: impl Fn(&[FixtureEvent]) -> bool) -> Vec<FixtureEvent> {
        let start = Instant::now();
        loop {
            let ev = self.win.events();
            if pred(&ev) || start.elapsed() > Duration::from_secs(2) {
                return ev;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_click_double_click_and_right_click() {
    let f = Fixture::open("click");
    let p = f.at(100, 80);
    let q = f.at(400, 300);
    let r = f.at(500, 120);
    f.execute(&format!(
        r#"{{"type":"click","x":{},"y":{}}},{{"type":"wait","ms":700}},{{"type":"double_click","x":{},"y":{}}},{{"type":"wait","ms":700}},{{"type":"click","x":{},"y":{},"button":"right"}}"#,
        p.x, p.y, q.x, q.y, r.x, r.y
    ))
    .expect("クリックの操作列");
    let ev = f.events_until(|e| {
        e.iter().any(|x| {
            matches!(
                x,
                FixtureEvent::ButtonUp {
                    button: Button::Right,
                    ..
                }
            )
        })
    });
    println!("events: {ev:?}");
    assert!(
        ev.contains(&FixtureEvent::ButtonDown {
            button: Button::Left,
            x: 100,
            y: 80
        }),
        "{ev:?}"
    );
    assert!(
        ev.contains(&FixtureEvent::ButtonUp {
            button: Button::Left,
            x: 100,
            y: 80
        }),
        "{ev:?}"
    );
    assert!(
        ev.contains(&FixtureEvent::DoubleClick { x: 400, y: 300 }),
        "{ev:?}"
    );
    assert!(
        ev.contains(&FixtureEvent::ButtonUp {
            button: Button::Right,
            x: 500,
            y: 120
        }),
        "{ev:?}"
    );
    println!("FIXTURE click x=100 y=80 (left), dblclick x=400 y=300, right x=500 y=120");
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_drag_interpolates_between_endpoints() {
    let f = Fixture::open("drag");
    let (a, b) = (f.at(60, 60), f.at(420, 260));
    f.execute(&format!(
        r#"{{"type":"drag","from":{{"x":{},"y":{}}},"to":{{"x":{},"y":{}}},"steps":12,"duration_ms":240}}"#,
        a.x, a.y, b.x, b.y
    ))
    .expect("ドラッグの操作列");
    let ev = f.events_until(|e| e.iter().any(|x| matches!(x, FixtureEvent::ButtonUp { .. })));
    println!("events: {ev:?}");
    let moves = ev
        .iter()
        .filter(|e| matches!(e, FixtureEvent::DragMove { .. }))
        .count();
    assert_eq!(
        ev.first(),
        Some(&FixtureEvent::ButtonDown {
            button: Button::Left,
            x: 60,
            y: 60
        }),
        "{ev:?}"
    );
    assert!(
        ev.contains(&FixtureEvent::ButtonUp {
            button: Button::Left,
            x: 420,
            y: 260
        }),
        "{ev:?}"
    );
    assert!(moves >= 6, "補間の移動が届いていない: {moves}");
    assert!(
        ev.contains(&FixtureEvent::DragMove { x: 420, y: 260 }),
        "{ev:?}"
    );
    println!("FIXTURE drag from=(60,60) to=(420,260) moves={moves}");
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_move_cursor_readback_is_exact() {
    let f = Fixture::open("move");
    let points = [f.at(1, 1), f.at(321, 123), f.at(637, 397), f.at(10, 390)];
    for p in points {
        f.execute(&format!(r#"{{"type":"move","x":{},"y":{}}}"#, p.x, p.y))
            .expect("移動");
        std::thread::sleep(Duration::from_millis(30));
        let got = input::cursor_pos().expect("GetCursorPos");
        assert_eq!(got, p, "正規化した絶対座標が 1 ピクセルずれた");
    }
    println!("FIXTURE move readback ok ({} points)", points.len());
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_scroll_reaches_window_under_cursor() {
    let f = Fixture::open("scroll");
    let p = f.at(200, 200);
    f.execute(&format!(
        r#"{{"type":"scroll","x":{},"y":{},"clicks":-2}}"#,
        p.x, p.y
    ))
    .expect("スクロール");
    let wheels = |e: &[FixtureEvent]| -> Vec<i32> {
        e.iter()
            .filter_map(|x| match x {
                FixtureEvent::Wheel { delta } => Some(*delta),
                _ => None,
            })
            .collect()
    };
    let ev = f.events_until(|e| wheels(e).len() >= 2);
    // 1 ノッチずつ 2 回届く (D-063)
    assert_eq!(wheels(&ev), vec![-120, -120], "{ev:?}");
    let total: i32 = wheels(&ev).iter().sum();
    println!("FIXTURE scroll delta={total}");
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_text_unicode_reaches_foreground_fixture() {
    let f = Fixture::open("text");
    f.activate();
    let text = "aB1 日本😀\n";
    f.execute(&format!(
        r#"{{"type":"type","text":{}}}"#,
        serde_json::to_string(text).unwrap()
    ))
    .expect("文字入力");
    let expected: Vec<u16> = "aB1 日本😀\r".encode_utf16().collect();
    let ev = f.events_until(|e| {
        e.iter()
            .filter(|x| matches!(x, FixtureEvent::Char { .. }))
            .count()
            >= expected.len()
    });
    let units: Vec<u16> = ev
        .iter()
        .filter_map(|e| {
            if let FixtureEvent::Char { unit } = e {
                Some(*unit)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(units, expected, "{ev:?}");
    let shown = String::from_utf16_lossy(&units).replace('\r', "\\r");
    println!("FIXTURE text \"{shown}\" units={}", units.len());
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_key_combos_reach_foreground_fixture() {
    let f = Fixture::open("key");
    f.activate();
    f.execute(r#"{"type":"key","keys":"ctrl+a"},{"type":"key","keys":"f5"},{"type":"key","keys":"left"},{"type":"key","keys":"shift+tab"}"#)
        .expect("キーの操作列");
    let ev = f.events_until(|e| {
        e.iter()
            .filter(|x| matches!(x, FixtureEvent::KeyDown { .. }))
            .count()
            >= 6
    });
    let vks: Vec<u16> = ev
        .iter()
        .filter_map(|e| {
            if let FixtureEvent::KeyDown { vk } = e {
                Some(*vk)
            } else {
                None
            }
        })
        .collect();
    println!("keydown: {vks:02x?}");
    assert_eq!(vks, vec![0x11, 0x41, 0x74, 0x25, 0x10, 0x09], "{ev:?}");
    println!("FIXTURE key ctrl+a f5 left shift+tab vk={vks:02x?}");
}

/// IME のキー (D-131): 名前が vk に写り、`WM_KEYDOWN` としてフィクスチャに届く。フィクスチャは IME を外してあるので、
/// IME の反応そのものはここでは見ない (手動確認の手順 33)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_ime_keys_reach_foreground_fixture() {
    let f = Fixture::open("ime-keys");
    f.activate();
    f.execute(r#"{"type":"key","keys":"kanji"},{"type":"key","keys":"convert"},{"type":"key","keys":"nonconvert"},{"type":"key","keys":"imeon"},{"type":"key","keys":"imeoff"}"#)
        .expect("IME のキーの操作列");
    let ev = f.events_until(|e| {
        e.iter()
            .filter(|x| matches!(x, FixtureEvent::KeyDown { .. }))
            .count()
            >= 5
    });
    let vks: Vec<u16> = ev
        .iter()
        .filter_map(|e| {
            if let FixtureEvent::KeyDown { vk } = e {
                Some(*vk)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(vks, vec![0x19, 0x1C, 0x1D, 0x16, 0x1A], "{ev:?}");
    println!("FIXTURE ime_keys vk={vks:02x?}");
}

/// focus (D-050): 前面のプロセス (このテスト) が持つ、前面でないフィクスチャ B を `focus` で前面にしてからキーを送る。
/// A を前面にしてプロセスを前面プロセスにしておくので、B への `SetForegroundWindow` は OS の条件を満たす。
/// 実装置の `find_window` (フィクスチャ限定) と `set_foreground` の配線を通す。入力は B のクライアント領域だけ
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_focus_brings_second_fixture_to_front_before_keys() {
    let a = Fixture::open("focus-a");
    a.activate();
    let b = Fixture::open_at("focus-b", 700);
    // B を作ると B が前面になりうるので、A を前面に戻して「B は前面でない」状態を作る
    a.activate();
    assert!(
        a.is_foreground() && !b.is_foreground(),
        "A が前面で B は前面でない"
    );
    b.win.clear_events();
    let exe = std::env::current_exe().unwrap();
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    // 題名の一部で B を選ぶ (A も同じプロセス)。フィクスチャ限定の装置は B 以外を候補にしない
    let r = b
        .execute(&format!(
            r#"{{"type":"focus","process":"{exe_name}","title":"fixture focus-b"}},{{"type":"key","keys":"a"}}"#
        ))
        .expect("focus とキーの操作列");
    assert_eq!(r.executed, 2);
    assert_eq!(r.focused.len(), 1, "{:?}", r.focused);
    assert_eq!(r.focused[0].hwnd, b.win.hwnd());
    assert!(b.is_foreground(), "focus の後は B が前面");
    let ev = b.events_until(|e| e.iter().any(|x| matches!(x, FixtureEvent::KeyDown { .. })));
    assert!(
        ev.iter()
            .any(|e| matches!(e, FixtureEvent::KeyDown { vk: 0x41 })),
        "キーは B に届く: {ev:?}"
    );
    assert!(
        !a.win
            .events()
            .iter()
            .any(|e| matches!(e, FixtureEvent::KeyDown { .. })),
        "A には届かない"
    );
    // 既に前面なら何もしない。A (題名が違う) は B 限定の装置では見つからず 11
    let r = b.execute(&format!(
        r#"{{"type":"focus","process":"{exe_name}","title":"fixture focus-b"}}"#
    ));
    assert_eq!(r.expect("既に前面").executed, 1);
    let e = b
        .execute(&format!(
            r#"{{"type":"focus","process":"{exe_name}","title":"fixture focus-a"}}"#
        ))
        .unwrap_err();
    assert_eq!(e.kind.exit_code(), 11, "{e}");
    println!(
        "FIXTURE focus: B foreground={} after focus, key reached B",
        b.is_foreground()
    );
}

/// 実装置の `post_close` と `window_presence` (D-062、D-096) の配線を通す。閉じるのはフィクスチャ B だけで、入力は送らず、
/// 前面 (A) も変えない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_close_posts_wm_close_to_background_fixture_only() {
    let a = Fixture::open("close-a");
    a.activate();
    let b = Fixture::open_at("close-b", 700);
    a.activate();
    assert!(
        a.is_foreground() && !b.is_foreground(),
        "A が前面で B は前面でない"
    );
    b.win.clear_events();
    let exe = std::env::current_exe().unwrap();
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    // B 限定の装置では、A (題名が違う) は見つからず 12。何も置かない
    let e = b
        .execute(&format!(
            r#"{{"type":"close","process":"{exe_name}","title":"fixture close-a"}}"#
        ))
        .unwrap_err();
    assert_eq!(e.kind.exit_code(), 12, "{e}");
    assert!(window::is_top_level(a.win.hwnd()), "A は残る");
    let r = b
        .execute(&format!(
            r#"{{"type":"close","process":"{exe_name}","title":"fixture close-b"}}"#
        ))
        .expect("close");
    assert_eq!(r.executed, 1);
    assert_eq!(r.closed.len(), 1, "{:?}", r.closed);
    assert_eq!(r.closed[0].hwnd, b.win.hwnd());
    assert!(r.closed[0].gone, "B は WM_CLOSE で消える: {:?}", r.closed);
    assert!(!window::is_top_level(b.win.hwnd()), "B のハンドルは無効");
    assert!(a.is_foreground(), "前面は A のまま");
    assert!(
        b.win.events().is_empty(),
        "入力は送らない: {:?}",
        b.win.events()
    );
    assert!(!r.closed[0].hidden, "B は壊れたので隠れただけではない");
    println!("FIXTURE close: B gone, A still foreground");
}

/// `window::presence_of` の 3 状態を実機の窓で見る (D-096)。見える窓は `Visible`、隠した窓は `Hidden`、隠れたまま
/// 違う pid では `Destroyed` (pid を可視より先に見る)、壊した後は `Destroyed`。入力は送らない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_window_presence_reports_visible_hidden_destroyed() {
    let f = Fixture::open("presence");
    let hwnd = f.win.hwnd();
    let pid = std::process::id();
    assert_eq!(window::presence_of(hwnd, pid), WindowPresence::Visible);
    f.win.hide();
    assert!(!f.win.is_visible(), "SW_HIDE で隠れる");
    assert_eq!(window::presence_of(hwnd, pid), WindowPresence::Hidden);
    assert_eq!(
        window::presence_of(hwnd, pid.wrapping_add(1)),
        WindowPresence::Destroyed,
        "隠れた窓でも pid が違えば再利用とみなす"
    );
    drop(f);
    assert_eq!(window::presence_of(hwnd, pid), WindowPresence::Destroyed);
    println!("FIXTURE presence: visible -> hidden -> destroyed");
}

/// WM_CLOSE で壊れず隠れるだけのフィクスチャ (トレイに入るアプリを模す) を `close` すると、`gone: true` と
/// `hidden: true` が返り、ハンドルは有効なまま見えなくなる (D-096)。閉じるのは B だけで、入力は送らず、
/// 前面 (A) も変えない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_close_reports_hidden_for_hide_on_close_fixture() {
    let a = Fixture::open("close-hide-a");
    a.activate();
    let b = Fixture::open_at("close-hide-b", 700);
    b.win.set_hide_on_close(true);
    a.activate();
    assert!(
        a.is_foreground() && !b.is_foreground(),
        "A が前面で B は前面でない"
    );
    b.win.clear_events();
    let exe = std::env::current_exe().unwrap();
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    let start = Instant::now();
    let r = b
        .execute(&format!(
            r#"{{"type":"close","process":"{exe_name}","title":"fixture close-hide-b"}}"#
        ))
        .expect("close");
    let elapsed_ms = start.elapsed().as_millis();
    assert_eq!(r.executed, 1);
    assert_eq!(r.closed.len(), 1, "{:?}", r.closed);
    assert_eq!(r.closed[0].hwnd, b.win.hwnd());
    assert!(r.closed[0].gone, "B は見えなくなる: {:?}", r.closed);
    assert!(r.closed[0].hidden, "B は隠れただけ: {:?}", r.closed);
    assert!(
        window::is_top_level(b.win.hwnd()),
        "B のハンドルは有効なまま"
    );
    assert!(!b.win.is_visible(), "B は見えない");
    assert!(a.is_foreground(), "前面は A のまま");
    assert!(
        b.win.events().is_empty(),
        "入力は送らない: {:?}",
        b.win.events()
    );
    println!("FIXTURE close hidden elapsed_ms={elapsed_ms}");
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_screenshot_of_fixture_region_only() {
    let f = Fixture::open("shot");
    // 塗りの確認なので、カーソルの影響を避けてフィクスチャの外寄りに置かない (BitBlt はカーソルを写さない)
    let inner = Rect::new(
        f.client.x + 10,
        f.client.y + 10,
        f.client.w - 20,
        f.client.h - 20,
    );
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
        .join(format!("fixture-shot-{}.png", std::process::id()));
    let report = f
        .execute(&format!(
            r#"{{"type":"screenshot","path":{},"region":{{"x":{},"y":{},"w":{},"h":{}}}}}"#,
            serde_json::to_string(&out.display().to_string()).unwrap(),
            inner.x,
            inner.y,
            inner.w,
            inner.h
        ))
        .expect("撮影");
    assert_eq!(report.screenshots[0].width, inner.w);
    let img = decode_png(&std::fs::read(&out).unwrap()).unwrap();
    let (r, g, b) = FIXTURE_RGB;
    let matching = img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] == r && p[1] == g && p[2] == b)
        .count();
    let total = (img.width * img.height) as usize;
    assert!(
        matching * 100 >= total * 99,
        "フィクスチャの色の画素が少ない: {matching}/{total}"
    );
    println!(
        "FIXTURE shot {}x{} match={matching}/{total}",
        img.width, img.height
    );
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_window_list_reports_fixture_rect() {
    // 読み取りだけ (列挙と問い合わせ)。入力も撮影もしない
    let f = Fixture::open("windows");
    let list = window::top_level_windows_detailed();
    // フィクスチャはツールウィンドウなので、一覧 (利用者が選ぶ対象) には出ない
    assert!(list.iter().all(|w| w.info.hwnd != f.win.hwnd()));
    assert!(
        list.iter().all(|w| w.minimized || w.rect.is_some()),
        "最小化していないウィンドウは矩形を持つ"
    );
    let me = window::describe(f.win.hwnd()).expect("フィクスチャの詳細");
    let expected = f.win.window_rect().unwrap();
    assert_eq!(
        me.rect,
        Some(expected),
        "DWM の枠の矩形が物理ピクセルで一致する"
    );
    assert!(!me.minimized);
    // 枠の無いポップアップには題名の帯が無い
    assert!(!window::is_caption(f.win.hwnd(), f.client.center()));
    println!(
        "FIXTURE windows listed {} windows, fixture rect={expected}",
        list.len()
    );
}

/// 自プロセスの汲まないスレッドの窓 (D-105): フィクスチャの窓のスレッドを `stall` で 2 秒眠らせた間も、`describe`、
/// `top_level_windows_detailed`、`top_level_windows` がそれぞれ 1 秒未満で戻り、`describe` がフィクスチャの題名を
/// 読めることを確かめる (題名の読みが WM_GETTEXT / WM_GETTEXTLENGTH を送ると、眠りの終わりまで待たされる)。
/// 読みごとに `stall` をかけ直す (1 回の眠りでまとめて測ると、対照で最初の読みが眠りを吸い、後の読みを撃てない)。
/// 読み取りだけで、入力も撮影もしない (`stall` は自分の窓に自分のメッセージを置くだけ)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_window_reads_do_not_wait_on_stalled_own_window() {
    let f = Fixture::open("stall");
    let hwnd = f.win.hwnd();
    let expected_title = format!("ai-cu fixture stall {}", std::process::id());
    let sleep = Duration::from_secs(2);
    let limit = Duration::from_secs(1);
    // 眠らせてから `read` だけを測り、眠りが明けるのを待ってから経過を返す
    let measure = |name: &str, read: &mut dyn FnMut()| -> Duration {
        assert!(f.win.stall(sleep), "{name} の前に眠りの印が立たない");
        let start = Instant::now();
        read();
        let elapsed = start.elapsed();
        let woke = Instant::now();
        while f.win.is_stalled() && woke.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!f.win.is_stalled(), "{name} の後に眠りが明けない");
        assert!(
            elapsed < limit,
            "{name} が眠っている窓に待たされた: {} ms",
            elapsed.as_millis()
        );
        elapsed
    };
    let mut described = None;
    let describe_d = measure("describe", &mut || described = window::describe(hwnd));
    let mut detailed = 0;
    let detailed_d = measure("top_level_windows_detailed", &mut || {
        detailed = window::top_level_windows_detailed().len()
    });
    let mut listed = 0;
    let listed_d = measure("top_level_windows", &mut || {
        listed = window::top_level_windows().len()
    });
    // 撮影の判定の列挙 (D-077)。リージョンの読み (`GetWindowRgnBox`、D-112) を足したので、ここでも撃つ
    let mut stacked = 0;
    let stacked_d = measure("stack_windows", &mut || {
        stacked = window::stack_windows().len()
    });
    let me = described.expect("フィクスチャの詳細");
    assert_eq!(
        me.info.title, expected_title,
        "自プロセスの窓の題名を読める"
    );
    println!(
        "FIXTURE stalled reads describe_ms={} detailed_ms={} listed_ms={} stack_ms={} title=ok (detailed={detailed} listed={listed} stack={stacked})",
        describe_d.as_millis(),
        detailed_d.as_millis(),
        listed_d.as_millis(),
        stacked_d.as_millis()
    );
}

/// 固まった窓のパスワード欄の判定の測り (D-108) で、読みごとにかける眠りの長さ (本番の上限 4 秒と縮めた待ち 1 秒より長い)
const FOCUS_STALL: Duration = Duration::from_secs(6);
/// 読みの後に眠りが明けるのを待つ上限 (D-108)
const FOCUS_WAKE_LIMIT: Duration = Duration::from_secs(8);

/// HRESULT を `0x80131505` の形で書く
fn hr_text(code: i32) -> String {
    format!("{:#010X}", code as u32)
}

/// 判定の戻りを `FIXTURE` の行の形にする。`TimedOut` は文に「UIA の時間切れ」があれば `uia`、なければ `limit`
fn focus_result_text(r: &Result<Option<bool>, UiaError>) -> String {
    match r {
        Ok(v) => format!("Ok({v:?})"),
        Err(UiaError::TimedOut(m)) if m.contains("UIA の時間切れ") => {
            "Err(TimedOut:uia)".into()
        }
        Err(UiaError::TimedOut(_)) => "Err(TimedOut:limit)".into(),
        Err(UiaError::Failed(_)) => "Err(Failed)".into(),
    }
}

/// 計測の入口の戻りを `FIXTURE focus_probe` の行の中身にする
fn focus_probe_text(p: &FocusProbe) -> String {
    let focused = match p.raw.focused.code {
        Some(c) => hr_text(c),
        None => "ok".into(),
    };
    let is_password = match &p.raw.is_password {
        Some(pr) => {
            let v = match pr.result {
                Ok(b) => b.to_string(),
                Err(c) => hr_text(c),
            };
            format!("{v}/{}", pr.ms)
        }
        None => "none".into(),
    };
    format!(
        "short_timeouts={} focused={focused}/{} is_password={is_password} classified={}",
        p.raw.short_timeouts,
        p.raw.focused.ms,
        focus_result_text(&p.classified)
    )
}

/// 場の対照 (D-108): 眠らせない状態で計測の入口を読み、前面のフィクスチャの要素が取れて IsPassword が偽で、
/// 振り分けが `Ok(Some(false))` であることを確かめる (場ができていなければ測らずに落ちる)
fn assert_focus_probe_baseline(tag: &str) {
    let p = probe_focused_password(|| {}).expect("眠らせない計測の入口");
    println!("FIXTURE focus_probe {tag}_base {}", focus_probe_text(&p));
    assert_eq!(
        p.raw.focused.code, None,
        "眠らせない GetFocusedElement が要素を返さない: {p:?}"
    );
    let pw = p.raw.is_password.as_ref().expect("IsPassword の読み");
    assert_eq!(pw.result, Ok(false), "眠らせない IsPassword: {p:?}");
    assert_eq!(p.classified, Ok(Some(false)), "眠らせない振り分け: {p:?}");
}

/// 振り分けが UIA 自身の時間切れ (`TimedOut` で、文に「UIA の時間切れ」。上限の文でない) か (D-108 のステップ 2)。
/// 時間切れかは本番と同じ振り分け (`classified`) だけで読み、HRESULT の一覧は書かない。文の判定は `focus_result_text` の 1 か所
fn assert_uia_timed_out(tag: &str, p: &FocusProbe) {
    assert_eq!(
        focus_result_text(&p.classified),
        "Err(TimedOut:uia)",
        "{tag}: 振り分けが UIA の時間切れでない: {p:?}"
    );
}

/// 読みの後、眠りが明けるまで最大 `FOCUS_WAKE_LIMIT` 待つ (閉じる前に明けさせる。明けたかは行に出すだけ)
fn wait_focus_stall_end(f: &Fixture) -> bool {
    wait_until(FOCUS_WAKE_LIMIT, || !f.win.is_stalled())
}

/// 固まった窓のパスワード欄の判定 (D-108 の (a)): 前面のフィクスチャのスレッドを 6 秒眠らせてから計測の入口を読み、
/// 2 つの UIA の呼び出しの HRESULT と経過を `FIXTURE focus_probe a` の行に出す。断言は場の対照、probe が Ok で
/// 返ったこと、読みの始めと終わりの両方で眠っていたこと、待たされた呼び出しの経過が本番の上限より短いこと、振り分けが
/// UIA の時間切れであること (ステップ 2 で測った結果から足した)、閉じた後の `stall` が偽 (閉じた後は置かない結果を見るだけ)。
/// 入力は `activate` が前面にできなかったときのフィクスチャ内のクリックだけ
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_focus_probe_stalled_before_get_focused() {
    let f = Fixture::open("uia-focus-a");
    f.activate();
    assert_focus_probe_baseline("a");
    let stalled = f.win.stall(FOCUS_STALL);
    let stalled_before = f.win.is_stalled();
    let probe = probe_focused_password(|| {});
    let stalled_after = f.win.is_stalled();
    let woke = wait_focus_stall_end(&f);
    let probe = probe.expect("眠らせた後の計測の入口が返らない");
    println!(
        "FIXTURE focus_probe a {} stalled_before={stalled_before} stalled_after={stalled_after} stall_set={stalled} woke={woke}",
        focus_probe_text(&probe)
    );
    assert!(
        stalled_before && stalled_after,
        "読みの始めと終わりの両方で眠っていない (空振り): before={stalled_before} after={stalled_after}"
    );
    match probe.raw.focused.code {
        Some(_) => assert!(
            probe.raw.focused.ms < probe.focus_limit_ms,
            "GetFocusedElement の時間切れが本番の上限より短く返らない: {probe:?}"
        ),
        None => {
            let pw = probe
                .raw
                .is_password
                .as_ref()
                .expect("GetFocusedElement が待たずに返ったのに IsPassword の読みが無い");
            assert!(pw.result.is_err(), "IsPassword が Err でない: {probe:?}");
            assert!(
                pw.ms >= probe.short_timeout_ms && pw.ms < probe.focus_limit_ms,
                "IsPassword の経過が縮めた待ち以上・本番の上限未満でない: {probe:?}"
            );
        }
    }
    assert_uia_timed_out("a", &probe);
    let s = f.win.staller();
    drop(f);
    assert!(
        !s.stall(Duration::from_millis(100)),
        "閉じた後のフィクスチャに眠りを置いた"
    );
}

/// 固まった窓のパスワード欄の判定 (D-108 の (a')): 前面のフィクスチャのスレッドを 6 秒眠らせてから本番の判定
/// (`focused_is_password`) を読み、結果と経過を `FIXTURE focus_probe a_prod=` の行に出す。断言は場の対照 (眠らせない
/// 本番の判定が `Ok(Some(false))`)、読みの始めと終わりの両方で眠っていたこと、本番の判定が `Err(TimedOut)` で断る
/// こと (fail-closed。ステップ 2 で足した)。入力は `activate` の前面化のクリックだけ
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_focus_check_on_stalled_fixture() {
    let f = Fixture::open("uia-focus-a-prod");
    f.activate();
    let base = focused_is_password();
    println!(
        "FIXTURE focus_probe a_prod_base={}",
        focus_result_text(&base)
    );
    assert_eq!(base, Ok(Some(false)), "眠らせない本番の判定");
    let stalled = f.win.stall(FOCUS_STALL);
    let stalled_before = f.win.is_stalled();
    let start = Instant::now();
    let r = focused_is_password();
    let ms = start.elapsed().as_millis();
    let stalled_after = f.win.is_stalled();
    let woke = wait_focus_stall_end(&f);
    println!(
        "FIXTURE focus_probe a_prod={} ms={ms} stalled_before={stalled_before} stalled_after={stalled_after} stall_set={stalled} woke={woke}",
        focus_result_text(&r)
    );
    assert!(
        stalled_before && stalled_after,
        "読みの始めと終わりの両方で眠っていない (空振り): before={stalled_before} after={stalled_after}"
    );
    assert!(
        matches!(r, Err(UiaError::TimedOut(_))),
        "固まった窓で本番の判定が断らない (fail-closed でない): {r:?}"
    );
}

/// 固まった窓のパスワード欄の判定 (D-108 の (b)): `GetFocusedElement` が返った後、`CurrentIsPassword` の前に
/// フィクスチャのスレッドを 6 秒眠らせ、HRESULT と経過を `FIXTURE focus_probe b` の行に出す。断言は場の対照、probe が
/// Ok で返ったこと、間で眠りを置けたこと、眠らせる前の `GetFocusedElement` が要素を返して `CurrentIsPassword` を
/// 読んだこと (読んでいなければ空振りとして失敗)、`CurrentIsPassword` が縮めた待ち以上待たされたなら振り分けが UIA の時間切れ
/// であること (ステップ 2 で足した。待たされなければその旨を行に出すだけ)。入力は `activate` の前面化のクリックだけ
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_focus_probe_stalled_between_calls() {
    let f = Fixture::open("uia-focus-b");
    f.activate();
    assert_focus_probe_baseline("b");
    let s = f.win.staller();
    let between_stalled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&between_stalled);
    let stalled_before = f.win.is_stalled();
    let probe = probe_focused_password(move || flag.store(s.stall(FOCUS_STALL), Ordering::SeqCst));
    let stalled_after = f.win.is_stalled();
    let woke = wait_focus_stall_end(&f);
    let probe = probe.expect("間で眠らせた計測の入口が返らない");
    println!(
        "FIXTURE focus_probe b {} stalled_before={stalled_before} stalled_after={stalled_after} between_stalled={} woke={woke}",
        focus_probe_text(&probe),
        between_stalled.load(Ordering::SeqCst)
    );
    assert!(
        between_stalled.load(Ordering::SeqCst),
        "2 つの呼び出しの間で眠りを置けなかった (空振り)"
    );
    assert_eq!(
        probe.raw.focused.code, None,
        "間で眠らせる前の GetFocusedElement が要素を返さない (場が崩れた): {probe:?}"
    );
    let pw = probe
        .raw
        .is_password
        .as_ref()
        .expect("GetFocusedElement の後に IsPassword を読んでいない (空振り)");
    if pw.ms >= probe.short_timeout_ms {
        assert_uia_timed_out("b", &probe);
    } else {
        println!("FIXTURE focus_probe b waited=false (CurrentIsPassword は待たされなかった)");
    }
}

/// 固まった窓の要素と木の読みの測り (D-109) で、読みごとにかける眠りの長さ (縮めた待ち 1 秒の数回分より長く、
/// フィクスチャのウォッチドッグ 30 秒に 1 回だけ収まる)
const READ_STALL: Duration = Duration::from_secs(12);
/// 要素と木の読みの後に眠りが明けるのを待つ上限 (D-109)
const READ_WAKE_LIMIT: Duration = Duration::from_secs(14);

/// 要素の読みの測り (D-109) の問い合わせ。絞り込みなし、上限 20 (子つきのフィクスチャで 3 要素・`skipped` 0・打ち切りなし。
/// 子ごとに describe・フォーカス・IsPassword・状態のパターンまで読む、いちばん呼び出しの多い経路)
fn read_probe_query() -> ElementQuery {
    ElementQuery {
        limit: 20,
        ..Default::default()
    }
}

/// 木の読みの測り (D-109) の深さと上限
const READ_PROBE_TREE_DEPTH: usize = 3;
const READ_PROBE_TREE_NODES: usize = 20;

/// `describe` が出す欄すべて (名前、種類、automation_id、クラス、矩形、有効、画面外)。種類以外の失敗は空文字・None・
/// false に握りつぶされるので、全部を比べる (D-109)
type DescribedFields = (String, String, String, String, Option<Rect>, bool, bool);

fn described_fields(e: &UiElement) -> DescribedFields {
    (
        e.name.clone(),
        e.control_type.clone(),
        e.automation_id.clone(),
        e.class.clone(),
        e.rect,
        e.enabled,
        e.offscreen,
    )
}

/// find の経路でだけ読むフラグと状態 (focused、password、toggle、selected、expanded。D-109)
type FlagFields = (bool, bool, Option<String>, Option<bool>, Option<String>);

fn flag_fields(e: &UiElement) -> FlagFields {
    (
        e.focused,
        e.password,
        e.toggle.clone(),
        e.selected,
        e.expanded.clone(),
    )
}

/// 2 つの要素の並びで、`describe` の欄すべての並びが同じか (`same`)
fn same_described<'a>(
    a: impl Iterator<Item = &'a UiElement>,
    b: impl Iterator<Item = &'a UiElement>,
) -> bool {
    a.map(described_fields).eq(b.map(described_fields))
}

/// 文の頭 60 文字 (行に出す。改行は空白にする)
fn head60(s: &str) -> String {
    s.chars()
        .take(60)
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// 要素の読みの結果を、場の対照 `base` と比べて行の `result=` の中身にする。名前そのものは出さない
fn elements_result_text(r: &Result<Elements, ReadError>, base: &Elements) -> String {
    match r {
        Ok(e) => format!(
            "Ok n={} skipped={} truncated={} same={} same_flags={}",
            e.elements.len(),
            e.skipped,
            e.truncated,
            same_described(e.elements.iter(), base.elements.iter()),
            e.elements
                .iter()
                .map(flag_fields)
                .eq(base.elements.iter().map(flag_fields))
        ),
        Err(m) => format!("Err {}", head60(&m.to_string())),
    }
}

/// 木の読みの結果を、場の対照 `base` と比べて行の `result=` の中身にする (`n` は `nodes.len()`、`truncated` は
/// `node_cap_hit`。木はフラグを持たないので `same_flags` は出さない)
fn tree_result_text(r: &Result<UiTree, ReadError>, base: &UiTree) -> String {
    match r {
        Ok(t) => format!(
            "Ok n={} skipped={} truncated={} depth_cut={} same={}",
            t.nodes.len(),
            t.skipped,
            t.node_cap_hit,
            t.depth_cut,
            same_described(
                t.nodes.iter().map(|n| &n.element),
                base.nodes.iter().map(|n| &n.element)
            )
        ),
        Err(m) => format!("Err {}", head60(&m.to_string())),
    }
}

/// `UiaError` の種類だけ (`Probe(<TimedOut|Failed>)` の中身)
fn uia_error_kind(e: &UiaError) -> &'static str {
    match e {
        UiaError::TimedOut(_) => "TimedOut",
        UiaError::Failed(_) => "Failed",
    }
}

/// 計測の入口の戻りを行の `short_timeouts=<b> ms=<n> result=<...>` にする。probe 自体が Err なら
/// `short_timeouts=- ms=<呼び手の側で測った経過> result=Probe(<種類>)`
fn read_probe_text<T>(
    p: &Result<ReadProbe<T>, UiaError>,
    caller_ms: u128,
    result: impl FnOnce(&Result<T, ReadError>) -> String,
) -> String {
    match p {
        Ok(p) => format!(
            "short_timeouts={} ms={} result={}",
            p.short_timeouts,
            p.ms,
            result(&p.result)
        ),
        Err(e) => format!(
            "short_timeouts=- ms={caller_ms} result=Probe({})",
            uia_error_kind(e)
        ),
    }
}

/// 固まった窓の読みが最初の UIA の時間切れで打ち切ったと見なす、閉包で眠りを置いてから probe が返るまでの上限
/// (ミリ秒。D-110)。縮めた待ち (測って約 1000 ms) の 1 回分と、2 回目の待ちが入る (約 2000 ms) ことまでを許し、
/// 待ちの重なり (D-109 の測りで要素 11991 ms、木 8591 ms) を確実に落とす境
const ABORT_LIMIT_MS: u64 = 4000;

/// 場の対照 (D-109): 眠らせない同じ要素の読みが 3 要素 (`skipped` 0、打ち切りなし) を返すことを確かめ、
/// 結果と経過 (`base_ms`) を返す (場ができていなければ測らずに落ちる)
fn read_probe_elements_baseline(tag: &str, hwnd: usize) -> (Elements, u64) {
    let p = probe_elements(hwnd, &read_probe_query(), true, || {}).expect("眠らせない要素の計測");
    let base = p.result.clone().expect("眠らせない要素の読み");
    println!(
        "FIXTURE read_probe {tag}_base short_timeouts={} ms={} result=Ok n={} skipped={} truncated={}",
        p.short_timeouts,
        p.ms,
        base.elements.len(),
        base.skipped,
        base.truncated
    );
    assert_eq!(base.elements.len(), 3, "眠らせない要素の読み: {base:?}");
    assert_eq!(base.skipped, 0, "眠らせない要素の読み: {base:?}");
    assert!(!base.truncated, "眠らせない要素の読み: {base:?}");
    (base, p.ms)
}

/// 閉包の中で眠りを置き、置けたかと `stall` が返った時刻を残す (D-109 の (b)(d))。返るのは閉包と、2 つの共有の値
#[allow(clippy::type_complexity)]
fn stall_in_closure(
    f: &Fixture,
) -> (
    impl FnOnce() + Send + 'static,
    Arc<AtomicBool>,
    Arc<Mutex<Option<Instant>>>,
) {
    let s = f.win.staller();
    let stall_set = Arc::new(AtomicBool::new(false));
    let stalled_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let (flag, at) = (Arc::clone(&stall_set), Arc::clone(&stalled_at));
    let closure = move || {
        let ok = s.stall(READ_STALL);
        flag.store(ok, Ordering::SeqCst);
        if let Ok(mut g) = at.lock() {
            *g = Some(Instant::now());
        }
    };
    (closure, stall_set, stalled_at)
}

/// 閉包で眠りを置いた時刻から `returned` までのミリ秒 (置いていなければ None。D-110)
fn after_ms(stalled_at: &Mutex<Option<Instant>>, returned: Instant) -> Option<u64> {
    stalled_at.lock().ok().and_then(|g| *g).map(|t| {
        u64::try_from(returned.saturating_duration_since(t).as_millis()).unwrap_or(u64::MAX)
    })
}

/// 閉包で眠りを置いた時刻から `returned` までのミリ秒 (置いていなければ `-`)
fn after_ms_text(stalled_at: &Mutex<Option<Instant>>, returned: Instant) -> String {
    match after_ms(stalled_at, returned) {
        Some(ms) => ms.to_string(),
        None => "-".into(),
    }
}

/// 根を読んだ後に眠らせた読みの断言 (D-110 の (b)(d)): probe が Ok (上限の `TimedOut` で返ったら失敗)、`result` が
/// `Err(ReadError::TimedOut(m))` で m が `context` (根より下の読みで打ち切った印) を含み、眠りを置いてから返るまでが
/// `ABORT_LIMIT_MS` 未満
fn assert_aborted_below_root<T: std::fmt::Debug>(
    tag: &str,
    probe: &Result<ReadProbe<T>, UiaError>,
    context: &str,
    after: Option<u64>,
) {
    let p = probe
        .as_ref()
        .unwrap_or_else(|e| panic!("{tag}: 計測の入口が返らなかった (上限か失敗): {e:?}"));
    match &p.result {
        Err(ReadError::TimedOut(m)) => assert!(
            m.contains(context),
            "{tag}: 時間切れの文に「{context}」が無い (根より下で打ち切っていない。測りが無効): {m}"
        ),
        other => panic!("{tag}: 読みが時間切れで打ち切られていない: {other:?}"),
    }
    let after = after.unwrap_or_else(|| panic!("{tag}: 眠りを置いた時刻が無い"));
    assert!(
        after < ABORT_LIMIT_MS,
        "{tag}: 眠りを置いてから {after} ms で返った (上限 {ABORT_LIMIT_MS} ms。待ちが重なった)"
    );
}

/// 固まった窓の要素の読み (D-109 の (a)): 子つきのフィクスチャのスレッドを 12 秒眠らせてから、縮めた待ちの計測の入口で
/// 要素を読み、どこで何秒で終わったかを `FIXTURE read_probe a` の行に出す (重なりではなく打ち切りの位置を測る)。
/// 断言は場の対照、眠りを置けたこと、読みの始めで眠っていたこと、probe が Ok で返ったこと、`result` が
/// `Err(ReadError::TimedOut)` であること (根の読み (`ElementFromHandle`) の UIA の時間切れ。D-110)。前面化も入力もしない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_read_probe_stalled_before_find() {
    let f = Fixture::open_with_controls("read-probe-a");
    let hwnd = f.win.hwnd();
    let (base, base_ms) = read_probe_elements_baseline("a", hwnd);
    let stall_set = f.win.stall(READ_STALL);
    let stalled_before = f.win.is_stalled();
    let start = Instant::now();
    let probe = probe_elements(hwnd, &read_probe_query(), true, || {});
    let caller_ms = start.elapsed().as_millis();
    let stalled_after = f.win.is_stalled();
    let woke = wait_until(READ_WAKE_LIMIT, || !f.win.is_stalled());
    println!(
        "FIXTURE read_probe a {} stalled_before={stalled_before} stalled_after={stalled_after} stall_set={stall_set} woke={woke} base_ms={base_ms}",
        read_probe_text(&probe, caller_ms, |r| elements_result_text(r, &base))
    );
    assert!(stall_set, "眠りを置けなかった (空振り)");
    assert!(
        stalled_before,
        "読みの始めで眠っていない (空振り): before={stalled_before}"
    );
    let p = probe
        .as_ref()
        .unwrap_or_else(|e| panic!("a: 計測の入口が返らなかった (上限か失敗): {e:?}"));
    assert!(
        matches!(p.result, Err(ReadError::TimedOut(_))),
        "a: 読みの前に眠らせたのに時間切れでない: {:?}",
        p.result
    );
}

/// 固まった窓の要素の読み (D-109 の (b)): 縮めた待ちの計測の入口で要素を読み、根を読んだ後・子の繰り返しの前に
/// フィクスチャのスレッドを 12 秒眠らせて、子の読みで最初の UIA の時間切れで打ち切るかを `FIXTURE read_probe b` の行
/// (`after_ms` は眠りを置いた後から probe が返るまで) に出す。断言は場の対照、閉包で眠りを置けたこと、probe が Ok で
/// 返ったこと、`result` が `Err(ReadError::TimedOut(m))` で m が「子の要素の読み: 」を含むこと (眠りが根を読んだ後に
/// 入った確かめ)、`after_ms` が `ABORT_LIMIT_MS` 未満 (待ちが重ならない) こと (D-110)。前面化も入力もしない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_read_probe_stalled_after_root() {
    let f = Fixture::open_with_controls("read-probe-b");
    let hwnd = f.win.hwnd();
    let (base, base_ms) = read_probe_elements_baseline("b", hwnd);
    let (between, stall_set, stalled_at) = stall_in_closure(&f);
    let stalled_before = f.win.is_stalled();
    let start = Instant::now();
    let probe = probe_elements(hwnd, &read_probe_query(), true, between);
    let returned = Instant::now();
    let caller_ms = returned.duration_since(start).as_millis();
    let stalled_after = f.win.is_stalled();
    let woke = wait_until(READ_WAKE_LIMIT, || !f.win.is_stalled());
    let stall_set = stall_set.load(Ordering::SeqCst);
    println!(
        "FIXTURE read_probe b {} stalled_before={stalled_before} stalled_after={stalled_after} stall_set={stall_set} woke={woke} after_ms={} base_ms={base_ms}",
        read_probe_text(&probe, caller_ms, |r| elements_result_text(r, &base)),
        after_ms_text(&stalled_at, returned)
    );
    assert!(
        stall_set,
        "根の後の閉包で眠りを置けなかった (閉包が呼ばれないか、置けない。空振り)"
    );
    assert_aborted_below_root(
        "b",
        &probe,
        "子の要素の読み: ",
        after_ms(&stalled_at, returned),
    );
}

/// 固まった窓の本番の要素の読み (D-109 の (c)): フィクスチャのスレッドを 6 秒眠らせてから本番の `elements` (既定の
/// 待ち) を読み、経過と結果を `FIXTURE read_probe c` の行に出す (既定の待ちが眠りより短く切れずに、明けるまで待つか)。
/// 断言は場の対照 (眠らせない本番の読みが 3 要素)、眠りを置けたこと、読みの始めで眠っていたことだけ。`Ok` も
/// `Err(Failed)` も `Err(TimedOut)` も行に出して通す。前面化も入力もしない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_elements_on_stalled_fixture() {
    let f = Fixture::open_with_controls("read-probe-c");
    let hwnd = f.win.hwnd();
    let start = Instant::now();
    let base = uia_elements(hwnd, &read_probe_query()).expect("眠らせない本番の要素の読み");
    let base_ms = start.elapsed().as_millis();
    println!(
        "FIXTURE read_probe c_base ms={base_ms} result=Ok n={} skipped={} truncated={}",
        base.elements.len(),
        base.skipped,
        base.truncated
    );
    assert_eq!(base.elements.len(), 3, "眠らせない本番の読み: {base:?}");
    assert_eq!(base.skipped, 0, "眠らせない本番の読み: {base:?}");
    assert!(!base.truncated, "眠らせない本番の読み: {base:?}");
    let stall_set = f.win.stall(FOCUS_STALL);
    let stalled_before = f.win.is_stalled();
    let start = Instant::now();
    let r = uia_elements(hwnd, &read_probe_query());
    let ms = start.elapsed().as_millis();
    let stalled_after = f.win.is_stalled();
    let woke = wait_until(FOCUS_WAKE_LIMIT, || !f.win.is_stalled());
    let result = match &r {
        Ok(e) => elements_result_text(&Ok(e.clone()), &base),
        Err(UiaError::Failed(m)) => format!("Err {}", head60(m)),
        Err(e @ UiaError::TimedOut(_)) => format!("Probe({})", uia_error_kind(e)),
    };
    println!(
        "FIXTURE read_probe c ms={ms} result={result} stalled_before={stalled_before} stalled_after={stalled_after} stall_set={stall_set} woke={woke} base_ms={base_ms}"
    );
    assert!(stall_set, "眠りを置けなかった (空振り)");
    assert!(
        stalled_before,
        "読みの始めで眠っていない (空振り): before={stalled_before}"
    );
}

/// 固まった窓の木の読み (D-109 の (d)): 縮めた待ちの計測の入口で木 (深さ 3、上限 20) を読み、根の子を取れた後・最初の子の
/// `describe` の前にフィクスチャのスレッドを 12 秒眠らせて、子の読みで最初の UIA の時間切れで打ち切るかを
/// `FIXTURE read_probe d` の行に出す。断言は場の対照 (眠らせない同じ読みが 3 ノードで `skipped` 0)、閉包で眠りを
/// 置けたこと、probe が Ok で返ったこと、`result` が `Err(ReadError::TimedOut(m))` で m が「深さ 1 の要素の読み: 」を
/// 含むこと (眠りが根を読んだ後に入った確かめ)、`after_ms` が `ABORT_LIMIT_MS` 未満 (待ちが重ならない) こと (D-110)。
/// 前面化も入力もしない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_read_probe_stalled_before_tree_child() {
    let f = Fixture::open_with_controls("read-probe-d");
    let hwnd = f.win.hwnd();
    let bp = probe_tree(
        hwnd,
        READ_PROBE_TREE_DEPTH,
        READ_PROBE_TREE_NODES,
        true,
        || {},
    )
    .expect("眠らせない木の計測");
    let base = bp.result.clone().expect("眠らせない木の読み");
    let base_ms = bp.ms;
    println!(
        "FIXTURE read_probe d_base short_timeouts={} ms={base_ms} result=Ok n={} skipped={} truncated={} depth_cut={}",
        bp.short_timeouts,
        base.nodes.len(),
        base.skipped,
        base.node_cap_hit,
        base.depth_cut
    );
    assert_eq!(base.nodes.len(), 3, "眠らせない木の読み: {base:?}");
    assert_eq!(base.skipped, 0, "眠らせない木の読み: {base:?}");
    let (before_child_read, stall_set, stalled_at) = stall_in_closure(&f);
    let stalled_before = f.win.is_stalled();
    let start = Instant::now();
    let probe = probe_tree(
        hwnd,
        READ_PROBE_TREE_DEPTH,
        READ_PROBE_TREE_NODES,
        true,
        before_child_read,
    );
    let returned = Instant::now();
    let caller_ms = returned.duration_since(start).as_millis();
    let stalled_after = f.win.is_stalled();
    let woke = wait_until(READ_WAKE_LIMIT, || !f.win.is_stalled());
    let stall_set = stall_set.load(Ordering::SeqCst);
    println!(
        "FIXTURE read_probe d {} stalled_before={stalled_before} stalled_after={stalled_after} stall_set={stall_set} woke={woke} after_ms={} base_ms={base_ms}",
        read_probe_text(&probe, caller_ms, |r| tree_result_text(r, &base)),
        after_ms_text(&stalled_at, returned)
    );
    assert!(
        stall_set,
        "最初の子の読みの前の閉包で眠りを置けなかった (閉包が呼ばれないか、置けない。空振り)"
    );
    assert_aborted_below_root(
        "d",
        &probe,
        "深さ 1 の要素の読み: ",
        after_ms(&stalled_at, returned),
    );
}

/// 撮影の判定の重なりの一覧 (D-077): フィクスチャ (題名つきのツールウィンドウ) が `stack_windows` に、`describe` と同じ
/// 枠の矩形で、題名なしで出る。フィクスチャ専用の装置の `stack_windows` は、一覧の門 (OutOfScope) で断らずに
/// フィクスチャだけを返す。読み取りだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_stack_windows_report_fixture_frame_untitled() {
    let f = Fixture::open("stack");
    let hwnd = f.win.hwnd();
    let stack = window::stack_windows();
    let me = stack
        .iter()
        .find(|w| w.info.hwnd == hwnd)
        .expect("フィクスチャが重なりの一覧に出る");
    let expected = window::describe(hwnd)
        .and_then(|d| d.rect)
        .expect("フィクスチャの枠の矩形");
    assert_eq!(me.rect, expected, "枠の矩形が describe と一致する");
    assert!(
        me.info.title.is_empty(),
        "題名は読まない: {:?}",
        me.info.title
    );
    assert_eq!(me.info.pid, std::process::id());
    let fixture_only = Win32Desktop::for_fixture(f.scope())
        .stack_windows()
        .expect("フィクスチャ専用の装置は断らない");
    assert_eq!(fixture_only.len(), 1, "フィクスチャだけ: {fixture_only:?}");
    assert_eq!(fixture_only[0].info.hwnd, hwnd);
    assert!(fixture_only[0].info.title.is_empty());
    println!(
        "FIXTURE stack {} windows, fixture rect={} see_through={} class={} fixture_only={}",
        stack.len(),
        me.rect,
        me.see_through,
        me.info.class,
        fixture_only.len()
    );
}

/// 撮影から外れる窓 (D-077): 層付きでないフィクスチャでも `GetWindowDisplayAffinity` が成功して値を返すこと
/// (文書には「層付きの窓でだけ成功する」とあり、失敗すると外れた窓を覆いに数えて奥の deny の窓が写る) と、
/// `WDA_EXCLUDEFROMCAPTURE` を付けると重なりの一覧で覆いとみなさないことを確かめる。付けるのは自分の窓だけで、
/// 最後に `WDA_NONE` に戻す。入力も撮影もしない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_capture_excluded_fixture_is_not_a_cover() {
    const WDA_NONE: u32 = 0;
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
    let f = Fixture::open("affinity");
    let hwnd = f.win.hwnd();
    let before = window::display_affinity(hwnd);
    // 付ける前は覆い (前後の差がアフィニティによるものだと言うため)
    let cover_before = window::stack_window(hwnd).map(|w| w.see_through);
    let set = f.win.set_display_affinity(WDA_EXCLUDEFROMCAPTURE);
    let after = window::display_affinity(hwnd);
    let me = window::stack_window(hwnd);
    let reset = f.win.set_display_affinity(WDA_NONE);
    println!(
        "FIXTURE affinity before={before:?} see_through_before={cover_before:?} set={set} after={after:?} see_through={:?} reset={reset}",
        me.as_ref().map(|w| w.see_through)
    );
    assert_eq!(before, Some(WDA_NONE), "層付きでない普通の窓でも読める");
    assert_eq!(cover_before, Some(false), "付ける前は覆いとみなす");
    assert!(set, "自分の窓に WDA_EXCLUDEFROMCAPTURE を付けられる");
    assert_eq!(after, Some(WDA_EXCLUDEFROMCAPTURE));
    assert_eq!(
        me.map(|w| w.see_through),
        Some(true),
        "撮影から外れる窓は覆いとみなさない"
    );
    assert!(reset);
}

/// 窓のリージョン (D-112): フィクスチャに `SetWindowRgn` でリージョンを付け、重なりの一覧の 1 窓 (`stack_window`) が
/// 枠まるごとの矩形では覆い (`see_through` が偽)、枠の一部の矩形と楕円では覆いでない (真)、外すと覆いに戻ることと、
/// どれでも `rect` が付ける前と同じ (リージョンで縮めない) ことを確かめる。フィクスチャは見えない縁の無い
/// `WS_POPUP | WS_BORDER` なので、`GetWindowRect` と DWM の枠の左上が同じという前提で、枠まるごとの矩形を
/// `Rect::new(0, 0, before.w, before.h)` にする (違えば (b) で落ちる)。値を全部取ってから外し、断言はその後。
/// 付けるのは自分の窓だけで、入力も撮影もしない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_window_region_drops_partial_cover() {
    let f = Fixture::open("region");
    let hwnd = f.win.hwnd();
    let read = || window::stack_window(hwnd).map(|w| (w.rect, w.see_through));
    // (a) 付ける前
    let a = read();
    let before = a.map(|(r, _)| r).expect("フィクスチャが重なりの一覧に出る");
    // (b) 枠まるごと
    let set_full = f
        .win
        .set_region(FixtureRegion::Rect(Rect::new(0, 0, before.w, before.h)));
    let b = read();
    // (c) 枠の一部の矩形
    let set_part = f
        .win
        .set_region(FixtureRegion::Rect(Rect::new(10, 20, 300, 200)));
    let c = read();
    // (d) 楕円
    let set_ellipse = f.win.set_region(FixtureRegion::Ellipse);
    let d = read();
    // (e) 外した後
    let set_clear = f.win.set_region(FixtureRegion::Clear);
    let e = read();
    let see = |v: Option<(Rect, bool)>| v.map(|(_, s)| s);
    let rects_same = [a, b, c, d, e]
        .iter()
        .all(|v| v.map(|(r, _)| r) == Some(before));
    println!(
        "FIXTURE region before={before:?} full_see={:?} part_see={:?} ellipse_see={:?} cleared_see={:?} rects_same={rects_same}",
        see(b),
        see(c),
        see(d),
        see(e)
    );
    assert!(
        set_full && set_part && set_ellipse && set_clear,
        "自分の窓にリージョンを付け外しできる (full={set_full} part={set_part} ellipse={set_ellipse} clear={set_clear})"
    );
    assert_eq!(see(a), Some(false), "付ける前は覆いとみなす");
    assert_eq!(
        see(b),
        Some(false),
        "枠まるごとのリージョンは覆いとみなす (before={before:?}。GetWindowRect と DWM の枠の左上が同じ前提)"
    );
    assert_eq!(
        see(c),
        Some(true),
        "枠の一部の矩形のリージョンは覆いとみなさない"
    );
    assert_eq!(see(d), Some(true), "楕円のリージョンは覆いとみなさない");
    assert_eq!(see(e), Some(false), "リージョンを外すと覆いに戻る");
    assert!(
        rects_same,
        "rect はリージョンで縮めない (before={before:?} a={a:?} b={b:?} c={c:?} d={d:?} e={e:?})"
    );
}

/// 撮影の判定の実機テスト (D-077、D-115) の設定: テストの実行ファイル名を deny.processes に書く (撮影は有効)
fn deny_own_process_policy(source: &'static str) -> Policy {
    let exe = std::env::current_exe().expect("実行ファイルのパス");
    let exe_name = exe
        .file_name()
        .expect("ファイル名")
        .to_string_lossy()
        .into_owned();
    let cfg = Config::parse(&format!(
        "version = 1\n[general]\ncapture_enabled = true\n[deny]\nprocesses = ['{exe_name}']\n"
    ))
    .expect("テストの設定");
    let policy = Policy::new(vec![Layer::from_config(&cfg, source).unwrap()]);
    assert!(policy.has_window_deny());
    policy
}

/// 撮影の判定 (D-077): テストの実行ファイル名を deny.processes に書いた設定で、実際の重なりの一覧を読み、フィクスチャの
/// 枠の右端の内側 1 ピクセルにかかる矩形は断り (フィクスチャが見えている)、右端のすぐ外に接する矩形は通す。フィクスチャは
/// 最前面のツールウィンドウなので、ほかの窓に覆われない前提 (`open` の作法)。読み取りと判定だけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_sensitive_window_check_refuses_rect_touching_denied_fixture() {
    let f = Fixture::open("sensitive");
    let hwnd = f.win.hwnd();
    let policy = deny_own_process_policy("sensitive");
    let stack = window::stack_windows();
    let r = stack
        .iter()
        .find(|w| w.info.hwnd == hwnd)
        .map(|w| w.rect)
        .expect("フィクスチャが重なりの一覧に出る");
    let mid_y = r.y + r.h as i32 / 2;
    let inside = Rect::new(r.x + r.w as i32 - 1, mid_y, 10, 10);
    let outside = Rect::new(r.x + r.w as i32, mid_y, 10, 10);
    let hit = run::sensitive_window(&stack, &policy, &inside, &[], None);
    let (info, visible) = hit
        .clone()
        .expect("右端の内側 1 ピクセルにかかる矩形は断る");
    assert_eq!(info.hwnd, hwnd, "断る窓はフィクスチャ: {info:?}");
    let overlap = inside.intersection(&r).expect("右端の 1 ピクセルの重なり");
    assert_eq!(overlap.w, 1);
    assert!(
        overlap.contains_rect(&visible),
        "見えている部分は重なりの中: {visible} / {overlap}"
    );
    let miss = run::sensitive_window(&stack, &policy, &outside, &[], None);
    assert!(
        miss.as_ref().is_none_or(|(w, _)| w.hwnd != hwnd),
        "右端のすぐ外に接する矩形ではフィクスチャを数えない: {miss:?}"
    );
    assert!(miss.is_none(), "ほかに deny の窓は無い: {miss:?}");
    println!(
        "FIXTURE sensitive rect={r} inside={inside} visible={visible} outside={outside} hit={} miss={}",
        hit.is_some(),
        miss.is_some()
    );
}

/// ゴーストを待つ上限 (下調べでは眠らせてから約 5〜6 秒で出た。D-115)
const GHOST_APPEAR_LIMIT: Duration = Duration::from_secs(8);
/// フィクスチャを眠らせる長さ (ゴーストを待つ上限より長く、判定を読み終えるまで眠りが続くように)
const GHOST_STALL: Duration = Duration::from_secs(9);

/// `stack` のうち、矩形が `r` と同じゴーストの窓 (`is_ghost_window`) の最初のもの (ゴーストは元の窓と同じ矩形に出る。重なるだけの別のアプリのゴーストを拾わない)
fn ghost_over(stack: &[StackWindow], r: &Rect) -> Option<StackWindow> {
    stack
        .iter()
        .find(|w| run::is_ghost_window(&w.info) && w.rect == *r)
        .cloned()
}

/// 撮影の判定のゴースト (D-115): テストの実行ファイル名を deny.processes に書いた設定で、フィクスチャを眠らせて OS に
/// ゴーストの窓 (クラス Ghost) を出させ、フィクスチャの枠の中央の矩形が、そのゴーストの窓で断られることを確かめる。
/// 応答 (`hung`) をすべて偽にした写しでは通る (変える前の判定ではゴーストが不透明な覆いになり、漏れていた)。
/// 眠りが明けるとフィクスチャの矩形に重なるゴーストが消え、`hung` が偽に戻ることも確かめる。読み取りと判定だけ
/// (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_ghost_of_hung_denied_fixture_is_refused() {
    let f = Fixture::open("ghost");
    let hwnd = f.win.hwnd();
    let policy = deny_own_process_policy("ghost");
    let r = window::stack_windows()
        .iter()
        .find(|w| w.info.hwnd == hwnd)
        .map(|w| w.rect)
        .expect("フィクスチャが重なりの一覧に出る");
    assert!(f.win.stall(GHOST_STALL), "眠りの印が立たない");
    let start = Instant::now();
    // 待ちの条件に hung を入れない (対照で hung を偽にしたとき、判定の assert で落ちるように)
    let (stack, ghost) = loop {
        let stack = window::stack_windows();
        if let Some(g) = ghost_over(&stack, &r) {
            break (stack, g);
        }
        assert!(
            start.elapsed() < GHOST_APPEAR_LIMIT,
            "ゴーストが出ない ({} ms 待った)",
            start.elapsed().as_millis()
        );
        std::thread::sleep(Duration::from_millis(250));
    };
    let waited_ms = start.elapsed().as_millis();
    // (0) 元の窓は見えたまま (cloaked でもなく) 一覧に残る前提 (hung は見ない)
    let me = stack
        .iter()
        .find(|w| w.info.hwnd == hwnd)
        .cloned()
        .expect("ゴーストが出ている間もフィクスチャが重なりの一覧に残る");
    let pos = |s: &[StackWindow], h: usize| s.iter().position(|w| w.info.hwnd == h);
    let ghost_pos = pos(&stack, ghost.info.hwnd);
    let fixture_pos = pos(&stack, hwnd);
    let c = r.center();
    let mid = Rect::new(c.x - 5, c.y - 5, 10, 10);
    // 実の並びのまま: ゴーストが前でも後ろでも断る (後ろなら元の deny の窓自身が見えて当たる。D-115 の追記)
    let actual_hit = run::sensitive_window(&stack, &policy, &mid, &[], None);
    assert!(
        actual_hit.is_some(),
        "実の並びで断らない: ghost_pos={ghost_pos:?} fixture_pos={fixture_pos:?}"
    );
    // ゴーストが実の並びで前に立つ回は、実の一覧のまま新しい規則で当たる (前のゴーストが先に当たる)
    if ghost_pos < fixture_pos {
        assert!(
            actual_hit
                .as_ref()
                .is_some_and(|(w, _)| w.hwnd == ghost.info.hwnd),
            "ゴーストが前に立つ実の並びで、ゴーストで断らない: {actual_hit:?}"
        );
    }
    // 下調べ (Python の窓) ではゴーストが元の窓の真前に立った。その並びを、実の読みの値のまま作る
    // (フィクスチャの項をゴーストのすぐ後ろに動かす)。ai-cu のフィクスチャでもゴーストは前に立つ回と後ろに立つ回が
    // あった (D-115 の追記) ので、並べ替えて、どちらの回でもゴーストの規則を撃つ
    let mut front: Vec<StackWindow> = stack
        .iter()
        .filter(|w| w.info.hwnd != hwnd)
        .cloned()
        .collect();
    let gi = pos(&front, ghost.info.hwnd).expect("ゴーストは一覧にある");
    front.insert(gi + 1, me.clone());
    // (1) ゴーストが前の並びでは、枠の中央の 10x10 はゴーストで断る
    let hit = run::sensitive_window(&front, &policy, &mid, &[], None);
    let (info, _) = hit
        .clone()
        .expect("Some で無い: 応答しない deny の窓のゴーストで断らない");
    assert!(
        run::is_ghost_window(&info),
        "断る窓はゴースト: {info:?} ghost={ghost:?}"
    );
    assert_eq!(
        info.hwnd, ghost.info.hwnd,
        "断る窓は待ちで見つけたゴースト: {info:?}"
    );
    // (2) フィクスチャは応答しない
    assert!(me.hung, "眠っているフィクスチャの hung が真: {me:?}");
    // (3) ゴーストは不透明な覆いに数えられている (ゴーストが前に立つと、変える前は元の窓を隠して漏れていた前提)
    assert!(!ghost.see_through, "ゴーストは覆い: {ghost:?}");
    // (4) ゴーストが前の並びで hung をすべて偽にした写しでは通る (変える前の判定)
    let unhung: Vec<StackWindow> = front
        .iter()
        .cloned()
        .map(|mut w| {
            w.hung = false;
            w
        })
        .collect();
    let unhung_hit = run::sensitive_window(&unhung, &policy, &mid, &[], None);
    assert!(
        unhung_hit.is_none(),
        "hung を偽にした写しでは通る: {unhung_hit:?}"
    );
    println!(
        "FIXTURE ghost fixture={r} ghost={} ghost_pid={} ghost_process={:?} ghost_see_through={} ghost_elevated={:?} ghost_hung={} hung=true ghost_pos={ghost_pos:?} fixture_pos={fixture_pos:?} actual_hit_class={:?} hit_class={:?} unhung_hit={:?} waited_ms={waited_ms}",
        ghost.rect,
        ghost.info.pid,
        ghost.info.process,
        ghost.see_through,
        ghost.info.elevated,
        ghost.hung,
        actual_hit.map(|(w, _)| w.class),
        info.class,
        unhung_hit.map(|(w, _)| w.class),
    );
    // 眠りが明けるのを最大 5 秒待つ
    let wake = Instant::now();
    while f.win.is_stalled() {
        assert!(wake.elapsed() < Duration::from_secs(5), "眠りが明けない");
        std::thread::sleep(Duration::from_millis(50));
    }
    // 明けた後、フィクスチャの矩形と同じゴーストが消え、hung が偽に戻る (最大 3 秒。デスクトップ全体は見ない:
    // 利用者の別のアプリのゴーストで落ちないため)。どちらが先に起きたかを 20 ms ごとの読みで測る (hung が先に
    // 偽になりゴーストが残る間は、ゴーストが前に立つ並びなら判定が通ってしまう。win32-reviewer の指摘、D-115)
    let after = Instant::now();
    let mut unhung_ms: Option<u128> = None;
    let mut gone_ms: Option<u128> = None;
    let mut gap_reads = 0u32;
    loop {
        let stack = window::stack_windows();
        let left = ghost_over(&stack, &r);
        let hung = stack.iter().find(|w| w.info.hwnd == hwnd).map(|w| w.hung);
        let now = after.elapsed().as_millis();
        if hung == Some(false) && unhung_ms.is_none() {
            unhung_ms = Some(now);
        }
        if left.is_none() && gone_ms.is_none() {
            gone_ms = Some(now);
        }
        if hung == Some(false) && left.is_some() {
            gap_reads += 1;
        }
        if left.is_none() && hung == Some(false) {
            break;
        }
        assert!(
            after.elapsed() < Duration::from_secs(3),
            "眠りが明けた後もゴーストか hung が残る: ghost={left:?} hung={hung:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    println!(
        "FIXTURE ghost_recover unhung_ms={unhung_ms:?} ghost_gone_ms={gone_ms:?} gap_reads={gap_reads}"
    );
}

/// 窓の持ち主 (D-074): フィクスチャ (ツールウィンドウなので一覧に出ない) を SW_HIDE で隠した後も、その持ち主として
/// テストのプロセスの pid と実行ファイル名が出る (トレイに隠したのと同じ、見えない窓)。枠なしのフィクスチャ
/// (`open_without_frame`) で開くので、テストのプロセスはフィクスチャのほかに見えるトップレベルの窓を持たない (D-094)。
/// だから自分の pid が出るのは見えない窓から。ただし既定の IME 窓なども同じ pid を
/// 出すので、pid だけではフィクスチャ自身を確かめられない。隠したフィクスチャのハンドルが、見えるかで絞らない列挙
/// (`top_level_hwnds`) に入ることも確かめる。読み取りだけ (隠すのは自分の窓)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_window_owners_include_own_hidden_window() {
    let f = Fixture::open_without_frame("owners");
    f.win.hide();
    assert!(!f.win.is_visible(), "SW_HIDE の後は見えない");
    assert!(
        window::top_level_hwnds().contains(&f.win.hwnd()),
        "隠したフィクスチャも、見えるかで絞らない列挙に入る"
    );
    let me = std::process::id();
    let owners = window::window_owners();
    let mine: Vec<_> = owners.iter().filter(|o| o.pid == me).collect();
    assert_eq!(mine.len(), 1, "pid ごとに 1 つ: {mine:?}");
    let exe = std::env::current_exe().expect("実行ファイルのパス");
    let exe_name = exe
        .file_name()
        .expect("ファイル名")
        .to_string_lossy()
        .into_owned();
    assert!(
        mine[0].process.eq_ignore_ascii_case(&exe_name),
        "{} と {exe_name}",
        mine[0].process
    );
    println!(
        "FIXTURE window_owners {} owners, self={}",
        owners.len(),
        mine[0].process
    );
}

/// 隠れた窓の一覧 (D-099): フィクスチャが見えている間は `hidden_top_windows` に入らず、SW_HIDE で隠した後は
/// そのハンドルが `cloaked: false` (hidden) で、自分の pid、フィクスチャのクラス、空でない題名とともに入る。
/// 自分のプロセスの隠れた窓 (フィクスチャを除く) のクラスも出す (雑音のクラス `IME`、`MSCTFIME UI` の推定を
/// 確かめるため)。枠なしのフィクスチャで開く (自分の枠の窓を数えに入れない。D-094)。読み取りだけ (隠すのは自分の窓)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_hidden_top_windows_include_hidden_fixture() {
    let f = Fixture::open_without_frame("hidden-list");
    let hwnd = f.win.hwnd();
    assert!(f.win.is_visible(), "開いた直後は見えている");
    // 見えている窓の列挙 (stack_windows。題名やツールウィンドウで絞らない) と隠れた窓の列挙の、ちょうど一方に入る
    // (判定の元が hidden_state 1 つであることの実機の確かめ。D-099)
    let in_lists = || {
        let shown = window::stack_windows().iter().any(|s| s.info.hwnd == hwnd);
        let hidden = window::hidden_top_windows()
            .into_iter()
            .find(|h| h.info.hwnd == hwnd);
        (shown, hidden)
    };
    // 付け外しの直後に DWM の読みが変わっているかは文書に無いので、条件がそろうまで 1 秒まで読み直す
    let wait_lists = |ok: &dyn Fn(bool, &Option<ai_cu::device::HiddenWindow>) -> bool| {
        let start = std::time::Instant::now();
        loop {
            let (shown, hidden) = in_lists();
            if ok(shown, &hidden) || start.elapsed() > std::time::Duration::from_secs(1) {
                return (shown, hidden);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    let (shown, hidden) = in_lists();
    assert!(
        shown && hidden.is_none(),
        "見えている間は見える列挙だけに入る: {shown} {hidden:?}"
    );
    // cloak すると見えるが cloaked として隠れた窓の側に移る (別の仮想デスクトップと同じ見え方)
    assert!(f.win.set_cloak(true), "DWMWA_CLOAK を受け付ける");
    let (shown, hidden) = wait_lists(&|s, h| !s && h.as_ref().is_some_and(|h| h.cloaked));
    let cloaked_ok = !shown && hidden.as_ref().is_some_and(|h| h.cloaked);
    assert!(f.win.set_cloak(false), "cloak を戻せる");
    assert!(
        cloaked_ok,
        "cloak の間は隠れた窓の側に cloaked: true で入る: {shown} {hidden:?}"
    );
    let (shown, hidden) = wait_lists(&|s, h| s && h.is_none());
    assert!(
        shown && hidden.is_none(),
        "cloak を戻すと見える列挙に戻る: {shown} {hidden:?}"
    );
    f.win.hide();
    let (shown, _) = in_lists();
    assert!(!shown, "SW_HIDE の後は見える列挙に入らない");
    assert!(!f.win.is_visible(), "SW_HIDE の後は見えない");
    let hidden = window::hidden_top_windows();
    let me = std::process::id();
    let found = hidden
        .iter()
        .find(|h| h.info.hwnd == hwnd)
        .unwrap_or_else(|| panic!("隠したフィクスチャが一覧に無い ({} 件)", hidden.len()));
    assert!(
        !found.cloaked,
        "SW_HIDE は cloaked ではなく hidden: {found:?}"
    );
    assert_eq!(found.info.pid, me, "持ち主はテストのプロセス: {found:?}");
    assert!(
        found.info.class.starts_with("ai_cu_fixture_"),
        "フィクスチャのクラス: {found:?}"
    );
    assert!(!found.info.title.is_empty(), "題名が読める: {found:?}");
    let mut own_classes: Vec<&str> = Vec::new();
    for h in hidden
        .iter()
        .filter(|h| h.info.pid == me && h.info.hwnd != hwnd)
    {
        if !own_classes.contains(&h.info.class.as_str()) {
            own_classes.push(&h.info.class);
        }
    }
    let own = hidden.iter().filter(|h| h.info.pid == me).count();
    println!(
        "FIXTURE hidden_top_windows total={} own={own} own_classes={own_classes:?}",
        hidden.len()
    );
}

/// UI Automation (D-054): フィクスチャ自身のウィンドウの要素を COM 経由で見つけられ、矩形が物理ピクセルで
/// フィクスチャの矩形と一致する。読み取りだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_finds_fixture_window_element_in_physical_pixels() {
    let f = Fixture::open("uia");
    let desktop = Win32Desktop::for_fixture(f.scope());
    // 枠の無いポップアップは UIA の種類が window ではなく pane になりうるので、種類では絞らない
    let q = ElementQuery {
        limit: 20,
        ..Default::default()
    };
    let found = desktop
        .ui_elements(f.win.hwnd(), &q)
        .expect("UIA の問い合わせ");
    let expected = f.win.window_rect().unwrap();
    let me = found
        .elements
        .iter()
        .find(|e| e.rect == Some(expected))
        .unwrap_or_else(|| {
            panic!("フィクスチャの矩形 {expected} と一致する要素が無い (UIA の矩形が物理ピクセルでない可能性): {found:?}")
        });
    assert!(
        matches!(me.control_type.as_str(), "window" | "pane"),
        "{me:?}"
    );
    assert!(!me.offscreen);
    // 種類で絞れば、その種類だけが返る
    let typed = ElementQuery {
        control_type: Some(me.control_type.clone()),
        limit: 20,
        ..Default::default()
    };
    let only = desktop.ui_elements(f.win.hwnd(), &typed).unwrap();
    assert!(
        only.elements
            .iter()
            .all(|e| e.control_type == me.control_type),
        "{only:?}"
    );
    // 範囲外のウィンドウには応じない (フィクスチャ専用の装置)
    assert!(matches!(
        desktop.ui_elements(0xDEAD, &q),
        Err(BackendError::OutOfScope(_))
    ));
    println!(
        "FIXTURE uia element name={:?} type={} rect={expected}",
        me.name, me.control_type
    );
}

/// 要素の木 (D-084): フィクスチャの根 (子の無い窓) を ControlViewWalker でたどると、深さ 0 の要素が根の 1 つだけで、
/// 子の無い根の `GetFirstChildElement` の null を失敗に数えない (`skipped` 0)。整形の 1 行目は根の中心を `@x,y` で出す。
/// 読み取りだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_tree_lists_fixture_root_with_center() {
    let f = Fixture::open("uia-tree");
    let desktop = Win32Desktop::for_fixture(f.scope());
    let t = desktop
        .ui_tree(f.win.hwnd(), TREE_DEPTH_DEFAULT, TREE_MAX_NODES)
        .expect("UIA の木");
    let expected = f.win.window_rect().unwrap();
    let root = &t.nodes.first().expect("根が無い").element;
    assert_eq!(t.nodes[0].depth, 0, "{t:?}");
    assert_eq!(root.rect, Some(expected), "{t:?}");
    assert!(
        matches!(root.control_type.as_str(), "window" | "pane"),
        "{root:?}"
    );
    assert_eq!(
        t.nodes.iter().filter(|n| n.depth == 0).count(),
        1,
        "根の兄弟をたどった: {t:?}"
    );
    assert_eq!(t.skipped, 0, "null を失敗に数えた: {t:?}");
    assert_eq!(t.depth_cut, 0, "{t:?}");
    assert!(!t.node_cap_hit, "{t:?}");
    let r = render_tree(&t, TREE_LIMIT_DEFAULT, false);
    let c = expected.center();
    let first = r.text.lines().next().unwrap_or_default();
    assert!(
        first.contains(&format!("@{},{}", c.x, c.y)),
        "1 行目に中心が無い: {first:?}"
    );
    // 範囲外のウィンドウには応じない (フィクスチャ専用の装置)
    assert!(matches!(
        desktop.ui_tree(0xDEAD, TREE_DEPTH_DEFAULT, TREE_MAX_NODES),
        Err(BackendError::OutOfScope(_))
    ));
    println!(
        "FIXTURE uia tree nodes={} depth_cut={} skipped={} first={:?}",
        t.nodes.len(),
        t.depth_cut,
        t.skipped,
        first
    );
}

/// 子つきのフィクスチャの EDIT (複数行・読み取り専用) の UIA の種類か。
/// D-088 では未確認のため `edit` と `document` の両方を受け入れていたが、D-088 の実機の走りで `edit` だったので、
/// D-089 で `edit` に絞った (別の環境で `document` が出たら戻す)
fn is_fixture_edit_type(t: &str) -> bool {
    t == "edit"
}

/// UIA の子孫と文字列 (D-088): 子つきのフィクスチャで、制限なしの問い合わせが根・ボタン・エディットを木の順に返し、
/// 子の矩形が子の窓の矩形と一致する。名前と種類で絞れ、`text` の問い合わせはエディットの本文を LF にそろえて返し、
/// ボタンの名前で絞ると文字列が無い (CLI の `text` が 13 にする条件)。読むだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_find_and_text_read_fixture_children() {
    // 根の要素の名前は題名 `ai-cu fixture <name> <pid>`。名前の絞り込みに根が当たらないよう、
    // name に `button` や `二行目` を含めない
    let f = Fixture::open_with_controls("uia-children");
    let desktop = Win32Desktop::for_fixture(f.scope());
    let button_rect = f.win.child_rect(FIXTURE_BUTTON_ID).expect("ボタンの矩形");
    let edit_rect = f.win.child_rect(FIXTURE_EDIT_ID).expect("エディットの矩形");
    assert!(
        f.client.contains_rect(&button_rect),
        "ボタンがクライアント領域 {} からはみ出す: {button_rect}",
        f.client
    );
    assert!(
        f.client.contains_rect(&edit_rect),
        "エディットがクライアント領域 {} からはみ出す: {edit_rect}",
        f.client
    );

    // (1) 制限なしの問い合わせ: 根、ボタン、エディットの 3 つが木の順に返る
    let all = ElementQuery {
        limit: 20,
        ..Default::default()
    };
    let found = desktop
        .ui_elements(f.win.hwnd(), &all)
        .expect("UIA の問い合わせ");
    assert_eq!(found.elements.len(), 3, "{found:?}");
    assert_eq!(found.skipped, 0, "{found:?}");
    assert!(!found.truncated, "{found:?}");
    let expected_root = f.win.window_rect().unwrap();
    assert_eq!(found.elements[0].rect, Some(expected_root), "{found:?}");
    let button = &found.elements[1];
    assert_eq!(button.control_type, "button", "{found:?}");
    assert_eq!(button.name, FIXTURE_BUTTON_TEXT, "{found:?}");
    assert_eq!(button.rect, Some(button_rect), "{found:?}");
    // automation_id は子の窓の ID (D-088 の実機の走りで "101" / "102"。D-089 で断言にした)
    assert_eq!(button.automation_id, "101", "{found:?}");
    let edit = &found.elements[2];
    assert!(is_fixture_edit_type(&edit.control_type), "{found:?}");
    assert_eq!(edit.rect, Some(edit_rect), "{found:?}");
    assert_eq!(edit.automation_id, "102", "{found:?}");
    let edit_type = edit.control_type.clone();

    // (2) 名前の部分一致 (大文字小文字を無視) でボタンだけ、エディットの実際の種類でエディットだけ
    let by_name = ElementQuery {
        name: Some("FIXTURE BUTTON".into()),
        limit: 20,
        ..Default::default()
    };
    let named = desktop.ui_elements(f.win.hwnd(), &by_name).unwrap();
    assert_eq!(named.elements.len(), 1, "{named:?}");
    assert_eq!(named.elements[0].rect, Some(button_rect), "{named:?}");
    assert!(!named.truncated, "{named:?}");
    let by_type = ElementQuery {
        control_type: Some(edit_type.clone()),
        limit: 20,
        ..Default::default()
    };
    let typed = desktop.ui_elements(f.win.hwnd(), &by_type).unwrap();
    assert_eq!(typed.elements.len(), 1, "{typed:?}");
    assert_eq!(typed.elements[0].rect, Some(edit_rect), "{typed:?}");
    assert!(!typed.truncated, "{typed:?}");

    // (3) text: 最初に文字列を持つ要素はエディットで、本文は LF にそろう
    let expected_text = normalize_newlines(FIXTURE_EDIT_TEXT);
    for q in [
        ElementQuery::for_text(None, None),
        ElementQuery::for_text(None, Some(edit_type.clone())),
    ] {
        let read = desktop
            .ui_elements(f.win.hwnd(), &q)
            .expect("文字列の問い合わせ");
        let (e, text) = first_text(&read.elements)
            .unwrap_or_else(|| panic!("文字列を持つ要素が無い (q={q:?}): {read:?}"));
        assert!(
            is_fixture_edit_type(&e.control_type)
                && e.rect == Some(edit_rect)
                && text == expected_text,
            "エディットの本文でない (q={q:?}): type={} name={:?} rect={:?} text={text:?}",
            e.control_type,
            e.name,
            e.rect
        );
    }

    // (4) ボタンの名前で絞ると文字列が無い (CLI の `text` が 13 にする条件。src/cli.rs の text_cmd)
    let q = ElementQuery::for_text(Some(FIXTURE_BUTTON_TEXT.into()), None);
    let read = desktop
        .ui_elements(f.win.hwnd(), &q)
        .expect("文字列の問い合わせ");
    if let Some((e, text)) = first_text(&read.elements) {
        panic!(
            "ボタンが文字列を返した: type={} name={:?} rect={:?} text={text:?}",
            e.control_type, e.name, e.rect
        );
    }

    assert!(f.win.events().is_empty(), "{:?}", f.win.events());
    println!(
        "FIXTURE uia children button_aid={:?} edit_aid={:?} edit_type={} edit_name={:?} button_rect={} edit_rect={}",
        button.automation_id, edit.automation_id, edit_type, edit.name, button_rect, edit_rect
    );
}

/// 要素の木の子の行 (D-084、D-088): 子つきのフィクスチャを ControlViewWalker でたどると、根 1 つと深さ 1 の
/// ボタン、エディットの 3 つが前順に返り、整形の 2 行目にボタンの種類と名前と中心の `@x,y` が出る。
/// 読むだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_tree_lists_fixture_children_with_centers() {
    // 根の要素の名前は題名。name に `button` や `二行目` を含めない (find と text のテストと同じ理由)
    let f = Fixture::open_with_controls("uia-children-tree");
    let desktop = Win32Desktop::for_fixture(f.scope());
    let button_rect = f.win.child_rect(FIXTURE_BUTTON_ID).expect("ボタンの矩形");
    let t = desktop
        .ui_tree(f.win.hwnd(), TREE_DEPTH_DEFAULT, TREE_MAX_NODES)
        .expect("UIA の木");
    assert_eq!(t.nodes.len(), 3, "{t:?}");
    let depths: Vec<usize> = t.nodes.iter().map(|n| n.depth).collect();
    assert_eq!(depths, [0, 1, 1], "{t:?}");
    assert_eq!(t.nodes[1].element.control_type, "button", "{t:?}");
    assert!(
        is_fixture_edit_type(&t.nodes[2].element.control_type),
        "{t:?}"
    );
    assert_eq!(t.skipped, 0, "{t:?}");
    assert_eq!(t.depth_cut, 0, "{t:?}");
    assert!(!t.node_cap_hit, "{t:?}");
    let r = render_tree(&t, TREE_LIMIT_DEFAULT, false);
    let c = button_rect.center();
    let second = r.text.lines().nth(1).unwrap_or_default();
    assert!(
        second.contains("button")
            && second.contains(FIXTURE_BUTTON_TEXT)
            && second.ends_with(&format!(" @{},{}", c.x, c.y)),
        "2 行目にボタンの種類・名前・行末の中心 @{},{} が無い: {:?}",
        c.x,
        c.y,
        r.text
    );
    assert!(f.win.events().is_empty(), "{:?}", f.win.events());
    println!("FIXTURE uia children tree={:?}", r.text);
}

/// 要素を 1 行で書く (種類と automation_id。0 件なら `none`)。結果の行に使う
fn kind_aid(found: &Elements) -> String {
    if found.elements.is_empty() {
        return "none".into();
    }
    found
        .elements
        .iter()
        .map(|e| format!("{}:{}", e.control_type, e.automation_id))
        .collect::<Vec<_>>()
        .join(",")
}

/// find の絞り込み (D-089): 子つきのフィクスチャで、automation_id は完全一致で効き (`"101"` でボタンだけ、`"10"` で
/// 何も当たらない)、パターンを持たない子 (ボタン、エディット) には toggle / selected / expanded が出ない。
/// 前面化の後のフォーカスを持つ要素は 1 つ以下で、エディットをクリックした後は focused + `"102"` でエディットが当たり、
/// 絞らない問い合わせ (結果に入れる直前に読む経路) と `text` の問い合わせ (with_text の経路) でもエディットの
/// `focused` が真になる。クリックはフォーカスを移す手段で、子の記録は見ない (D-088 の覆る条件に当たらない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_find_filters_fixture_children_by_automation_id_and_focus() {
    // 根の要素の名前は題名 `ai-cu fixture uia-focus <pid>`。名前では絞らない
    let f = Fixture::open_with_controls("uia-focus");
    let desktop = Win32Desktop::for_fixture(f.scope());
    let button_rect = f.win.child_rect(FIXTURE_BUTTON_ID).expect("ボタンの矩形");
    let edit_rect = f.win.child_rect(FIXTURE_EDIT_ID).expect("エディットの矩形");
    // limit を明示する (既定の 0 では 0 件の断言が性質と無関係に通る)
    let q = |aid: Option<&str>, focused: bool| ElementQuery {
        automation_id: aid.map(str::to_string),
        focused,
        limit: 20,
        ..Default::default()
    };
    let ask = |query: &ElementQuery| {
        desktop
            .ui_elements(f.win.hwnd(), query)
            .expect("UIA の問い合わせ")
    };

    // 1. automation_id の完全一致と、パターンを持たない子に状態が出ないこと
    let b = ask(&q(Some("101"), false));
    assert_eq!(b.elements.len(), 1, "{b:?}");
    assert!(!b.truncated, "{b:?}");
    let button = &b.elements[0];
    assert_eq!(button.control_type, "button", "{b:?}");
    assert_eq!(button.rect, Some(button_rect), "{b:?}");
    let none10 = ask(&q(Some("10"), false));
    assert!(
        none10.elements.is_empty() && !none10.truncated,
        "\"10\" が部分一致で当たった: {none10:?}"
    );
    let ed = ask(&q(Some("102"), false));
    assert_eq!(ed.elements.len(), 1, "{ed:?}");
    let edit = &ed.elements[0];
    assert!(is_fixture_edit_type(&edit.control_type), "{ed:?}");
    assert_eq!(edit.rect, Some(edit_rect), "{ed:?}");
    for e in [button, edit] {
        assert!(
            e.toggle.is_none() && e.selected.is_none() && e.expanded.is_none(),
            "パターンを持たないはずの子に状態が出た (出た値を D-089 に記録して断言を見直す): aid={} toggle={:?} selected={:?} expanded={:?}",
            e.automation_id,
            e.toggle,
            e.selected,
            e.expanded
        );
    }

    // 2. 前面化の後: フォーカスを持つ要素は 1 つ以下 (どれかは断言せず出力に出す)
    f.activate();
    let after_activate = ask(&q(None, true));
    assert!(after_activate.elements.len() <= 1, "{after_activate:?}");
    assert!(!after_activate.truncated, "{after_activate:?}");
    if let Some(e) = after_activate.elements.first() {
        assert!(e.focused, "{after_activate:?}");
    }

    // 3. エディットをクリックしてフォーカスを移し、focused + "102" でエディットが当たるまで待つ (最大 2 秒)
    let c = edit_rect.center();
    f.execute(&format!(r#"{{"type":"click","x":{},"y":{}}}"#, c.x, c.y))
        .expect("エディットの中心のクリック");
    let start = Instant::now();
    let after_click = loop {
        let r = ask(&q(Some("102"), true));
        if r.elements.len() == 1 || start.elapsed() > Duration::from_secs(2) {
            break r;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        after_click.elements.len(),
        1,
        "クリックの後 2 秒でエディットがフォーカスを持たない: {after_click:?}"
    );
    assert!(after_click.elements[0].focused, "{after_click:?}");
    let not_button = ask(&q(Some("101"), true));
    assert!(
        not_button.elements.is_empty() && !not_button.truncated,
        "フォーカスの無いボタンが当たった: {not_button:?}"
    );
    // 2 つの経路の結果を先に取ってから、まとめて断言する (片方が外れても、もう片方を出す)
    let unfiltered = ask(&q(Some("102"), false));
    let via_result = unfiltered.elements.first().map(|e| e.focused);
    let read = ask(&ElementQuery::for_text(None, None));
    let via_text = first_text(&read.elements).map(|(e, _)| (e.automation_id.clone(), e.focused));
    assert!(
        via_result == Some(true) && via_text == Some(("102".to_string(), true)),
        "エディットの focused が真でない: 絞らない問い合わせ (i)={via_result:?} text の問い合わせ (ii)={via_text:?} \
         (i)={unfiltered:?} (ii)={read:?}"
    );

    println!(
        "FIXTURE uia focus after_activate={} after_click={}",
        kind_aid(&after_activate),
        kind_aid(&after_click)
    );
}

/// パスワード欄の automation_id ("103") で絞る問い合わせ (limit を明示する。既定の 0 では 0 件の断言が性質と無関係に通る)
fn password_query(focused: bool) -> ElementQuery {
    ElementQuery {
        automation_id: Some(FIXTURE_PASSWORD_ID.to_string()),
        focused,
        limit: 20,
        ..Default::default()
    }
}

/// パスワードの要素 (D-090): `ES_PASSWORD` の EDIT は find で `password == true` になり、文字列を読む問い合わせ
/// (`text` と `wait_for` の文字列の条件の経路) では読まずに飛ばされて `password_skipped` に数えられ、返る要素が無い。
/// 結果のどこにも欄の中身が出ない。読むだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_uia_password_edit_is_flagged_and_not_read() {
    let f = Fixture::open_with_password("uia-password");
    let desktop = Win32Desktop::for_fixture(f.scope());
    // 中身が無いことの断言が空回りしないよう、欄が実際に中身を持つことを同じプロセスから先に確かめる
    assert_eq!(
        f.win.password_edit_text().as_deref(),
        Ok(FIXTURE_PASSWORD_TEXT)
    );

    let found = desktop
        .ui_elements(f.win.hwnd(), &password_query(false))
        .expect("UIA の問い合わせ");
    assert_eq!(found.elements.len(), 1, "{found:?}");
    let e = &found.elements[0];
    assert!(e.password, "パスワード欄の password が真でない: {found:?}");
    assert!(e.text.is_none(), "{found:?}");

    let read = desktop
        .ui_elements(f.win.hwnd(), &ElementQuery::for_text(None, None))
        .expect("文字列の問い合わせ");
    assert!(
        read.elements.is_empty() && read.password_skipped >= 1,
        "パスワード欄が読まずに飛ばされていない: {read:?}"
    );
    for out in [format!("{found:?}"), format!("{read:?}")] {
        assert!(
            !out.contains(FIXTURE_PASSWORD_TEXT),
            "結果に欄の中身が出た: {out}"
        );
    }
    assert!(f.win.events().is_empty(), "{:?}", f.win.events());
    println!(
        "FIXTURE password find={} password={} skipped={}",
        kind_aid(&found),
        e.password,
        read.password_skipped
    );
}

/// 最大 2 秒、`pred` が真になるまで UIA のパスワード欄の問い合わせ (`password_query(focused)`) を繰り返す
fn wait_password_query(
    f: &Fixture,
    desktop: &Win32Desktop,
    focused: bool,
    pred: impl Fn(&Elements) -> bool,
) -> Elements {
    let start = Instant::now();
    loop {
        let r = desktop
            .ui_elements(f.win.hwnd(), &password_query(focused))
            .expect("UIA の問い合わせ");
        if pred(&r) || start.elapsed() > Duration::from_secs(2) {
            return r;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// パスワード欄の文字列の長さが `len` になるまで最大 2 秒待つ (WM_CHAR は非同期に処理される)。最後に読んだ文字列を返す
fn wait_password_len(f: &Fixture, len: usize) -> String {
    let start = Instant::now();
    loop {
        let s = f.win.password_edit_text().expect("パスワード欄の文字列");
        if s.chars().count() == len || start.elapsed() > Duration::from_secs(2) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 4 (`policy_denied`) で、rule が `general.password_input` であること (allow.processes の 4 や 7 と区別する)
fn assert_password_refused(r: Result<RunReport, AppError>, what: &str) {
    let e = match r {
        Ok(rep) => panic!("{what}: パスワード欄への type が通った: {rep:?}"),
        Err(e) => e,
    };
    assert_eq!(e.kind, ErrorKind::PolicyDenied, "{what}: {e}");
    assert_eq!(
        e.deny.as_ref().map(|d| d.rule),
        Some("general.password_input"),
        "{what}: {e}"
    );
}

/// パスワード欄への入力 (D-090): フィクスチャ限定の実入力で、パスワード欄をクリックしてフォーカスを移すと、
/// 既定の設定 (password_input 偽) の type は 4 で欄の文字列は変わらない。同じ操作列の click の直後の type
/// (click の後の落ち着き待ちだけ) でも 4 になる。password_input を許した設定では type が届いて 1 文字増える。
/// 文字列が変わらないことを先に、届くことを最後に見る (届いた後は初期の文字列でなくなるため)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_type_into_password_edit_refused_by_default_and_allowed_by_config() {
    let f = Fixture::open_with_password("password-type");
    let desktop = Win32Desktop::for_fixture(f.scope());
    let edit = f
        .win
        .child_rect(FIXTURE_PASSWORD_ID)
        .expect("パスワード欄の矩形");
    assert!(
        f.client.contains_rect(&edit),
        "パスワード欄がクライアント領域 {} からはみ出す: {edit}",
        f.client
    );
    let c = edit.center();
    let click_edit = format!(r#"{{"type":"click","x":{},"y":{}}}"#, c.x, c.y);
    let type_x = r#"{"type":"type","text":"x"}"#;
    f.activate();

    // (1) 欄をクリックしてフォーカスを移し、focused + "103" で当たるまで待ってから、既定の設定で type する
    f.execute(&click_edit).expect("パスワード欄のクリック");
    let focused = wait_password_query(&f, &desktop, true, |r| r.elements.len() == 1);
    assert_eq!(
        focused.elements.len(),
        1,
        "クリックの後 2 秒でパスワード欄がフォーカスを持たない: {focused:?}"
    );
    let focus = desktop.focused_is_password();
    println!("FIXTURE password focus={focus:?}");
    assert_password_refused(f.execute(type_x), "既定の設定");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        f.win.password_edit_text().as_deref(),
        Ok(FIXTURE_PASSWORD_TEXT),
        "断ったのに欄の文字列が変わった"
    );

    // (2) フォーカスを親に戻し (親の空いた所のクリック)、click と type を 1 つの操作列で既定の設定のまま実行する
    let away = f.at(400, 300);
    assert!(!edit.contains(away), "親の空いた所が欄に重なる: {away:?}");
    f.execute(&format!(
        r#"{{"type":"click","x":{},"y":{}}}"#,
        away.x, away.y
    ))
    .expect("親の空いた所のクリック");
    let unfocused = wait_password_query(&f, &desktop, true, |r| r.elements.is_empty());
    assert!(
        unfocused.elements.is_empty(),
        "親をクリックした後 2 秒でパスワード欄のフォーカスが外れない: {unfocused:?}"
    );
    assert_password_refused(
        f.execute(&format!("{click_edit},{type_x}")),
        "同じ操作列の click の直後",
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        f.win.password_edit_text().as_deref(),
        Ok(FIXTURE_PASSWORD_TEXT),
        "同じ操作列の click の直後: 断ったのに欄の文字列が変わった"
    );

    // (3) password_input を許した設定では type が届き、文字列が 12 から 13 文字になる
    let allowed = fixture_policy(f.client, |cfg| cfg.general.password_input = true);
    let focused = wait_password_query(&f, &desktop, true, |r| r.elements.len() == 1);
    assert_eq!(focused.elements.len(), 1, "{focused:?}");
    f.execute_with(&allowed, type_x)
        .expect("password_input を許した設定の type");
    let after = wait_password_len(&f, FIXTURE_PASSWORD_TEXT.chars().count() + 1);
    assert_eq!(
        after.chars().count(),
        FIXTURE_PASSWORD_TEXT.chars().count() + 1,
        "許した設定の type が欄に届かない"
    );
    println!(
        "FIXTURE password type refused=2 allowed_len={}",
        after.chars().count()
    );
}

/// `wait_for` の文字列の条件が満たされる側 (D-068、D-088): 子つきのフィクスチャの窓に `"text":"二行目"` を付けると、
/// 1 回目の確認でエディットが満たし、`waited[0].element` の種類がエディットの種類、矩形がエディットの窓の矩形になる。
/// 読むだけ (入力も撮影もしない)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_wait_for_text_is_met_by_fixture_edit() {
    let f = Fixture::open_with_controls("wait-text");
    let edit_rect = f.win.child_rect(FIXTURE_EDIT_ID).expect("エディットの矩形");
    let exe = std::env::current_exe().unwrap();
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    // 題名は部分一致 (フィクスチャの題名は "ai-cu fixture wait-text <pid>")
    let action = format!(
        r#"{{"type":"wait_for","process":{},"title":{},"text":{},"timeout_ms":3000}}"#,
        serde_json::to_string(&exe_name).unwrap(),
        serde_json::to_string("ai-cu fixture wait-text").unwrap(),
        serde_json::to_string("二行目").unwrap()
    );
    let r = f.execute(&action).expect("wait_for (text)");
    assert_eq!(r.executed, 1);
    assert_eq!(r.waited.len(), 1, "{:?}", r.waited);
    assert_eq!(r.waited[0].checks, 1, "{:?}", r.waited);
    let el = r.waited[0].element.as_ref().expect("要素");
    assert!(
        el["type"].as_str().is_some_and(is_fixture_edit_type),
        "{el}"
    );
    assert_eq!(
        el["rect"],
        serde_json::to_value(edit_rect).unwrap(),
        "UIA の矩形がエディットの窓の矩形 {edit_rect} と一致しない: {el}"
    );
    assert!(f.win.events().is_empty(), "{:?}", f.win.events());
    println!("FIXTURE wait_for text element={el}");
}

/// 条件がそろうまで待つ `wait_for` (D-068)。読むだけで、入力も撮影もしない。フィクスチャのウィンドウと要素が
/// 「ある」で `waited` に入り (要素の矩形はフィクスチャの矩形)、無い題名の `absent` は即時に満たされ、
/// 無い題名の present と無い文字列は 15 になる。最後に 1 回の判定 (`find_window` + `ui_elements`) の所要を出す
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_wait_for_sees_fixture_window_and_element_and_times_out_on_absence() {
    let f = Fixture::open("wait");
    let exe = std::env::current_exe().unwrap();
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    // 題名は部分一致 (フィクスチャの題名は "ai-cu fixture wait <pid>")
    let title = "ai-cu fixture wait";
    let wait = |title: &str, extra: &str, timeout_ms: u64| -> String {
        format!(
            r#"{{"type":"wait_for","process":{},"title":{}{extra},"timeout_ms":{timeout_ms}}}"#,
            serde_json::to_string(&exe_name).unwrap(),
            serde_json::to_string(title).unwrap()
        )
    };
    // (1) ウィンドウの条件: 最初の判定で満たされる
    let r = f
        .execute(&wait(title, "", 3000))
        .expect("wait_for (window)");
    assert_eq!(r.executed, 1);
    assert_eq!(r.waited.len(), 1, "{:?}", r.waited);
    assert_eq!(
        r.waited[0].window.as_ref().map(|w| w.hwnd),
        Some(f.win.hwnd()),
        "{:?}",
        r.waited
    );
    assert_eq!(r.waited[0].checks, 1);
    assert!(r.waited[0].element.is_none(), "{:?}", r.waited);
    // (2) 要素の条件: フィクスチャの UIA の要素の名前は題名 (fixture_real_uia_finds_fixture_window_element_in_physical_pixels
    // の出力のとおり)。矩形は DWM の枠の矩形と物理ピクセルで一致し、center が付き、text は無い
    let r = f
        .execute(&wait(
            title,
            r#","element":{"name":"ai-cu fixture wait"}"#,
            3000,
        ))
        .expect("wait_for (element)");
    assert_eq!(r.waited.len(), 1, "{:?}", r.waited);
    let el = r.waited[0].element.as_ref().expect("要素");
    let expected = f.win.window_rect().unwrap();
    assert_eq!(
        el["rect"],
        serde_json::to_value(expected).unwrap(),
        "UIA の矩形がフィクスチャの矩形 {expected} と一致しない: {el}"
    );
    // 枠の無いポップアップは window ではなく pane になりうる
    assert!(
        matches!(el["type"].as_str(), Some("window" | "pane")),
        "{el}"
    );
    assert!(!el["center"].is_null(), "{el}");
    assert!(el.get("text").is_none(), "{el}");
    let element_checks = r.waited[0].checks;
    let element_elapsed_ms = r.waited[0].elapsed_ms;
    // (3) 無い題名の absent: 即時に満たされ、window のキーは無い
    let r = f
        .execute(&wait("no-such-fixture-title", r#","absent":true"#, 300))
        .expect("wait_for (absent)");
    assert_eq!(r.waited.len(), 1, "{:?}", r.waited);
    assert_eq!(r.waited[0].checks, 1);
    assert!(r.waited[0].window.is_none(), "{:?}", r.waited);
    let v = serde_json::to_value(&r).unwrap();
    assert!(v["waited"][0].get("window").is_none(), "{v}");
    assert!(v["waited"][0].get("element").is_none(), "{v}");
    // 最初の確認で満たされた absent (timeout_ms 300) には、windows で確かめるよう促す note が付く (D-070)
    assert!(
        r.waited[0].note.is_some_and(|n| n.contains("windows")),
        "{:?}",
        r.waited
    );
    // (4) 無い題名の present: 15、unmet は window、detail.window は null、elapsed_ms は timeout_ms 以上
    let e = f
        .execute(&wait("no-such-fixture-title", "", 300))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::WaitTimeout, "{e}");
    assert_eq!(e.kind.exit_code(), 15);
    let d = e.detail.as_ref().expect("detail");
    assert_eq!(d["unmet"], "window", "{d}");
    assert!(d["window"].is_null(), "{d}");
    assert_eq!(d["timeout_ms"], 300, "{d}");
    let p = e.progress.as_ref().expect("progress");
    assert_eq!(p["executed"], 0, "{p}");
    assert_eq!(p["partial"], false, "{p}");
    assert!(
        p["elapsed_ms"].as_u64().unwrap() >= 300,
        "時間切れは timeout_ms 以上の経過: {p}"
    );
    // (5) 無い文字列: フィクスチャは文字列を持つ要素が無いので、unmet は element。detail.window にフィクスチャ (題名なし)
    let e = f
        .execute(&wait(title, r#","text":"no-such-text""#, 300))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::WaitTimeout, "{e}");
    let d = e.detail.as_ref().expect("detail");
    assert_eq!(d["unmet"], "element", "{d}");
    assert_eq!(
        d["window"]["hwnd"].as_u64(),
        Some(f.win.hwnd() as u64),
        "{d}"
    );
    assert!(d["window"].get("title").is_none(), "題名は入れない: {d}");
    assert!(
        f.win.events().is_empty(),
        "入力は送らない: {:?}",
        f.win.events()
    );
    // (6) 1 回の判定 (find_window + ui_elements) の所要。主エージェントが読んで design-notes に書くかを決める
    let desktop = Win32Desktop::for_fixture(f.scope());
    let started = Instant::now();
    let w = desktop
        .find_window(&exe_name, Some(title))
        .expect("find_window")
        .expect("フィクスチャ");
    let q = ElementQuery {
        name: Some(title.to_string()),
        limit: 1,
        ..Default::default()
    };
    let found = desktop.ui_elements(w.info.hwnd, &q).expect("ui_elements");
    let one_check_ms = started.elapsed().as_millis();
    assert!(!found.elements.is_empty(), "{found:?}");
    println!(
        "FIXTURE wait_for checks={element_checks} elapsed_ms={element_elapsed_ms} one_check_ms={one_check_ms} (1 回の UIA の判定の所要)"
    );
}

/// 画面が落ち着くまで待つ `wait_stable` と、変わるまで待つ `wait_change` (D-081)。入力は送らず、撮った画像は
/// 保存しない (差の割合だけを見る)。(1) 一色のフィクスチャの内側の region で `wait_stable` が満たされる。
/// (2) 1 回の撮影 + 縮小 + 比較の所要を 10 回測る (フィクスチャが見えている間に測り、奥の画面を撮る回数を増やさない)。
/// (3) 同じ region で `wait_change` を始め、別スレッドが 300 ms 後にフィクスチャを隠すと満たされる。隠した後は
/// フィクスチャの奥の画面 (別のアプリの中身) を撮るが保存しない。奥がたまたまフィクスチャと同じ一色なら
/// 満たされず 15 になる (そのときは机の状態を変えて打ち直す)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_wait_stable_and_change_on_fixture() {
    let f = Fixture::open("screen-wait");
    let inner = Rect::new(
        f.client.x + 10,
        f.client.y + 10,
        f.client.w - 20,
        f.client.h - 20,
    );
    let region = format!(
        r#""region":{{"x":{},"y":{},"w":{},"h":{}}}"#,
        inner.x, inner.y, inner.w, inner.h
    );
    // (1) 動かない画面: stable_ms (既定 300) の静けさで満たされる
    let r = f
        .execute(&format!(
            r#"{{"type":"wait_stable",{region},"timeout_ms":3000}}"#
        ))
        .expect("wait_stable");
    assert_eq!(r.executed, 1);
    assert_eq!(r.waited.len(), 1, "{:?}", r.waited);
    let w = &r.waited[0];
    assert_eq!(w.rect, Some(inner), "{w:?}");
    assert!(w.changed_ratio.is_none(), "{w:?}");
    assert!(w.window.is_none(), "{w:?}");
    assert!(w.elapsed_ms >= 300, "静けさは stable_ms 以上: {w:?}");
    assert!(w.checks >= 2, "比べるには 2 回以上撮る: {w:?}");
    assert!(
        r.screenshots.is_empty(),
        "画像は保存しない: {:?}",
        r.screenshots
    );
    let (stable_checks, stable_ms) = (w.checks, w.elapsed_ms);
    // (2) 1 回の撮影 (実の装置の capture) + thumb + frame_diff の所要。主エージェントが読んで design-notes に足す
    let desktop = Win32Desktop::for_fixture(f.scope());
    let base = thumb(&desktop.capture(inner).expect("撮影"));
    let mut costs = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        let small = thumb(&desktop.capture(inner).expect("撮影"));
        // 測るのは所要だけ (通知などが重なっても落とさない。落ち着きは (1) で確かめた)
        std::hint::black_box(frame_diff(&base, &small));
        costs.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let one_frame_ms_max = costs.iter().cloned().fold(0.0, f64::max);
    let one_frame_ms_avg = costs.iter().sum::<f64>() / costs.len() as f64;
    // (3) 変わるのを待つ間に、別スレッドが 300 ms 後にフィクスチャを隠す
    // 時刻はどれもテストの時計 (Instant)。hide の時刻は呼ぶ直前 (隠れる変化はこれより前に起きない)
    let (r, before_wait, done, hide_called) = std::thread::scope(|s| {
        let win = &f.win;
        let hider = s.spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let called = Instant::now();
            win.hide();
            called
        });
        let before_wait = Instant::now();
        let r = f.execute(&format!(
            r#"{{"type":"wait_change",{region},"timeout_ms":3000}}"#
        ));
        let done = Instant::now();
        let called = hider.join().expect("隠すスレッド");
        (r, before_wait, done, called)
    });
    assert!(!f.win.is_visible(), "フィクスチャが隠れていない");
    let r = r.expect("wait_change (フィクスチャを隠した後に満たされる)");
    assert_eq!(r.waited.len(), 1, "{:?}", r.waited);
    let w = &r.waited[0];
    assert_eq!(w.rect, Some(inner), "{w:?}");
    // 満たされた wait_change は、基準 (1 回目) と変わったフレームで必ず 2 回以上撮っている。3 回以上なら、
    // 基準と比べて変わらなかったフレームが隠す前に 1 回以上ある (間隔は 100 ms + 撮影、hide は 300 ms 後)
    assert!(
        w.checks >= 3,
        "隠す前に基準と 1 回以上比べて、変わらなかった: {w:?}"
    );
    // 差の大きさは閾値を超えたこと (満たされたこと) と同じなので見ない。奥の画面しだいで値は揺れる
    let ratio = w.changed_ratio.expect("changed_ratio");
    // 満たされたのは隠した後: 待ち始めの直前から戻るまでの時間は、待ち始めの直前から hide を呼ぶまでの時間以上。
    // 起動から待ち始めまでの差 (validate、入力デスクトップの判定) で揺れないよう、どちらもテストの時計で比べる
    let hide_at = hide_called.duration_since(before_wait);
    let change_done = done.duration_since(before_wait);
    assert!(
        change_done >= hide_at,
        "hide より前に満たされた: 戻り {change_done:?} < hide {hide_at:?} ({w:?})"
    );
    assert!(
        u128::from(w.elapsed_ms) <= change_done.as_millis() + 1,
        "elapsed_ms は待ち始めの直前から戻るまでに収まる: {w:?} {change_done:?}"
    );
    assert!(
        r.screenshots.is_empty(),
        "画像は保存しない: {:?}",
        r.screenshots
    );
    assert!(
        f.win.events().is_empty(),
        "入力は送らない: {:?}",
        f.win.events()
    );
    println!(
        "FIXTURE wait_screen stable_checks={stable_checks} stable_ms={stable_ms} change_ms={} one_frame_ms_max={one_frame_ms_max:.1} one_frame_ms_avg={one_frame_ms_avg:.1} (region {}x{}、debug ビルド。change_checks={} changed_ratio={ratio} hide_at_ms={})",
        w.elapsed_ms,
        inner.w,
        inner.h,
        w.checks,
        hide_at.as_millis()
    );
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_scope_rejects_outside_before_sending() {
    let f = Fixture::open("scope");
    let before = input::cursor_pos().unwrap();
    let outside = Point::new(f.client.x + f.client.w as i32 + 50, f.client.y + 10);
    let mut sink = RealSink::for_fixture(f.scope());
    let r = sink.send(&[InputEvent::MoveTo {
        x: outside.x,
        y: outside.y,
    }]);
    assert!(matches!(r, Err(BackendError::OutOfScope(_))), "{r:?}");
    assert_eq!(
        input::cursor_pos().unwrap(),
        before,
        "範囲外の移動は送られていない"
    );
    // 方針でも拒否される (実行器は送る前に止まる)
    let e = f
        .execute(&format!(
            r#"{{"type":"click","x":{},"y":{}}}"#,
            outside.x, outside.y
        ))
        .unwrap_err();
    println!("FIXTURE scope rejected: {e}");
}

/// ロックしていない今の画面で、入力デスクトップが使える側を返し、WTS のロックが非ロックと読め、
/// 押されたままのキーもボタンも無い (D-066、D-067)。
/// 読むだけで、入力は送らない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_input_desktop_is_available_and_no_keys_held() {
    let desktop = Win32Desktop::new();
    assert_eq!(
        desktop.input_desktop().expect("入力デスクトップを読めない"),
        InputDesktop::Available,
        "ロックもセーバーも無い画面で走らせる"
    );
    // WTS のセッションのロック (D-067) が、ロックしていない画面で「非ロック」と読める
    assert_eq!(
        ai_cu::win32::desktop::input_desktop().session_locked,
        Some(false),
        "WTS の SessionFlags がロックしていない画面で UNLOCK を返す"
    );
    assert_eq!(
        desktop.keys_held().expect("keys_held"),
        HeldInputs::default(),
        "確認の間はキーボードとマウスから手を離す"
    );
    println!("FIXTURE input desktop available, no keys held");
}

/// 注入した Shift を、どの経路で抜けても離す (失敗の経路でも押したままにしない)
struct ShiftUp(FixtureScope);

impl Drop for ShiftUp {
    fn drop(&mut self) {
        // 離す送信の失敗 (画面の読みの失敗など) と、UIPI に黙って捨てられた離し (SendInput の戻り値にも出ない) は、
        // 読み直して残っていれば人に知らせる。Shift が押されたままだと、以後の人の操作が狂う
        if let Err(e) = RealSink::for_fixture(self.0).send(&[InputEvent::Key {
            vk: 0x10,
            down: false,
            extended: false,
        }]) {
            eprintln!("FIXTURE Shift を離す送信が失敗: {e}");
        }
        std::thread::sleep(Duration::from_millis(50));
        let still = Win32Desktop::for_fixture(self.0)
            .keys_held()
            .map(|h| h.keys.contains(&"shift"))
            .unwrap_or(true);
        if still {
            eprintln!(
                "FIXTURE Shift が押されたままの可能性がある。手で Shift を 1 回押して離してください"
            );
        }
    }
}

/// 注入した Shift の押下も非同期のキーの状態に載る (Microsoft Learn の GetAsyncKeyState) ので、人が押したままの
/// キーの代わりに使う (D-066)。フィクスチャを前面にしてから Shift を押し、`keys_held` が `shift` を返すことと、
/// 実行器がフィクスチャへの click を 10 `keys_held` で断り、クリックが 1 つも届かないことを確かめ、最後に必ず離す
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_injected_shift_is_seen_by_keys_held_and_run_refuses() {
    let f = Fixture::open("keys-held");
    f.activate();
    let _up = ShiftUp(f.scope());
    RealSink::for_fixture(f.scope())
        .send(&[InputEvent::Key {
            vk: 0x10,
            down: true,
            extended: false,
        }])
        .expect("フィクスチャが前面のときだけ Shift を押す");
    std::thread::sleep(Duration::from_millis(100));
    let held = Win32Desktop::for_fixture(f.scope())
        .keys_held()
        .expect("keys_held");
    assert!(held.keys.contains(&"shift"), "{held:?}");
    f.win.clear_events();
    let c = f.client.center();
    let e = f
        .execute(&format!(r#"{{"type":"click","x":{},"y":{}}}"#, c.x, c.y))
        .unwrap_err();
    assert_eq!(e.kind.exit_code(), 10, "{e}");
    assert_eq!(e.reason, Some("keys_held"));
    assert!(e.progress.is_none(), "何も送る前の失敗: {e}");
    std::thread::sleep(Duration::from_millis(200));
    let ev = f.win.events();
    assert!(
        !ev.iter().any(|x| matches!(
            x,
            FixtureEvent::ButtonDown { .. }
                | FixtureEvent::ButtonUp { .. }
                | FixtureEvent::DoubleClick { .. }
        )),
        "クリックは届いていない: {ev:?}"
    );
    println!("FIXTURE keys_held saw {held:?}, run refused with 10 keys_held");
}

// ---- 操作の前の枠 (D-093、plan 2026-10-28-activity-overlay のステップ 2) ----
//
// 枠の窓 (`src/win32/overlay.rs`) の未確認の前提 (前面を取らない、ヒットテストを素通りする、撮影に写らない、ai-cu の
// 一覧と覆いの判定に出ない、自分の枠の見せる・隠すが `overlay_windows` に映る) を実機で確かめる。入力は送らない
// (フィクスチャの前面化で `activate` がフィクスチャの中をクリックすることはある)。枠は自分のプロセスのものだけを出し、
// テストの終わりに `OverlayThread` の Drop で壊す。ほかのテストのフィクスチャは開いている間ずっと枠を出す (D-094)。
// 自分の枠を数えるテストは枠なしで開き、出す前に `assert_no_own_overlay_shown` で確かめる。
// 予告の間のテスト (D-095) は実行器に `RealIndicator` を渡し、自分の枠が見えてから別スレッドで入力を送る。

/// 枠を見せる期限 (テストの間だけ。ウォッチドッグより短い)
const OVERLAY_SHOW_MS: u64 = 20_000;

/// このプロセスの枠の窓
fn own_overlays() -> Vec<OverlayWindow> {
    overlay_windows(Some(std::process::id()))
}

/// このプロセスの枠のうち、見えているものが `visible` の向き (true: 1 つ以上ある、false: 1 つも無い) になるまで
/// 待つ。なれば、なるまでの経過を返す
fn wait_own_overlay(visible: bool, limit: Duration) -> Option<Duration> {
    let start = Instant::now();
    loop {
        let any = own_overlays().iter().any(|w| w.visible);
        if any == visible {
            return Some(start.elapsed());
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 出す前に自分の枠が 1 つも見えていないことを確かめる (フィクスチャの枠や前のテストの枠の残りを検出する。D-094)
fn assert_no_own_overlay_shown() {
    assert!(
        own_overlays().iter().all(|w| !w.visible),
        "出す前は自分の枠が見えていない (フィクスチャの枠か前のテストの枠が残っている): {:?}",
        own_overlays()
    );
}

/// 枠のスレッドを立てて見せ、見えるまで待つ
fn show_overlay() -> OverlayThread {
    assert_no_own_overlay_shown();
    let ov = OverlayThread::start().expect("枠のスレッドを立てられない");
    ov.show(OVERLAY_SHOW_MS);
    assert!(
        wait_own_overlay(true, Duration::from_secs(2)).is_some(),
        "show の後 2 秒以内に枠が見えない: {:?}",
        own_overlays()
    );
    // 帯と札の窓がそろって描かれるまで少し待つ
    std::thread::sleep(Duration::from_millis(200));
    ov
}

/// このプロセスの見えている枠の窓の矩形 (重なりの一覧の読み方で)
fn own_overlay_rects() -> Vec<Rect> {
    own_overlays()
        .into_iter()
        .filter(|w| w.visible)
        .filter_map(|w| window::stack_window(w.hwnd))
        .map(|w| w.rect)
        .collect()
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_overlay_does_not_take_foreground() {
    let f = Fixture::open_without_frame("overlay-foreground");
    f.activate();
    let before = window::foreground_hwnd();
    assert_eq!(before, f.win.hwnd(), "フィクスチャが前面");
    let ov = show_overlay();
    std::thread::sleep(Duration::from_millis(300));
    let after = window::foreground_hwnd();
    let shown = own_overlays().iter().filter(|w| w.visible).count();
    ov.release(0);
    println!(
        "FIXTURE overlay foreground before={before:#x} after={after:#x} shown_windows={shown}"
    );
    assert_eq!(after, before, "枠を見せても前面は変わらない");
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_overlay_is_transparent_to_hit_test() {
    let f = Fixture::open_at_corner("overlay-hit");
    let primary = screen::primary_screen().expect("主モニタ");
    let frame = f.win.window_rect().expect("フィクスチャの矩形");
    assert_eq!(
        (frame.x, frame.y),
        (primary.x, primary.y),
        "フィクスチャが主モニタの左上の角にある"
    );
    let ov = show_overlay();
    let rects = own_overlay_rects();
    let hwnd = f.win.hwnd();
    // 上の帯、左の帯、左上の角 (どの帯の太さでも帯の中に入るよう、クライアント領域の縁から 1 ピクセル内側)
    let points = [f.at(100, 1), f.at(1, 100), f.at(1, 1)];
    let mut seen = Vec::new();
    for p in points {
        assert!(
            rects.iter().any(|r| r.contains(p)),
            "点 {p:?} が枠の帯の上にない: {rects:?}"
        );
        seen.push((p, window::root_at(p), window::at_point(p).map(|w| w.hwnd)));
    }
    ov.release(0);
    println!(
        "FIXTURE overlay hit_test fixture={hwnd:#x} overlay_windows={} points={:?}",
        rects.len(),
        seen.iter()
            .map(|(p, root, at)| format!("({},{}) root={root:#x} at={at:#x?}", p.x, p.y))
            .collect::<Vec<_>>()
    );
    for (p, root, at) in seen {
        assert_eq!(root, hwnd, "root_at({p:?}) はフィクスチャ (枠を素通りする)");
        assert_eq!(at, Some(hwnd), "at_point({p:?}) はフィクスチャ");
    }
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_overlay_not_in_capture() {
    let f = Fixture::open_at_corner("overlay-shot");
    let ov = show_overlay();
    let rects = own_overlay_rects();
    // クライアント領域の上の 4 行と左の 4 列 (帯の太さは 96 DPI で 6 ピクセル以上なので、帯の中)
    let c = f.client;
    let strips = [Rect::new(c.x, c.y, c.w, 4), Rect::new(c.x, c.y, 4, c.h)];
    let desktop = Win32Desktop::for_fixture(f.scope());
    let (r, g, b) = FIXTURE_RGB;
    let mut results = Vec::new();
    for strip in strips {
        assert!(
            rects.iter().any(|o| o.intersects(&strip)),
            "撮る矩形 {strip} が枠の帯に重ならない: {rects:?}"
        );
        let img = desktop.capture(strip).expect("撮影");
        let matching = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] == r && p[1] == g && p[2] == b)
            .count();
        let total = (img.width * img.height) as usize;
        let first = img.rgba.get(..4).map(|p| p.to_vec());
        results.push((strip, matching, total, first));
    }
    let still_shown = own_overlays().iter().any(|w| w.visible);
    ov.release(0);
    println!(
        "FIXTURE overlay capture shown={still_shown} strips={:?}",
        results
            .iter()
            .map(|(s, m, t, first)| format!("{s} match={m}/{t} first={first:?}"))
            .collect::<Vec<_>>()
    );
    assert!(still_shown, "撮る間も枠が見えていた");
    for (strip, matching, total, _) in results {
        assert!(
            matching * 100 >= total * 99,
            "帯の位置にフィクスチャの色でない画素がある (枠が写った): {strip} {matching}/{total}"
        );
    }
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_overlay_window_lists() {
    // 読み取りだけ (列挙と問い合わせ)。入力も撮影もしない
    let _watchdog = Watchdog::start("overlay-lists");
    dpi::ensure_per_monitor_v2().expect("Per-Monitor V2 を宣言できない");
    let ov = show_overlay();
    let own: Vec<usize> = own_overlays()
        .into_iter()
        .filter(|w| w.visible)
        .map(|w| w.hwnd)
        .collect();
    let listed = window::top_level_windows_detailed();
    let in_list: Vec<usize> = listed
        .iter()
        .filter(|w| w.info.class == OVERLAY_CLASS || own.contains(&w.info.hwnd))
        .map(|w| w.info.hwnd)
        .collect();
    let stack = window::stack_windows();
    let in_stack: Vec<(usize, u32, bool)> = stack
        .iter()
        .filter(|w| w.info.class == OVERLAY_CLASS || own.contains(&w.info.hwnd))
        .map(|w| (w.info.hwnd, w.info.pid, w.see_through))
        .collect();
    ov.release(0);
    let own_in_stack = in_stack.iter().filter(|(h, _, _)| own.contains(h)).count();
    println!(
        "FIXTURE overlay lists own_shown={} top_windows={} stack={} own_in_stack={own_in_stack} see_through_all={}",
        own.len(),
        in_list.len(),
        in_stack.len(),
        in_stack.iter().all(|(_, _, s)| *s)
    );
    assert!(!own.is_empty(), "枠が見えていた");
    assert!(
        in_list.is_empty(),
        "windows の一覧に枠の窓が出ない: {in_list:x?}"
    );
    assert_eq!(
        own_in_stack,
        own.len(),
        "見えている自分の枠の窓は重なりの一覧に入る (入ったうえで覆いに数えない): {in_stack:x?}"
    );
    assert!(
        in_stack.iter().all(|(_, _, s)| *s),
        "重なりの一覧の枠の窓は覆いとみなさない (see_through): {in_stack:x?}"
    );
}

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_overlay_own_windows_track_show_and_release() {
    // 読み取りと自分の枠の表示だけ。入力も撮影もしない
    let _watchdog = Watchdog::start("overlay-track");
    dpi::ensure_per_monitor_v2().expect("Per-Monitor V2 を宣言できない");
    let me = std::process::id();
    let others = overlay_windows(None)
        .iter()
        .filter(|w| w.pid != me && w.visible)
        .count();
    assert_no_own_overlay_shown();
    let ov = OverlayThread::start().expect("枠のスレッドを立てられない");
    let created = own_overlays();
    assert!(
        created.iter().all(|w| !w.visible),
        "start は窓を作るだけで見せない: {created:?}"
    );
    // (1) show で見え、見せ始めの時刻を持つ
    ov.show(OVERLAY_SHOW_MS);
    let shown_in = wait_own_overlay(true, Duration::from_secs(2)).expect("show の後に見える");
    std::thread::sleep(Duration::from_millis(100));
    let shown = own_overlays();
    let visible: Vec<&OverlayWindow> = shown.iter().filter(|w| w.visible).collect();
    assert!(
        visible.iter().all(|w| w.since_tick.is_some()),
        "見えている枠は見せ始めの時刻を持つ: {shown:?}"
    );
    // (2) release(0) の後 500 ms 以内に見えなくなり、時刻のプロパティも消える
    ov.release(0);
    let hidden_in = wait_own_overlay(false, Duration::from_millis(500));
    assert!(
        hidden_in.is_some(),
        "release(0) の後 500 ms 以内に見えなくなる: {:?}",
        own_overlays()
    );
    // hide は窓ごとに SW_HIDE してから RemovePropW するので、最後の窓が隠れた直後は
    // プロパティがまだ残っていることがある。消えるまで短く待つ
    let cleared = wait_until(Duration::from_millis(100), || {
        own_overlays().iter().all(|w| w.since_tick.is_none())
    });
    assert!(
        cleared,
        "隠した枠は見せ始めの時刻を持たない: {:?}",
        own_overlays()
    );
    // (3) もう一度 show して release(300): 100 ms 後はまだ見え、200 ms より後かつ 800 ms 以内に見えなくなる。
    // release は命令を積んで起こすだけで戻るので、枠のスレッドが処理し終えた後 (100 ms 後) に読む
    ov.show(OVERLAY_SHOW_MS);
    wait_own_overlay(true, Duration::from_secs(2)).expect("2 回目の show の後に見える");
    let released_at = Instant::now();
    ov.release(300);
    std::thread::sleep(Duration::from_millis(100));
    let still = own_overlays().iter().any(|w| w.visible);
    let still_checked_in = released_at.elapsed();
    let hidden = wait_own_overlay(false, Duration::from_millis(800));
    let lingered_for = hidden.map(|_| released_at.elapsed());
    println!(
        "FIXTURE overlay track windows={} shown_in_ms={} hidden_in_ms={:?} linger300_still={still} (checked in {} ms) linger300_hidden_in_ms={:?} other_pid_shown={others}",
        created.len(),
        shown_in.as_millis(),
        hidden_in.map(|d| d.as_millis()),
        still_checked_in.as_millis(),
        lingered_for.map(|d| d.as_millis())
    );
    assert!(
        still_checked_in < Duration::from_millis(200),
        "release(300) の後の確かめが 200 ms を超えた (残す時間の内側で読めていない): {still_checked_in:?}"
    );
    assert!(still, "release(300) の 100 ms 後はまだ見えている");
    let lingered_for = lingered_for.unwrap_or_else(|| {
        panic!(
            "release(300) の後 800 ms 以内に見えなくなる: {:?}",
            own_overlays()
        )
    });
    assert!(
        lingered_for >= Duration::from_millis(200),
        "release(300) は残す時間を置いてから隠す (200 ms より前に消えた): {lingered_for:?}"
    );
}

/// 延長 (`extend`) は見えていて解かれた枠だけを残し、見えていない枠は出さない (D-107)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_overlay_extend_keeps_released_frame() {
    // 読み取りと自分の枠の表示だけ。入力も撮影もしない
    let _watchdog = Watchdog::start("overlay-extend");
    dpi::ensure_per_monitor_v2().expect("Per-Monitor V2 を宣言できない");
    assert_no_own_overlay_shown();
    let ov = OverlayThread::start().expect("枠のスレッドを立てられない");
    // (1) 見せていない枠への延長は新しく出さない (立てたばかりのスレッドは active が偽なので、この区間が
    // 見分けるのは extend が active を立ててしまう退行だけ。延長の実装の残りは (2) と (3) が撃つ)
    ov.extend(5000);
    std::thread::sleep(Duration::from_millis(300));
    let not_shown_stays_hidden = own_overlays().iter().all(|w| !w.visible);
    // 断言はすべて計測の行 (println) の後に置く。どれで落ちても経過の値が出力に残る
    // (2) release(1000) の 100 ms 後に extend(3000): 1000 ms の残す時間を越えて残り、延長の後に切れる
    ov.show(OVERLAY_SHOW_MS);
    wait_own_overlay(true, Duration::from_secs(2)).expect("show の後に見える");
    let released_at = Instant::now();
    ov.release(1000);
    std::thread::sleep(Duration::from_millis(100));
    ov.extend(3000);
    let extended_after = released_at.elapsed();
    std::thread::sleep(Duration::from_millis(1500).saturating_sub(released_at.elapsed()));
    let still = own_overlays().iter().any(|w| w.visible);
    let still_at = released_at.elapsed();
    let hidden = wait_own_overlay(
        false,
        Duration::from_millis(4500).saturating_sub(released_at.elapsed()),
    );
    let hidden_in = hidden.map(|_| released_at.elapsed());
    // (3) release(0) で見えなくなった後の延長で戻らない (止められた実行の後)
    ov.show(OVERLAY_SHOW_MS);
    wait_own_overlay(true, Duration::from_secs(2)).expect("3 回目の show の後に見える");
    ov.release(0);
    wait_own_overlay(false, Duration::from_millis(500)).expect("release(0) の後に見えなくなる");
    ov.extend(5000);
    std::thread::sleep(Duration::from_millis(300));
    let stays_hidden_after_release0 = own_overlays().iter().all(|w| !w.visible);
    println!(
        "FIXTURE overlay extend not_shown_stays_hidden={not_shown_stays_hidden} extended_after_ms={} still_at_ms={} hidden_in_ms={:?} release0_stays_hidden={stays_hidden_after_release0}",
        extended_after.as_millis(),
        still_at.as_millis(),
        hidden_in.map(|d| d.as_millis())
    );
    assert!(not_shown_stays_hidden, "見せていない枠は延長で見えない (1)");
    // 重い検査の最中に遅れて、延長が切れた枠に届く誤った落ち方を見分ける
    assert!(
        extended_after < Duration::from_millis(500),
        "release の後 extend を送るまでが 500 ms を超えた (送るのが遅れ、残す時間の内側に届いた保証が無い): {extended_after:?}"
    );
    assert!(
        still_at < Duration::from_millis(2500),
        "release から 1500 ms の確かめが 2500 ms を超えた (延長の内側で読めていない): {still_at:?}"
    );
    assert!(
        still,
        "延長した枠は release(1000) の残す時間を越えて見えている (release から {still_at:?})"
    );
    assert!(
        hidden_in.is_some(),
        "延長の後、release から 4500 ms 以内に見えなくなる: {:?}",
        own_overlays()
    );
    // 延長は送った時刻 (release から 100 ms 以上後) から 3000 ms なので、終わりは release から 3100 ms 以上後。
    // GetTickCount64 の粒度とタイマーのずれを見込んで 2900 ms を下限にする (延長が linger を無視して早く切れる退行を捕まえる)
    assert!(
        hidden_in.is_some_and(|d| d >= Duration::from_millis(2900)),
        "延長した枠は release から 2900 ms より前に消えない: {hidden_in:?}"
    );
    assert!(
        stays_hidden_after_release0,
        "release(0) の後の延長で枠が戻らない: {:?}",
        own_overlays()
    );
    // 後のテストに残さない
    assert_no_own_overlay_shown();
    drop(ov);
}

/// フィクスチャを開いている間は自分の枠が見え、閉じると消える (D-094)。入力は送らない
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_fixture_frame_follows_open_and_drop() {
    assert_no_own_overlay_shown();
    let f = Fixture::open("frame");
    let shown_in = wait_own_overlay(true, Duration::from_secs(2));
    assert!(
        shown_in.is_some(),
        "フィクスチャを開いた後 2 秒以内に自分の枠が見えない: {:?}",
        own_overlays()
    );
    drop(f);
    let hidden_in = wait_own_overlay(false, Duration::from_secs(1));
    println!(
        "FIXTURE frame shown_in_ms={:?} hidden_in_ms={:?}",
        shown_in.map(|d| d.as_millis()),
        hidden_in.map(|d| d.as_millis())
    );
    assert!(
        hidden_in.is_some(),
        "フィクスチャを閉じた後 1 秒以内に自分の枠が消えない: {:?}",
        own_overlays()
    );
}

// ---- 予告の間の人の入力 (D-093、D-095) ----
//
// 予告の間に人がマウスかキーに触れると、何も送らずに 6 / 10 `lead_input` で止まり、枠がすぐ消えることを確かめる
// (手順 29 の (b) の置き換え)。人の手の代わりに、別スレッドからフィクスチャの中へ `SendInput` で入力を送る。
// 実行器には `RealIndicator` を渡し、フィクスチャは枠なしで開く (フィクスチャの枠が出ていると予告の間が縮む)。

/// 予告の間の長さ (設定の `indicator.lead_ms`)。方針・断言・時刻はこれから導く
const LEAD_MS: u64 = 1500;
/// 自分の枠が見えてから入力を送るまで (猶予の 400 ms 後。予告の終わりの 800 ms 前)
const LEAD_SEND_AFTER_MS: u64 = run::LEAD_INPUT_GRACE_MS + 400;
/// 実行の後に枠を残す時間 (止められた実行はすぐ消え、ふつうの終わりは残るのを見分ける)
const LEAD_LINGER_MS: u64 = 5000;

/// 見えている ai-cu の枠 (どのプロセスでも) の pid
fn visible_overlay_pids() -> Vec<u32> {
    overlay_windows(None)
        .into_iter()
        .filter(|w| w.visible)
        .map(|w| w.pid)
        .collect()
}

/// 見えている枠があれば落とす (黙って飛ばさない。他の枠が見えていると予告の間が縮む)
fn assert_no_overlay_shown_anywhere() {
    let pids = visible_overlay_pids();
    if !pids.is_empty() {
        panic!("他の ai-cu の枠が出ている。消えてからやり直す: pid={pids:?}");
    }
}

/// 予告の間のテストの下ごしらえ: (0) 始まりの知らせ (D-094) を済ませ (出すかは `start_frame` の判定に任せ、
/// 返った枠はすぐ消す)、(1) 見えている枠が無くなるまで 7 秒まで待ち、(2) 自分の枠が見えていないことを確かめる
fn lead_preflight() {
    dpi::ensure_per_monitor_v2().expect("Per-Monitor V2 を宣言できない");
    drop(start_frame(Frame::Show));
    let _ = wait_own_overlay(false, Duration::from_secs(1));
    let _ = wait_until(Duration::from_secs(7), || visible_overlay_pids().is_empty());
    assert_no_overlay_shown_anywhere();
    assert_no_own_overlay_shown();
}

/// 入力を送るスレッドの結果
struct LeadInput {
    /// 自分の枠が見えた (見えなければ送らない)
    seen: bool,
    /// 送った結果と、送り終えた時刻
    sent: Option<(Result<(), BackendError>, Instant)>,
}

/// 自分の枠が見えるまで 2 秒まで待ち、見えてから `LEAD_SEND_AFTER_MS` 後に `events` をフィクスチャへ送るスレッド
fn spawn_lead_input(
    scope: FixtureScope,
    events: Vec<InputEvent>,
) -> std::thread::JoinHandle<LeadInput> {
    std::thread::spawn(move || {
        if wait_own_overlay(true, Duration::from_secs(2)).is_none() {
            return LeadInput {
                seen: false,
                sent: None,
            };
        }
        std::thread::sleep(Duration::from_millis(LEAD_SEND_AFTER_MS));
        let r = RealSink::for_fixture(scope).send(&events);
        LeadInput {
            seen: true,
            sent: Some((r, Instant::now())),
        }
    })
}

/// 予告の間を撃った 1 回の結果
struct LeadOutcome {
    error: AppError,
    /// 実行を呼んでから返るまで
    returned: Duration,
    /// 入力を送り終えてから返るまで (送れていなければ None)
    sent_to_return: Option<Duration>,
    /// 返ってから自分の枠が見えなくなるまで (500 ms 以内に消えなければ None)
    hidden_in: Option<Duration>,
    input: LeadInput,
    /// 実行の直前に読んだ `overlay_shown_for_ms()`
    other_shown_ms: Option<u64>,
    /// 返った 200 ms 後のフィクスチャの記録
    events: Vec<FixtureEvent>,
}

/// 入力のスレッドを立て、直前に他の枠を確かめてから、`RealIndicator` でフィクスチャの中心の click を実行する。
/// 実行は失敗するはず (成功すれば落とす)
fn run_lead(f: &Fixture, events: Vec<InputEvent>) -> LeadOutcome {
    let policy = fixture_policy(f.client, |c| c.indicator.lead_ms = LEAD_MS);
    let c = f.client.center();
    assert_no_overlay_shown_anywhere();
    let other_shown_ms = overlay_shown_for_ms();
    let input = spawn_lead_input(f.scope(), events);
    // `ind` は枠が消えたかを読み終えるまで drop しない (Drop が窓を壊すと、消えたかの断言が常に通る)
    let mut ind = RealIndicator::new();
    let start = Instant::now();
    let r = f.execute_lead(
        &policy,
        &format!(r#"{{"type":"click","x":{},"y":{}}}"#, c.x, c.y),
        &mut ind,
        LEAD_LINGER_MS,
    );
    let returned_at = Instant::now();
    let returned = returned_at - start;
    let hidden_in = wait_own_overlay(false, Duration::from_millis(500));
    drop(ind);
    let input = input.join().expect("入力のスレッド");
    let sent_to_return = input
        .sent
        .as_ref()
        .map(|(_, at)| returned_at.saturating_duration_since(*at));
    std::thread::sleep(Duration::from_millis(200));
    let events = f.win.events();
    let error = match r {
        Ok(rep) => panic!("予告の間で止まらず実行した: {rep:?}"),
        Err(e) => e,
    };
    LeadOutcome {
        error,
        returned,
        sent_to_return,
        hidden_in,
        input,
        other_shown_ms,
        events,
    }
}

/// 両方のテストに共通の断言: 何も送る前の失敗で、入力のスレッドが枠を見て送れ、返った後すぐ枠が消え、
/// フィクスチャにボタンが届いていない
fn assert_lead_common(o: &LeadOutcome) {
    let e = &o.error;
    assert!(e.progress.is_none(), "何も送る前の失敗: {e}");
    assert_eq!(e.action_index, None, "{e}");
    assert!(e.to_json().get("executed").is_none(), "{}", e.to_json());
    assert!(o.input.seen, "予告の間に自分の枠が見えなかった");
    match &o.input.sent {
        Some((Ok(()), _)) => {}
        other => panic!(
            "枠を見て入力を送れなかった: {:?}",
            other.as_ref().map(|(r, _)| r)
        ),
    }
    assert!(
        o.hidden_in.is_some(),
        "返った後 500 ms 以内に自分の枠が消えない: {:?}",
        own_overlays()
    );
    assert!(
        !o.events.iter().any(|x| matches!(
            x,
            FixtureEvent::ButtonDown { .. }
                | FixtureEvent::ButtonUp { .. }
                | FixtureEvent::DoubleClick { .. }
        )),
        "クリックは届いていない: {:?}",
        o.events
    );
}

/// `FIXTURE lead ...` の 1 行 (重い検査のログから値を拾う)
fn print_lead(what: &str, o: &LeadOutcome, idle_ms: Option<u64>) {
    let opt = |v: Option<u128>| v.map_or("-".to_string(), |v| v.to_string());
    println!(
        "FIXTURE lead {what} exit={} returned_ms={} sent_to_return_ms={} idle_ms={} hidden_in_ms={} seen_overlay={} other_shown_ms={}",
        o.error.kind.exit_code(),
        o.returned.as_millis(),
        opt(o.sent_to_return.map(|d| d.as_millis())),
        opt(idle_ms.map(u128::from)),
        opt(o.hidden_in.map(|d| d.as_millis())),
        o.input.seen,
        opt(o.other_shown_ms.map(u128::from)),
    );
}

/// 予告の間にフィクスチャの上でマウスを 200 px 動かすと、何も送らずに 6 で止まり、枠がすぐ消える (手順 29 の (b1))
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_lead_stops_on_injected_mouse_move() {
    lead_preflight();
    let f = Fixture::open_without_frame("lead-mouse");
    let c = f.client.center();
    RealSink::for_fixture(f.scope())
        .send(&[InputEvent::MoveTo {
            x: c.x - 100,
            y: c.y,
        }])
        .expect("カーソルをフィクスチャの中に置く");
    std::thread::sleep(Duration::from_millis(200));
    f.win.clear_events();
    let o = run_lead(
        &f,
        vec![InputEvent::MoveTo {
            x: c.x + 100,
            y: c.y,
        }],
    );
    print_lead("mouse", &o, None);
    assert_lead_common(&o);
    let e = &o.error;
    assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
    assert_eq!(e.kind.exit_code(), 6, "{e}");
    assert!(e.message.contains("マウス"), "{e}");
    assert!(
        o.returned < Duration::from_millis(LEAD_MS),
        "マウスはその場で止まる (予告の間より短い): {:?}",
        o.returned
    );
    assert!(
        o.returned >= Duration::from_millis(LEAD_SEND_AFTER_MS),
        "送る前の別の原因で止まった: {:?}",
        o.returned
    );
}

/// 予告の間にフィクスチャを前面にして Shift を 1 打すると、何も送らずに 10 `lead_input` で止まり、枠がすぐ消える
/// (手順 29 の (b2))。無操作の長さが注入した Shift から来たことも確かめる (`SendInput` が `GetLastInputInfo` を
/// 更新する実測)
#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_lead_stops_on_injected_key_with_lead_input() {
    lead_preflight();
    let f = Fixture::open_without_frame("lead-key");
    f.activate();
    let _up = ShiftUp(f.scope());
    let shift = |down| InputEvent::Key {
        vk: 0x10,
        down,
        extended: false,
    };
    let o = run_lead(&f, vec![shift(true), shift(false)]);
    let e = &o.error;
    let detail = e.detail.as_deref().cloned().unwrap_or_default();
    let idle_ms = detail["idle_ms"].as_u64();
    print_lead("key", &o, idle_ms);
    assert_lead_common(&o);
    assert_eq!(e.kind.exit_code(), 10, "{e}");
    assert_eq!(e.reason, Some("lead_input"), "{e}");
    assert_eq!(detail["lead_ms"].as_u64(), Some(LEAD_MS), "{detail}");
    assert_eq!(e.hint.as_deref(), Some(LEAD_USER_ACTIVE_HINT), "{e}");
    assert!(
        o.returned >= Duration::from_millis(LEAD_MS - 100),
        "キーは予告の間の終わりに判定する: {:?}",
        o.returned
    );
    let idle = i128::from(idle_ms.unwrap_or_else(|| panic!("detail に idle_ms が無い: {detail}")));
    let s2r = o.sent_to_return.expect("送れている").as_millis() as i128;
    assert!(
        s2r - 200 <= idle && idle <= s2r + 50,
        "無操作が注入した Shift から来ていない (idle_ms={idle}、送ってから返るまで {s2r} ms)"
    );
}

/// `cond` が真になるまで `limit` の間 10 ms ごとに見る。なれば true
fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if cond() {
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---- 入力の錠 (D-113) ----
//
// 別のスレッドが pid 入りの名前の錠を持っている間、工場 (`RealDevices`) の配線ごとの実行器が待ち、解かれてから
// フィクスチャの中をクリックすることを確かめる。既定の名前 `Local\ai-cu-input` は使わない (利用者の ai-cu の錠を握らない)

/// 別のスレッドが錠を持ち続ける長さ
const LOCK_HOLD_MS: u64 = 700;
/// 実行器が待ったとみなす下限 (持ち続ける長さから、知らせを受けてから時計を始めるまでのずれを引いた分)
const LOCK_WAIT_MIN_MS: u64 = 600;

#[test]
#[ignore = "実機のフィクスチャ限定。cargo test -- --ignored --test-threads=1"]
fn fixture_real_input_lock_waits_for_other_holder() {
    // 始まりの知らせ (D-094) は済ませ、フィクスチャは枠なしで開く (実行器の枠は工場の `RealIndicator` が出す)
    dpi::ensure_per_monitor_v2().expect("Per-Monitor V2 を宣言できない");
    drop(start_frame(Frame::Show));
    let f = Fixture::open_without_frame("input_lock");
    f.win.clear_events();
    let name = format!("Local\\ai-cu-input-test-{}-fixture", std::process::id());
    let (held_tx, held_rx) = mpsc::channel::<()>();
    let holder_name = name.clone();
    let holder = std::thread::spawn(move || {
        let m = InputMutex::open(&holder_name).expect("錠を開ける");
        assert!(m.try_lock().expect("錠を試せる"), "空いている錠が取れない");
        held_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(LOCK_HOLD_MS));
        let released_at = Instant::now();
        m.unlock().expect("持ったスレッドで解ける");
        released_at
    });
    held_rx.recv().expect("錠を取った知らせ");
    // 待った実行の予告の間 (既定 3000 ms) で経過の断言が満たされないように、予告の間は 0 にする
    let policy = fixture_policy(f.client, |c| c.indicator.lead_ms = 0);
    let p = f.at(200, 150);
    let mut devices = RealDevices::with_input_lock_name(&name);
    let start = Instant::now();
    let r = {
        let (mut indicator, mut input_lock) = devices.indicator_and_lock();
        f.execute_locked(
            &policy,
            &format!(r#"{{"type":"click","x":{},"y":{}}}"#, p.x, p.y),
            indicator.as_mut(),
            input_lock.as_mut(),
            0,
        )
    };
    let returned_at = Instant::now();
    let waited = returned_at - start;
    let released_at = holder.join().expect("錠を持つスレッド");
    let ev = f.events_until(|e| {
        e.iter().any(|x| {
            matches!(
                x,
                FixtureEvent::ButtonUp {
                    button: Button::Left,
                    ..
                }
            )
        })
    });
    let clicked = ev.contains(&FixtureEvent::ButtonUp {
        button: Button::Left,
        x: 200,
        y: 150,
    });
    println!(
        "FIXTURE input_lock waited_ms={} clicked={clicked}",
        waited.as_millis()
    );
    r.expect("錠が解かれた後の実行");
    assert!(clicked, "フィクスチャにクリックが届かない: {ev:?}");
    assert!(
        waited >= Duration::from_millis(LOCK_WAIT_MIN_MS),
        "錠を待たずに返った: {waited:?}"
    );
    assert!(
        returned_at > released_at,
        "錠が解かれる前に返った (返るまで {waited:?})"
    );
}
