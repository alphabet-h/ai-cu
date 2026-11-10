//! 層の合成の安全の性質を、任意の層の組で確かめるプロパティテスト (D-071)。
//! 約束は D-018 と D-029 の「追加の層は deny を足せるが allow を広げられない」。性質の一覧は
//! docs/specs/2026-10-21-policy-props.md。実デスクトップには触れない (`Policy` の判定と `validate` だけを呼ぶ)。
//!
//! 回し方: `proptest!` マクロは使わず、各テストで種を固定した `TestRunner` を作る。失敗の保存は切る
//! (対照で作られる偽の反例のファイルを残さない)。本物の反例は、src/policy.rs か src/validate.rs の
//! 表駆動の単体テストに書き写す。

use std::cell::Cell;
use std::path::PathBuf;

use ai_cu::config::{
    Config, General, INDICATOR_LEAD_MAX_MS, IndicatorConfig, Layer, Limits, Rules, Stop,
};
use ai_cu::device::WindowInfo;
use ai_cu::error::ErrorKind;
use ai_cu::geometry::{Point, Rect};
use ai_cu::keys::KeyCombo;
use ai_cu::policy::Policy;
use ai_cu::script::{Action, ActionKind, Button, ClickModifier, Pos, Region, Script, WindowTarget};
use ai_cu::validate::{Validation, validate};
use proptest::bool::weighted;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use proptest::test_runner::{RngSeed, TestCaseResult, TestRunner};

/// 乱数の種。固定して、前提の件数の下限の assert を実行ごとにぶれない確認にする (D-071)
const SEED: u64 = 0x0418_0071;
/// 1 つの性質で回す件数 (proptest の既定と同じ)
const CASES: u32 = 256;

fn runner(cases: u32) -> TestRunner {
    TestRunner::new(ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(SEED),
        ..ProptestConfig::default()
    })
}

/// `strategy` の値で `test` を `cases` 回まわす。破れたら、縮めた反例を出して panic する。
fn check<S: Strategy>(cases: u32, strategy: S, test: impl Fn(S::Value) -> TestCaseResult) {
    if let Err(e) = runner(cases).run(&strategy, test) {
        panic!("{e}");
    }
}

// ---------------------------------------------------------------------------
// 語彙と表記揺れ
// ---------------------------------------------------------------------------

const PROCESSES: [&str; 4] = ["notepad", "calc", "zed", "explorer"];
const CLASSES: [&str; 3] = ["Notepad", "Edit", "Chrome_WidgetWin_1"];

/// (修飾キー, キー)。後ろ 2 つは組み込みの拒否
const KEYS: [(&[&str], &str); 8] = [
    (&["ctrl"], "s"),
    (&["alt"], "tab"),
    (&[], "enter"),
    (&[], "tab"),
    (&["shift"], "f4"),
    (&["ctrl", "shift"], "t"),
    (&["win"], "r"),
    (&["ctrl", "alt"], "del"),
];
/// 組み込みで拒否するキーの組 (src/keys.rs の `builtin_denied`)
const BUILTIN_KEYS: [(&[&str], &str); 5] = [
    (&["win"], "r"),
    (&["ctrl", "alt"], "del"),
    (&["alt"], "f4"),
    (&["ctrl", "shift"], "esc"),
    (&["ctrl"], "esc"),
];

#[derive(Debug, Clone, Copy)]
struct ProcSpelling {
    upper: bool,
    exe: bool,
    path: bool,
}

fn arb_proc_spelling() -> impl Strategy<Value = ProcSpelling> {
    (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(|(upper, exe, path)| ProcSpelling {
        upper,
        exe,
        path,
    })
}

fn spell_process(word: &str, s: ProcSpelling) -> String {
    let mut out = word.to_string();
    if s.exe {
        out.push_str(".exe");
    }
    if s.upper {
        out = out.to_uppercase();
    }
    if s.path {
        out = format!("C:\\Apps\\{out}");
    }
    out
}

fn arb_process() -> impl Strategy<Value = String> {
    (select(&PROCESSES[..]), arb_proc_spelling()).prop_map(|(w, s)| spell_process(w, s))
}

/// 0: そのまま、1: 小文字、2: 大文字
fn spell_class(word: &str, s: u8) -> String {
    match s % 3 {
        0 => word.to_string(),
        1 => word.to_lowercase(),
        _ => word.to_uppercase(),
    }
}

fn arb_class() -> impl Strategy<Value = String> {
    (select(&CLASSES[..]), any::<u8>()).prop_map(|(w, s)| spell_class(w, s))
}

#[derive(Debug, Clone, Copy, Default)]
struct KeySpelling {
    swap: bool,
    alias: [u8; 2],
    key_alias: u8,
    upper: bool,
}

fn arb_key_spelling() -> impl Strategy<Value = KeySpelling> {
    (any::<bool>(), any::<[u8; 2]>(), any::<u8>(), any::<bool>()).prop_map(
        |(swap, alias, key_alias, upper)| KeySpelling {
            swap,
            alias,
            key_alias,
            upper,
        },
    )
}

fn modifier_alias(m: &str, pick: u8) -> &'static str {
    let names: &[&'static str] = match m {
        "ctrl" => &["ctrl", "control", "ctl"],
        "alt" => &["alt", "option"],
        "shift" => &["shift"],
        "win" => &["win", "super", "lwin"],
        _ => unreachable!("語彙に無い修飾キー {m}"),
    };
    names[pick as usize % names.len()]
}

fn key_alias(k: &str, pick: u8) -> String {
    let names: &[&str] = match k {
        "del" => &["del", "delete"],
        "enter" => &["enter", "return"],
        "esc" => &["esc", "escape"],
        _ => &[],
    };
    if names.is_empty() {
        k.to_string()
    } else {
        names[pick as usize % names.len()].to_string()
    }
}

fn spell_key(mods: &[&str], key: &str, s: KeySpelling) -> String {
    let mut parts: Vec<String> = mods
        .iter()
        .enumerate()
        .map(|(i, m)| modifier_alias(m, s.alias[i % 2]).to_string())
        .collect();
    if s.swap {
        parts.reverse();
    }
    parts.push(key_alias(key, s.key_alias));
    let joined = parts.join("+");
    if s.upper {
        joined.to_uppercase()
    } else {
        joined
    }
}

fn arb_key_from(
    list: &'static [(&'static [&'static str], &'static str)],
) -> impl Strategy<Value = String> {
    (select(list), arb_key_spelling()).prop_map(|((mods, key), s)| spell_key(mods, key, s))
}

fn arb_key() -> impl Strategy<Value = String> {
    arb_key_from(&KEYS)
}

/// 0..400 を 50 刻みの左上、50..=200 を 50 刻みの幅と高さ
fn arb_grid_rect() -> impl Strategy<Value = Rect> {
    (0i32..8, 0i32..8, 1u32..=4, 1u32..=4)
        .prop_map(|(x, y, w, h)| Rect::new(x * 50, y * 50, w * 50, h * 50))
}

/// 0: `x,y,w,h`、1: 区切りの後に空白、2: 前後にも空白
fn spell_region(r: Rect, s: u8) -> String {
    match s % 3 {
        0 => format!("{},{},{},{}", r.x, r.y, r.w, r.h),
        1 => format!("{}, {}, {}, {}", r.x, r.y, r.w, r.h),
        _ => format!(" {} ,{},{} , {} ", r.x, r.y, r.w, r.h),
    }
}

fn arb_region() -> impl Strategy<Value = String> {
    (arb_grid_rect(), any::<u8>()).prop_map(|(r, s)| spell_region(r, s))
}

/// 0: 正規名、1: 大文字、2: `_` を `-` に
fn spell_action(k: ActionKind, s: u8) -> String {
    match s % 3 {
        0 => k.name().to_string(),
        1 => k.name().to_uppercase(),
        _ => k.name().replace('_', "-"),
    }
}

/// `capture.save_dirs` の語彙 (D-075)。空、上下の関係 (`C:\a` と `C:\a\b`)、無関係 (`C:\b`)
const SAVE_DIRS: [&[&str]; 4] = [&[], &[r"C:\a"], &[r"C:\a\b"], &[r"C:\b"]];
/// 保存先の問い合わせ。`C:\a` だけの下、`C:\a` と `C:\a\b` の下 (大文字小文字違い)、どれの下でもない (部品の境界)
const SAVE_PATHS: [&str; 3] = [r"C:\a\x.png", r"c:\A\b\y.png", r"C:\ab\z.png"];

/// `mcp.require_approval` の語彙 (D-078)。`MCP_TOOL_NAMES` の部分集合: 空、1 つ、重なる 2 つ、重ならないもの
const MCP_SETS: [&[&str]; 5] = [&[], &["click"], &["click", "type"], &["type"], &["shot"]];

fn arb_action_name() -> impl Strategy<Value = String> {
    select(&ActionKind::ALL[..]).prop_map(|k| k.name().to_string())
}

// ---------------------------------------------------------------------------
// 設定と層
// ---------------------------------------------------------------------------

/// 1 つの次元: 1/2 で空 (allow なら「制限しない」)、残りで 1〜2 件
fn arb_dim(item: impl Strategy<Value = String> + 'static) -> BoxedStrategy<Vec<String>> {
    prop_oneof![Just(Vec::new()), vec(item, 1..=2)].boxed()
}

fn arb_rules() -> impl Strategy<Value = Rules> {
    (
        arb_dim(arb_action_name()),
        arb_dim(arb_process()),
        arb_dim(arb_class()),
        arb_dim(arb_region()),
        arb_dim(arb_key()),
    )
        .prop_map(|(actions, processes, classes, regions, keys)| Rules {
            actions,
            processes,
            classes,
            regions,
            keys,
        })
}

/// 設定の値 (TOML の文字列は経ない)。総スイッチと全画面の許可は 3:1 で true に寄せる
fn arb_config() -> impl Strategy<Value = Config> {
    (
        (weighted(0.75), weighted(0.75), weighted(0.75)),
        arb_rules(),
        arb_rules(),
        (1u32..=8, 1001u64..=20000, 0u32..=50),
        (1u32..=100, select(&["", r"C:\stop\a", r"C:\stop\b"][..])),
        (weighted(0.75), 0u64..=5000, select(&SAVE_DIRS[..])),
        select(&MCP_SETS[..]),
        // 枠の設定 (D-093): enabled と sound は半々、lead_ms は 0 から上限まで
        (weighted(0.5), 0u64..=INDICATOR_LEAD_MAX_MS, weighted(0.5)),
    )
        .prop_map(
            |(
                (input_enabled, capture_enabled, password_input),
                allow,
                deny,
                (max_actions, max_duration_ms, max_text_len),
                (mouse_threshold_px, sentinel),
                (full_screen, min_idle_ms, save_dirs),
                mcp_tools,
                (indicator_enabled, lead_ms, sound),
            )| {
                let mut cfg = Config {
                    general: General {
                        input_enabled,
                        capture_enabled,
                        password_input,
                    },
                    allow,
                    deny,
                    limits: Limits {
                        max_actions,
                        max_duration_ms,
                        max_text_len,
                    },
                    stop: Stop {
                        sentinel_file: sentinel.to_string(),
                        mouse_threshold_px,
                    },
                    ..Config::default()
                };
                cfg.capture.full_screen = full_screen;
                cfg.capture.min_idle_ms = min_idle_ms;
                cfg.capture.save_dirs = save_dirs.iter().map(|d| d.to_string()).collect();
                cfg.mcp.require_approval = mcp_tools.iter().map(|t| t.to_string()).collect();
                cfg.indicator = IndicatorConfig {
                    enabled: indicator_enabled,
                    lead_ms,
                    sound,
                };
                cfg
            },
        )
}

fn layer(cfg: &Config) -> Layer {
    Layer::from_config(cfg, "prop").expect("生成した設定は検証を通る")
}

fn arb_layer() -> impl Strategy<Value = Layer> {
    arb_config().prop_map(|c| layer(&c))
}

/// 床と、0〜3 個の追加の層
fn arb_floor_and_overlays() -> impl Strategy<Value = (Layer, Vec<Layer>)> {
    (arb_layer(), vec(arb_layer(), 0..=3))
}

/// `P = Policy([F] ++ O)` と `Pf = Policy([F])`
fn policies(floor: &Layer, overlays: &[Layer]) -> (Policy, Policy) {
    let mut all = vec![floor.clone()];
    all.extend(overlays.iter().cloned());
    (Policy::new(all), Policy::new(vec![floor.clone()]))
}

// ---------------------------------------------------------------------------
// 問い合わせ
// ---------------------------------------------------------------------------

const SCREEN: Rect = Rect::new(0, 0, 400, 400);

fn win(process: &str, class: &str) -> WindowInfo {
    WindowInfo {
        hwnd: 1,
        pid: 2,
        process: process.into(),
        class: class.into(),
        title: String::new(),
        elevated: Some(false),
    }
}

#[derive(Debug, Clone)]
struct Query {
    kind: ActionKind,
    key: KeyCombo,
    point: Point,
    rect: Rect,
    window: WindowInfo,
    process: String,
    text: String,
    full: (Option<Rect>, Option<Rect>),
    save_path: PathBuf,
}

impl Query {
    /// 全画面の判定の問い合わせが全画面に当たるか (src/policy.rs の `check_full_screen` と同じ定義)
    fn is_full_screen(&self) -> bool {
        match self.full {
            (None, _) => true,
            (Some(r), Some(s)) => r.contains_rect(&s),
            (Some(_), None) => false,
        }
    }
}

/// 0..450 の 25 刻みの格子の点 (画面 0,0,400,400 の外も少し含む)
fn arb_point() -> impl Strategy<Value = Point> {
    (0i32..18, 0i32..18).prop_map(|(x, y)| Point::new(x * 25, y * 25))
}

fn arb_screen() -> impl Strategy<Value = Option<Rect>> {
    prop_oneof![Just(Some(SCREEN)), Just(None)]
}

/// 1/2 で region の省略、1/4 で画面と同じか画面を覆う矩形、1/4 で格子の矩形 (画面を覆えない)
fn arb_full_query() -> impl Strategy<Value = (Option<Rect>, Option<Rect>)> {
    prop_oneof![
        2 => arb_screen().prop_map(|s| (None, s)),
        1 => prop_oneof![Just(SCREEN), Just(Rect::new(0, 0, 450, 450))].prop_map(|r| (Some(r), Some(SCREEN))),
        1 => (arb_grid_rect(), arb_screen()).prop_map(|(r, s)| (Some(r), s)),
    ]
}

fn arb_query() -> impl Strategy<Value = Query> {
    (
        select(&ActionKind::ALL[..]),
        arb_key(),
        arb_point(),
        arb_grid_rect(),
        (arb_process(), arb_class()),
        arb_process(),
        "[a-zあ]{0,60}",
        (arb_full_query(), select(&SAVE_PATHS[..])),
    )
        .prop_map(
            |(kind, key, point, rect, (p, c), process, text, (full, save_path))| Query {
                kind,
                key: key.parse().expect("語彙のキーは解析できる"),
                point,
                rect,
                window: win(&p, &c),
                process,
                text,
                full,
                save_path: PathBuf::from(save_path),
            },
        )
}

const CHECK_NAMES: [&str; 10] = [
    "check_action_kind",
    "check_key",
    "check_point",
    "check_capture_rect",
    "check_window",
    "check_process",
    "check_text_len",
    "check_full_screen",
    "check_save_dir",
    "password_input_allowed",
];

/// 10 の判定の `is_ok()` (並びは `CHECK_NAMES`)
fn check_all(p: &Policy, q: &Query) -> [bool; 10] {
    [
        p.check_action_kind(q.kind).is_ok(),
        p.check_key(&q.key).is_ok(),
        p.check_point(q.point).is_ok(),
        p.check_capture_rect(&q.rect).is_ok(),
        p.check_window(&q.window).is_ok(),
        p.check_process(&q.process).is_ok(),
        p.check_text_len(&q.text).is_ok(),
        p.check_full_screen(q.full.0.as_ref(), q.full.1.as_ref())
            .is_ok(),
        p.check_save_dir(&q.save_path).is_ok(),
        // パスワード欄への入力 (D-090)。問い合わせの値は使わない
        p.password_input_allowed().is_ok(),
    ]
}

// ---------------------------------------------------------------------------
// 性質
// ---------------------------------------------------------------------------

/// 性質 1 の窓の寄せ方: (寄せるか, 候補の番号, プロセスの表記, クラスの表記)
type WindowSteer = (bool, usize, ProcSpelling, u8);

fn arb_window_steer() -> impl Strategy<Value = WindowSteer> {
    (
        any::<bool>(),
        any::<usize>(),
        arb_proc_spelling(),
        any::<u8>(),
    )
}

/// 性質 1 の `check_window` の専用の生成 (性質 3 の前例と同じく、前提が成り立ちやすい形)。
/// 窓はプロセスとクラスの両方が全層を通る積なので、無作為の窓だけでは `P` を通る率が 1 割の境目にある。
/// 1/2 で、語彙の窓のうち `P` が通すもの (表記揺れつき) に差し替える。候補が無ければ無作為の窓のまま。
/// 候補を `P` で選んでも性質は空回りしない: 確かめるのは「`P` が通すなら `Pf` も通す」で、`Pf` は選びに使わない
fn steer_window(q: &mut Query, p: &Policy, (on, pick, ps, cs): WindowSteer) {
    if !on {
        return;
    }
    let candidates: Vec<WindowInfo> = PROCESSES
        .iter()
        .flat_map(|pw| {
            CLASSES
                .iter()
                .map(move |cw| win(&spell_process(pw, ps), &spell_class(cw, cs)))
        })
        .filter(|w| p.check_window(w).is_ok())
        .collect();
    if !candidates.is_empty() {
        q.window = candidates[pick % candidates.len()].clone();
    }
}

/// 性質 1: 追加の層は判定を広げない。`P` が通すものは `Pf` も通す
#[test]
fn prop_overlays_never_widen_checks() {
    let passed: [Cell<u32>; 10] = std::array::from_fn(|_| Cell::new(0));
    check(
        CASES,
        (arb_floor_and_overlays(), arb_query(), arb_window_steer()),
        |((floor, overlays), mut q, steer)| {
            let (p, pf) = policies(&floor, &overlays);
            steer_window(&mut q, &p, steer);
            let (rp, rf) = (check_all(&p, &q), check_all(&pf, &q));
            let declares_save_dirs = std::iter::once(&floor)
                .chain(&overlays)
                .any(|l| !l.save_dirs.is_empty());
            for i in 0..CHECK_NAMES.len() {
                // check_action_kind は wait を、check_full_screen は全画面に当たらない問い合わせを、
                // check_save_dir は save_dirs を書いた層が 1 つも無い問い合わせを、どの層も見ずに
                // 無条件に通すので数えない
                let vacuous = (i == 0 && q.kind == ActionKind::Wait)
                    || (i == 7 && !q.is_full_screen())
                    || (i == 8 && !declares_save_dirs);
                if rp[i] && !vacuous {
                    passed[i].set(passed[i].get() + 1);
                }
                prop_assert!(!rp[i] || rf[i], "{} が追加の層で広がった", CHECK_NAMES[i]);
            }
            Ok(())
        },
    );
    for (i, n) in passed.iter().enumerate() {
        eprintln!("PROPS {}: P が通った {} / {CASES}", CHECK_NAMES[i], n.get());
        assert!(
            n.get() >= CASES / 10,
            "{} で P が通った件数 {} が下限 {} に届かない (生成の分布を直す)",
            CHECK_NAMES[i],
            n.get(),
            CASES / 10
        );
    }
}

/// 1/2 で「制限しない形」(実入力が有効で、allow のプロセス、クラス、領域が空) にした設定
fn arb_maybe_unrestricted_layer() -> impl Strategy<Value = Layer> {
    (any::<bool>(), arb_config()).prop_map(|(open, mut c)| {
        if open {
            c.general.input_enabled = true;
            c.allow.processes.clear();
            c.allow.classes.clear();
            c.allow.regions.clear();
        }
        layer(&c)
    })
}

/// 性質 2 と 3: 上限は床より緩まず、緩める判定 (`unrestricted`) は床が緩めるときだけ
#[test]
fn prop_overlays_only_tighten_limits() {
    let unrestricted = Cell::new(0u32);
    let strategy = (
        arb_maybe_unrestricted_layer(),
        vec(arb_maybe_unrestricted_layer(), 0..=3),
    );
    check(CASES, strategy, |(floor, overlays)| {
        let (p, pf) = policies(&floor, &overlays);
        // 性質 2
        prop_assert!(p.max_actions() <= pf.max_actions());
        prop_assert!(p.max_duration_ms() <= pf.max_duration_ms());
        prop_assert!(p.max_text_len() <= pf.max_text_len());
        prop_assert!(p.mouse_threshold_px() <= pf.mouse_threshold_px());
        prop_assert!(p.capture_min_idle_ms() >= pf.capture_min_idle_ms());
        // 操作の前の枠 (D-093): 知らせる側が安全側。追加の層は消せず、縮められず、黙らせられない
        prop_assert!(p.indicator_enabled() >= pf.indicator_enabled());
        prop_assert!(p.indicator_lead_ms() >= pf.indicator_lead_ms());
        prop_assert!(p.indicator_sound() >= pf.indicator_sound());
        let sentinels = p.sentinel_files();
        for s in pf.sentinel_files() {
            prop_assert!(sentinels.contains(&s), "基本の層の停止の目印 {s} が消えた");
        }
        // 毎回の承認を求める MCP の道具 (D-078): 床の分は追加の層で外れない
        let approval = p.mcp_require_approval();
        prop_assert!(
            pf.mcp_require_approval().is_subset(&approval),
            "基本の層の require_approval {:?} が {:?} に含まれない",
            pf.mcp_require_approval(),
            approval
        );
        // 性質 3
        if p.unrestricted() {
            unrestricted.set(unrestricted.get() + 1);
            prop_assert!(pf.unrestricted(), "追加の層で unrestricted になった");
        }
        if pf.has_region_rules() {
            prop_assert!(p.has_region_rules(), "追加の層で領域の規則が消えた");
        }
        Ok(())
    });
    eprintln!("PROPS unrestricted: {} / {CASES}", unrestricted.get());
    assert!(
        unrestricted.get() >= CASES / 20,
        "P が unrestricted だった件数 {} が下限 {} に届かない",
        unrestricted.get(),
        CASES / 20
    );
}

/// 性質 4: 層の並びを入れ替えても判定は変わらない
#[test]
fn prop_layer_order_does_not_matter() {
    let strategy = (vec(arb_layer(), 1..=4), arb_query())
        .prop_flat_map(|(layers, q)| (Just(layers.clone()), Just(layers).prop_shuffle(), Just(q)));
    check(CASES, strategy, |(layers, shuffled, q)| {
        let a = check_all(&Policy::new(layers), &q);
        let b = check_all(&Policy::new(shuffled), &q);
        prop_assert_eq!(a, b);
        Ok(())
    });
}

/// 性質 5 の問い合わせ (deny と allow の両方に同じ語を持つ次元)
#[derive(Debug, Clone)]
enum Probe {
    Process(String),
    Class(String),
    Key(String),
    Action(String),
    Region(Point),
}

/// 同じ語を表記を変えて deny と allow の両方に足した設定と、その語の第 3 の表記の問い合わせ
fn arb_conflicting_config() -> impl Strategy<Value = (Config, Probe)> {
    let non_wait: Vec<ActionKind> = ActionKind::ALL
        .into_iter()
        .filter(|k| *k != ActionKind::Wait)
        .collect();
    let non_builtin: Vec<(&[&str], &str)> = KEYS
        .into_iter()
        // 組み込みの拒否の判定は src/keys.rs の `builtin_denied` だけに置く (写しを作らない)
        .filter(|(m, k)| {
            let combo: KeyCombo = spell_key(m, k, KeySpelling::default())
                .parse()
                .expect("語彙のキーは解析できる");
            combo.builtin_denied().is_none()
        })
        .collect();
    let conflict = prop_oneof![
        (
            select(&PROCESSES[..]),
            arb_proc_spelling(),
            arb_proc_spelling(),
            arb_proc_spelling()
        )
            .prop_map(|(w, a, b, c)| (
                0usize,
                spell_process(w, a),
                spell_process(w, b),
                Probe::Process(spell_process(w, c))
            )),
        (select(&CLASSES[..]), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(w, a, b, c)| (
            1usize,
            spell_class(w, a),
            spell_class(w, b),
            Probe::Class(spell_class(w, c))
        )),
        (
            select(non_builtin),
            arb_key_spelling(),
            arb_key_spelling(),
            arb_key_spelling()
        )
            .prop_map(|((m, k), a, b, c)| (
                2usize,
                spell_key(m, k, a),
                spell_key(m, k, b),
                Probe::Key(spell_key(m, k, c))
            )),
        (select(non_wait), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(k, a, b, c)| (
            3usize,
            spell_action(k, a),
            spell_action(k, b),
            Probe::Action(spell_action(k, c))
        )),
        (
            arb_grid_rect(),
            any::<u8>(),
            any::<u8>(),
            0u32..200,
            0u32..200
        )
            .prop_map(|(r, a, b, dx, dy)| (
                4usize,
                spell_region(r, a),
                spell_region(r, b),
                Probe::Region(Point::new(r.x + (dx % r.w) as i32, r.y + (dy % r.h) as i32))
            )),
    ];
    (arb_config(), conflict).prop_map(|(mut cfg, (dim, deny, allow, probe))| {
        dim_of(&mut cfg.deny, dim).push(deny);
        dim_of(&mut cfg.allow, dim).push(allow);
        (cfg, probe)
    })
}

/// 性質 5 の次元の番号 (0 プロセス、1 クラス、2 キー、3 操作、4 領域) の規則の列
fn dim_of(r: &mut Rules, dim: usize) -> &mut Vec<String> {
    match dim {
        0 => &mut r.processes,
        1 => &mut r.classes,
        2 => &mut r.keys,
        3 => &mut r.actions,
        _ => &mut r.regions,
    }
}

/// 性質 5: 層の deny に当たる問い合わせは、その層の allow に何があっても拒否される (`wait` は除く)
#[test]
fn prop_deny_wins_within_a_layer() {
    let strategy = (
        arb_conflicting_config(),
        vec(arb_layer(), 0..=3),
        0usize..=3,
    );
    check(CASES, strategy, |((cfg, probe), mut others, at)| {
        let at = at.min(others.len());
        others.insert(at, layer(&cfg));
        let p = Policy::new(others);
        match &probe {
            Probe::Process(s) => {
                prop_assert!(p.check_process(s).is_err());
                prop_assert!(p.check_window(&win(s, "Edit")).is_err());
            }
            Probe::Class(s) => prop_assert!(p.check_window(&win("notepad.exe", s)).is_err()),
            Probe::Key(s) => {
                let k: KeyCombo = s.parse().expect("語彙のキーは解析できる");
                prop_assert!(p.check_key(&k).is_err());
            }
            Probe::Action(s) => {
                let k = ActionKind::parse(s).expect("操作の表記揺れは解析できる");
                prop_assert!(p.check_action_kind(k).is_err());
            }
            Probe::Region(pt) => {
                prop_assert!(p.check_point(*pt).is_err());
                prop_assert!(p.check_capture_rect(&Rect::new(pt.x, pt.y, 1, 1)).is_err());
            }
        }
        Ok(())
    });
}

/// 性質 6: 組み込みの拒否は、どの層の組でも拒否で `builtin`
#[test]
fn prop_builtin_key_denial_is_absolute() {
    let strategy = (vec(arb_layer(), 1..=4), arb_key_from(&BUILTIN_KEYS));
    check(CASES, strategy, |(layers, key)| {
        let k: KeyCombo = key.parse().expect("組み込みの拒否のキーは解析できる");
        let d = Policy::new(layers).check_key(&k);
        prop_assert!(matches!(&d, Err(d) if d.is_builtin()), "{key}: {d:?}");
        Ok(())
    });
}

/// 性質 7: 床が無い (閉じた層が床) とき、`wait` 以外のすべての操作と全画面の撮影を拒否する
#[test]
fn prop_closed_floor_denies_everything() {
    let strategy = (vec(arb_layer(), 0..=3), arb_screen());
    check(CASES, strategy, |(overlays, screen)| {
        let mut all = vec![Layer::closed("(既定) 何も許可しない")];
        all.extend(overlays);
        let p = Policy::new(all);
        for k in ActionKind::ALL {
            let r = p.check_action_kind(k);
            if k == ActionKind::Wait {
                prop_assert!(r.is_ok());
            } else {
                prop_assert!(r.is_err(), "{} が基本の層なしで通った", k.name());
            }
        }
        prop_assert!(p.check_full_screen(None, screen.as_ref()).is_err());
        Ok(())
    });
}

/// 性質 8: 同じ語の表記揺れは判定の結果を変えない
#[test]
fn prop_spelling_variants_agree() {
    let words = (
        (
            select(&PROCESSES[..]),
            arb_proc_spelling(),
            arb_proc_spelling(),
        ),
        (select(&CLASSES[..]), any::<u8>(), any::<u8>()),
        (select(&KEYS[..]), arb_key_spelling(), arb_key_spelling()),
    );
    let strategy = (vec(arb_layer(), 1..=4), words);
    check(
        CASES,
        strategy,
        |(layers, ((pw, pa, pb), (cw, ca, cb), ((km, kk), ka, kb)))| {
            let p = Policy::new(layers);
            let (p1, p2) = (spell_process(pw, pa), spell_process(pw, pb));
            let (c1, c2) = (spell_class(cw, ca), spell_class(cw, cb));
            let k1: KeyCombo = spell_key(km, kk, ka).parse().expect("解析できる");
            let k2: KeyCombo = spell_key(km, kk, kb).parse().expect("解析できる");
            prop_assert_eq!(p.check_process(&p1).is_ok(), p.check_process(&p2).is_ok());
            prop_assert_eq!(
                p.check_window(&win(&p1, &c1)).is_ok(),
                p.check_window(&win(&p2, &c2)).is_ok()
            );
            prop_assert_eq!(p.check_key(&k1).is_ok(), p.check_key(&k2).is_ok());
            Ok(())
        },
    );
}

// ---------------------------------------------------------------------------
// 検証 (validate) の性質
// ---------------------------------------------------------------------------

fn pos(p: Point) -> Pos {
    Pos { x: p.x, y: p.y }
}

fn region(r: Rect) -> Region {
    Region {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
    }
}

/// 撮影の対象 (region, window): 全画面 (region も window も無い)、window、画面と同じか画面を覆う矩形、格子の矩形。
/// screenshot と画面の待ち (D-081) が共有する
fn arb_capture_target() -> impl Strategy<Value = (Option<Region>, Option<WindowTarget>)> {
    prop_oneof![
        4 => Just((None, None)),
        2 => arb_process().prop_map(|process| (None, Some(WindowTarget { process, title: None }))),
        3 => prop_oneof![Just(SCREEN), Just(Rect::new(0, 0, 450, 450))]
            .prop_map(|r| (Some(region(r)), None)),
        3 => arb_grid_rect().prop_map(|r| (Some(region(r)), None)),
    ]
}

fn arb_screenshot() -> impl Strategy<Value = Action> {
    arb_capture_target().prop_map(|(region, window)| Action::Screenshot {
        path: "target/x.png".into(),
        region,
        window,
        scale: 1.0,
        max_edge: None,
        settle: None,
    })
}

/// 画面の待ち (D-081)。撮る対象は screenshot と同じ生成。値は形の検査を通る範囲 (timeout_ms は stable_ms 以上)
/// で、時間の予算 (開いた形で 20000) に収まる長さ
fn arb_wait_stable() -> impl Strategy<Value = Action> {
    (arb_capture_target(), 100u64..=300, 0u64..=200).prop_map(
        |((region, window), stable_ms, extra)| Action::WaitStable {
            region,
            window,
            stable_ms,
            threshold: 0.0,
            timeout_ms: stable_ms + extra,
        },
    )
}

fn arb_wait_change() -> impl Strategy<Value = Action> {
    (arb_capture_target(), 1u64..=500).prop_map(|((region, window), timeout_ms)| {
        Action::WaitChange {
            region,
            window,
            threshold: 0.001,
            timeout_ms,
        }
    })
}

/// 1 つの操作。座標は `arb_point` の格子、文字は 3:1 で改行とタブを含まない
fn arb_action() -> BoxedStrategy<Action> {
    let text = prop_oneof![3 => "[a-zあ]{0,60}", 1 => "[a-zあ\n\t]{0,60}"];
    let clicks = prop_oneof![-3i32..=-1, 1i32..=3];
    // 修飾キー (D-137) は空に寄せる (性質 9 と 11 の「P が通った」の件数を下限から遠ざけるため)
    let mods = || {
        prop_oneof![
            3 => Just(Vec::<ClickModifier>::new()),
            1 => Just(vec![ClickModifier::Ctrl]),
            1 => Just(vec![ClickModifier::Shift, ClickModifier::Alt]),
            1 => Just(vec![ClickModifier::Ctrl, ClickModifier::Alt]),
        ]
    };
    prop_oneof![
        (arb_point(), mods()).prop_map(|(p, modifiers)| Action::Click {
            x: p.x,
            y: p.y,
            button: Button::Left,
            modifiers,
        }),
        (arb_point(), mods()).prop_map(|(p, modifiers)| Action::DoubleClick {
            x: p.x,
            y: p.y,
            button: Button::Left,
            modifiers,
        }),
        arb_point().prop_map(|p| Action::Move { x: p.x, y: p.y }),
        (arb_point(), arb_point(), 1u32..=4, 0u64..=200).prop_map(|(a, b, steps, duration_ms)| {
            Action::Drag {
                from: pos(a),
                to: pos(b),
                button: Button::Left,
                steps,
                duration_ms,
            }
        }),
        (prop::option::of(arb_point()), clicks, any::<bool>()).prop_map(
            |(at, clicks, horizontal)| Action::Scroll {
                x: at.map(|p| p.x),
                y: at.map(|p| p.y),
                clicks,
                horizontal,
            }
        ),
        (text, 0u64..=20).prop_map(|(text, interval_ms)| Action::Type { text, interval_ms }),
        arb_key().prop_map(|keys| Action::Key { keys }),
        (0u64..=500).prop_map(|ms| Action::Wait { ms }),
        arb_screenshot(),
        arb_process().prop_map(|process| Action::Focus {
            process,
            title: None
        }),
        arb_process().prop_map(|process| Action::Close {
            process,
            title: "a".into()
        }),
        (arb_process(), 0u64..=500).prop_map(|(process, timeout_ms)| Action::WaitFor {
            process,
            title: None,
            element: None,
            text: None,
            absent: false,
            timeout_ms,
        }),
        arb_wait_stable(),
        arb_wait_change(),
    ]
    .boxed()
}

fn script(actions: Vec<Action>) -> Script {
    Script {
        version: 1,
        actions,
    }
}

/// 短い操作列 (1〜4 個)。全部の操作が通る組を作りやすくする
fn arb_script() -> impl Strategy<Value = Script> {
    vec(arb_action(), 1..=4).prop_map(script)
}

/// 検証用の層: 1/2 で「開いた形」(総スイッチと全画面の撮影が true、allow と deny が空、
/// max_text_len 100、max_duration_ms 20000)、残りは通常の生成。`max_actions` は床なら 1..=8、
/// 追加の層の開いた形なら 8..=50 (床だけが操作列の長さを下回る組を作る)
fn arb_validate_layer(is_floor: bool) -> impl Strategy<Value = Layer> {
    let open_max = if is_floor { 1u32..=8 } else { 8u32..=50 };
    (any::<bool>(), arb_config(), open_max).prop_map(|(open, mut c, max_actions)| {
        if open {
            c.general.input_enabled = true;
            c.general.capture_enabled = true;
            c.capture.full_screen = true;
            c.allow = Rules::default();
            c.deny = Rules::default();
            c.limits.max_text_len = 100;
            c.limits.max_duration_ms = 20000;
            c.limits.max_actions = max_actions;
            // 撮影の path は相対の target/x.png なので、save_dirs があると撮影がすべて断られ、
            // 性質 9 の前提の件数が下限を割る (D-075)
            c.capture.save_dirs.clear();
        }
        layer(&c)
    })
}

fn has_policy_denied(v: &Validation) -> bool {
    v.issues
        .iter()
        .any(|i| i.code == ErrorKind::PolicyDenied.code())
}

/// 性質 9 と 11: 追加の層は検証を通しやすくしない。`P` で通る操作列は `Pf` でも通り、
/// `Pf` で `policy_denied` になる操作列は `P` でも通らない
#[test]
fn prop_overlays_never_make_validation_pass() {
    let (passed, floor_denied) = (Cell::new(0u32), Cell::new(0u32));
    let strategy = (
        arb_validate_layer(true),
        vec(arb_validate_layer(false), 0..=3),
        arb_script(),
        arb_screen(),
    );
    check(CASES, strategy, |(floor, overlays, s, screen)| {
        let (p, pf) = policies(&floor, &overlays);
        let (vp, vf) = (validate(&s, &p, screen), validate(&s, &pf, screen));
        // 空の操作列や wait だけの列は層を見ずに通るので、性質 9 の前提に数えない
        if vp.ok() && s.actions.iter().any(|a| a.kind() != ActionKind::Wait) {
            passed.set(passed.get() + 1);
        }
        prop_assert!(
            !vp.ok() || vf.ok(),
            "追加の層で検証が通るようになった: 基本の層だけの問題 {:?}",
            vf.issues
        );
        if has_policy_denied(&vf) {
            floor_denied.set(floor_denied.get() + 1);
            prop_assert!(!vp.ok(), "基本の層の policy_denied が追加の層で消えた");
        }
        Ok(())
    });
    eprintln!(
        "PROPS validate: P が通った {} / {CASES}、Pf が policy_denied {} / {CASES}",
        passed.get(),
        floor_denied.get()
    );
    for (name, n) in [
        ("P で通った (wait 以外を含む)", passed.get()),
        ("Pf で policy_denied", floor_denied.get()),
    ] {
        assert!(
            n >= CASES / 10,
            "{name} の件数 {n} が下限 {} に届かない (生成の分布を直す)",
            CASES / 10
        );
    }
}

/// 性質 10: 床が閉じていれば、`wait` 以外の操作を 1 つでも含む操作列は検証を通らない
#[test]
fn prop_closed_floor_rejects_any_non_wait_script() {
    let non_wait = arb_action().prop_filter("wait 以外", |a| a.kind() != ActionKind::Wait);
    let strategy = (
        vec(arb_validate_layer(false), 0..=3),
        vec(arb_action(), 0..=3),
        non_wait,
        0usize..=3,
        arb_screen(),
    );
    check(
        CASES,
        strategy,
        |(overlays, mut actions, required, at, screen)| {
            actions.insert(at.min(actions.len()), required);
            let mut all = vec![Layer::closed("(既定) 何も許可しない")];
            all.extend(overlays);
            let v = validate(&script(actions), &Policy::new(all), screen);
            prop_assert!(!v.ok(), "基本の層なしで検証が通った");
            Ok(())
        },
    );
}
