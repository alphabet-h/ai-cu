//! 操作を、装置へ送るバッチと待ちの列に展開する (純関数)。ドライランの出力と、実行器の両方がこれを使う。

use serde::Serialize;

use crate::device::InputEvent;
use crate::geometry::{Point, Rect};
use crate::keys::KeyCombo;
use crate::script::{Action, Button, ElementFilter, Settle, WindowTarget};

const VK_RETURN: u16 = 0x0D;
const VK_TAB: u16 = 0x09;
pub const WHEEL_DELTA: i32 = 120;
const DRAG_SETTLE_MS: u64 = 50;
/// キーの組み合わせの後の落ち着き待ち。直後の文字入力の先頭が落ちるのを避ける (D-040)
pub const KEY_SETTLE_MS: u64 = 50;
/// クリックの直後にキーと文字が続くときの落ち着き待ち。クリックで移ったフォーカスを、アプリが次の描画で
/// 入力欄に反映するより先に最初の文字が届くと落ちる (gpui など。D-059)
pub const CLICK_SETTLE_MS: u64 = 200;
/// `focus` で `SetForegroundWindow` の後、前面になるのを待つ上限 (D-050)。時間の予算にも数える
pub const FOCUS_SETTLE_MS: u64 = 500;
/// `scroll` のノッチとノッチの間の待ち (D-063)。本物のホイールに合わせて 1 ノッチずつ送る
pub const SCROLL_NOTCH_INTERVAL_MS: u64 = 20;
/// `close` で `WM_CLOSE` を置いた後、ウィンドウが消えるのを待つ上限 (D-062)。時間の予算にも数える
pub const CLOSE_SETTLE_MS: u64 = 1000;
/// `wait_for` が条件を確かめる間隔 (D-068)。UIA の FindAll は重いので 50 ms では回さない。
/// 最初の判定は待たずに行い、間の待ちは 50 ms ごとに停止要因と期限を見る
pub const WAIT_FOR_POLL_MS: u64 = 250;
/// 画面の待ち (`wait_stable` / `wait_change`、D-081) が撮る間隔。撮影と縮小は UIA の FindAll より軽いので、
/// 300 ms の静けさを 3〜4 回の比較で確かめられる間隔にする。最初の撮影は待たずに行う
pub const WAIT_SCREEN_POLL_MS: u64 = 100;
/// 操作列の撮影 (`Executor::grab`) で対象のウィンドウが隠れて見えたとき、解き直して待つ上限 (D-082)。focus の直後の
/// 描画の過渡と、閉じかけの窓に次の窓が重なる間を覆う。時間の予算には数えない
pub const OCCLUDED_GRACE_MS: u64 = 500;

/// 画面の待ちが満たす条件 (D-081)。`Stable` は直前のフレームとの差が閾値以下の比較が `stable_ms` 続くこと、
/// `Change` は待ち始めのフレーム (基準) との差が閾値を超えること
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Until {
    Stable { stable_ms: u64 },
    Change,
}

/// バッチの入力が届く先。ガードはこれを見て、座標の下か前面のウィンドウを確かめる (D-033)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// マウス。各点の下のウィンドウに届く。空なら現在のカーソル位置
    Mouse { points: Vec<Point> },
    /// キーボード。前面のウィンドウに届く
    Keyboard,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// 1 回の SendInput で送るイベント列
    Batch {
        target: Target,
        events: Vec<InputEvent>,
    },
    Sleep {
        ms: u64,
    },
    /// `window` があれば撮る直前にその矩形を解く (D-053)。どちらも None なら仮想デスクトップ全体
    Capture {
        rect: Option<Rect>,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<WindowTarget>,
        scale: f64,
        /// 長辺の上限 (D-072)。実際の縮尺は矩形を解いた後に `image::effective_scale` で決める
        #[serde(skip_serializing_if = "Option::is_none")]
        max_edge: Option<u32>,
        path: String,
        /// 撮る前に画面が落ち着くのを待つ (D-081)。落ち着いたときか時間切れのときの最後のフレームを保存する
        #[serde(skip_serializing_if = "Option::is_none")]
        settle: Option<Settle>,
    },
    /// 許可されたアプリのウィンドウを前面にする (D-050)。入力は送らない
    Focus {
        process: String,
        title: Option<String>,
    },
    /// 許可されたアプリのウィンドウに `WM_CLOSE` を置く (D-062)。入力は送らず、前面も変えない
    Close {
        process: String,
        title: String,
    },
    /// 対象のウィンドウ、要素、文字列が出る (`absent` なら消える) まで待つ (D-068)。読むだけで何も送らない
    WaitFor {
        process: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        element: Option<ElementFilter>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        absent: bool,
        timeout_ms: u64,
    },
    /// 画面が落ち着く / 変わるまで待つ (D-081)。撮る対象は `Capture` と同じ (`window` は撮るたびに矩形を解く)。
    /// 撮るだけで何も送らない
    WaitScreen {
        rect: Option<Rect>,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<WindowTarget>,
        until: Until,
        threshold: f64,
        timeout_ms: u64,
    },
}

fn button(b: Button, down: bool) -> InputEvent {
    InputEvent::Button { button: b, down }
}

fn mouse(points: Vec<Point>, events: Vec<InputEvent>) -> Step {
    Step::Batch {
        target: Target::Mouse { points },
        events,
    }
}

/// 始点と終点の間を `steps` 回で補間した点 (終点を含み、始点は含まない)。
pub fn interpolate(from: Point, to: Point, steps: u32) -> Vec<Point> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|i| {
            let t = f64::from(i) / f64::from(steps);
            let x = f64::from(from.x) + (f64::from(to.x) - f64::from(from.x)) * t;
            let y = f64::from(from.y) + (f64::from(to.y) - f64::from(from.y)) * t;
            Point::new(x.round() as i32, y.round() as i32)
        })
        .collect()
}

/// キーの組み合わせを、押す・離すの列にする。修飾キーは先に押して後で逆順に離す。1 回で送る (D-019)。
pub fn key_events(combo: &KeyCombo) -> Vec<InputEvent> {
    let mut ev = Vec::new();
    for m in &combo.modifiers {
        ev.push(InputEvent::Key {
            vk: m.vk(),
            down: true,
            extended: false,
        });
    }
    let extended = combo.key.is_extended();
    ev.push(InputEvent::Key {
        vk: combo.key.vk,
        down: true,
        extended,
    });
    ev.push(InputEvent::Key {
        vk: combo.key.vk,
        down: false,
        extended,
    });
    for m in combo.modifiers.iter().rev() {
        ev.push(InputEvent::Key {
            vk: m.vk(),
            down: false,
            extended: false,
        });
    }
    ev
}

fn tap_vk(vk: u16, out: &mut Vec<InputEvent>) {
    out.push(InputEvent::Key {
        vk,
        down: true,
        extended: false,
    });
    out.push(InputEvent::Key {
        vk,
        down: false,
        extended: false,
    });
}

/// 文字列を、1 文字ずつの入力イベントのバッチにする。改行は Enter、タブは Tab、その他は UTF-16 の単位ごとに
/// `KEYEVENTF_UNICODE` (サロゲートペアは 2 単位を同じバッチで順に)。`\r\n` は 1 回の Enter。
/// 受け手 (新しいメモ帳など) は入力を非同期に処理し、まとめて速く送ると文字を落としたり順序を入れ替えたり
/// するので、1 文字ずつ送って間を空ける (D-040)。
pub fn text_events(text: &str) -> Vec<Vec<InputEvent>> {
    let mut chunks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let mut cur = Vec::new();
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                tap_vk(VK_RETURN, &mut cur);
            }
            '\n' => tap_vk(VK_RETURN, &mut cur),
            '\t' => tap_vk(VK_TAB, &mut cur),
            _ => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf).iter() {
                    cur.push(InputEvent::Unicode {
                        unit: *unit,
                        down: true,
                    });
                    cur.push(InputEvent::Unicode {
                        unit: *unit,
                        down: false,
                    });
                }
            }
        }
        chunks.push(cur);
    }
    chunks
}

/// 操作を手順に展開する。形の検査 (`script::shape_issue`) を通った操作を前提にする。
pub fn plan(action: &Action) -> Result<Vec<Step>, String> {
    Ok(match action {
        Action::Click { x, y, button: b } => {
            let p = Point::new(*x, *y);
            vec![mouse(
                vec![p],
                vec![InputEvent::move_to(p), button(*b, true), button(*b, false)],
            )]
        }
        Action::DoubleClick { x, y, button: b } => {
            let p = Point::new(*x, *y);
            vec![mouse(
                vec![p],
                vec![
                    InputEvent::move_to(p),
                    button(*b, true),
                    button(*b, false),
                    button(*b, true),
                    button(*b, false),
                ],
            )]
        }
        Action::Drag {
            from,
            to,
            button: b,
            steps,
            duration_ms,
        } => {
            let (from, to): (Point, Point) = ((*from).into(), (*to).into());
            let mut out = vec![
                mouse(vec![from], vec![InputEvent::move_to(from)]),
                Step::Sleep { ms: DRAG_SETTLE_MS },
                mouse(vec![from], vec![button(*b, true)]),
            ];
            let per_step = duration_ms / u64::from((*steps).max(1));
            for p in interpolate(from, to, *steps) {
                if per_step > 0 {
                    out.push(Step::Sleep { ms: per_step });
                }
                out.push(mouse(vec![p], vec![InputEvent::move_to(p)]));
            }
            out.push(Step::Sleep { ms: DRAG_SETTLE_MS });
            out.push(mouse(vec![to], vec![button(*b, false)]));
            out
        }
        Action::Move { x, y } => {
            let p = Point::new(*x, *y);
            vec![mouse(vec![p], vec![InputEvent::move_to(p)])]
        }
        Action::Scroll {
            x,
            y,
            clicks,
            horizontal,
        } => {
            // 本物のホイールと同じく 1 ノッチ (WHEEL_DELTA) ずつ、間を空けて送る。大きな delta を 1 つに載せると、
            // 丸めて少ししか動かないアプリがある (gpui。D-063)
            let wheel = InputEvent::Wheel {
                delta: WHEEL_DELTA * clicks.signum(),
                horizontal: *horizontal,
            };
            let at = match (x, y) {
                (Some(x), Some(y)) => Some(Point::new(*x, *y)),
                _ => None,
            };
            let mut out = Vec::new();
            for i in 0..clicks.unsigned_abs() {
                if i > 0 {
                    out.push(Step::Sleep {
                        ms: SCROLL_NOTCH_INTERVAL_MS,
                    });
                }
                out.push(match at {
                    Some(p) if i == 0 => mouse(vec![p], vec![InputEvent::move_to(p), wheel]),
                    Some(p) => mouse(vec![p], vec![wheel]),
                    None => mouse(Vec::new(), vec![wheel]),
                });
            }
            out
        }
        Action::Type { text, interval_ms } => {
            let mut out = Vec::new();
            for (i, chunk) in text_events(text).into_iter().enumerate() {
                if i > 0 && *interval_ms > 0 {
                    out.push(Step::Sleep { ms: *interval_ms });
                }
                out.push(Step::Batch {
                    target: Target::Keyboard,
                    events: chunk,
                });
            }
            out
        }
        Action::Key { keys } => {
            let combo: KeyCombo = keys.parse()?;
            vec![
                Step::Batch {
                    target: Target::Keyboard,
                    events: key_events(&combo),
                },
                Step::Sleep { ms: KEY_SETTLE_MS },
            ]
        }
        Action::Wait { ms } => vec![Step::Sleep { ms: *ms }],
        Action::Screenshot {
            path,
            region,
            window,
            scale,
            max_edge,
            settle,
        } => vec![Step::Capture {
            rect: region.map(Rect::from),
            window: window.clone(),
            scale: *scale,
            max_edge: *max_edge,
            path: path.clone(),
            settle: *settle,
        }],
        Action::Focus { process, title } => vec![Step::Focus {
            process: process.clone(),
            title: title.clone(),
        }],
        Action::Close { process, title } => vec![Step::Close {
            process: process.clone(),
            title: title.clone(),
        }],
        Action::WaitFor {
            process,
            title,
            element,
            text,
            absent,
            timeout_ms,
        } => vec![Step::WaitFor {
            process: process.clone(),
            title: title.clone(),
            element: element.clone(),
            text: text.clone(),
            absent: *absent,
            timeout_ms: *timeout_ms,
        }],
        Action::WaitStable {
            region,
            window,
            stable_ms,
            threshold,
            timeout_ms,
        } => vec![Step::WaitScreen {
            rect: region.map(Rect::from),
            window: window.clone(),
            until: Until::Stable {
                stable_ms: *stable_ms,
            },
            threshold: *threshold,
            timeout_ms: *timeout_ms,
        }],
        Action::WaitChange {
            region,
            window,
            threshold,
            timeout_ms,
        } => vec![Step::WaitScreen {
            rect: region.map(Rect::from),
            window: window.clone(),
            until: Until::Change,
            threshold: *threshold,
            timeout_ms: *timeout_ms,
        }],
    })
}

/// 操作列で `action` の次が `next` のとき、`action` の手順の後に足す待ち。クリックの直後にキーか文字が続くなら
/// `CLICK_SETTLE_MS` (D-059)。次が `wait` なら、書いた待ちを尊重して足さない。
pub fn settle_before_next(action: &Action, next: Option<&Action>) -> Option<Step> {
    let clicked = matches!(action, Action::Click { .. } | Action::DoubleClick { .. });
    let keys_next = matches!(next, Some(Action::Type { .. } | Action::Key { .. }));
    (clicked && keys_next).then_some(Step::Sleep {
        ms: CLICK_SETTLE_MS,
    })
}

/// 手順に含まれる、マウスが触れるすべての点 (補間の点を含む)。
pub fn mouse_points(steps: &[Step]) -> Vec<Point> {
    steps
        .iter()
        .flat_map(|s| match s {
            Step::Batch {
                target: Target::Mouse { points },
                ..
            } => points.clone(),
            _ => Vec::new(),
        })
        .collect()
}

/// 手順の待ちの合計 (ミリ秒)。
pub fn total_sleep_ms(steps: &[Step]) -> u64 {
    steps
        .iter()
        .map(|s| match s {
            Step::Sleep { ms } => *ms,
            _ => 0,
        })
        .sum()
}

/// 時間の予算 (ミリ秒): 待ちの合計に、`focus` が前面になるのを待つ上限、`close` で消えるのを待つ上限、
/// `wait_for`、`wait_stable`、`wait_change` と screenshot の `settle` の `timeout_ms` (全額) を足したもの。実行時の期限の検証に使う。
pub fn time_budget_ms(steps: &[Step]) -> u64 {
    steps
        .iter()
        .map(|s| match s {
            Step::Sleep { ms } => *ms,
            Step::Focus { .. } => FOCUS_SETTLE_MS,
            Step::Close { .. } => CLOSE_SETTLE_MS,
            Step::WaitFor { timeout_ms, .. } | Step::WaitScreen { timeout_ms, .. } => *timeout_ms,
            Step::Capture {
                settle: Some(s), ..
            } => s.timeout_ms,
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Pos;

    #[test]
    fn plan_click_is_single_atomic_batch() {
        let steps = plan(&Action::Click {
            x: 5,
            y: 6,
            button: Button::Left,
        })
        .unwrap();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            Step::Batch { target, events } => {
                assert_eq!(
                    target,
                    &Target::Mouse {
                        points: vec![Point::new(5, 6)]
                    }
                );
                assert_eq!(events.len(), 3);
                assert_eq!(events[0], InputEvent::MoveTo { x: 5, y: 6 });
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plan_drag_interpolates_and_balances_button() {
        let steps = plan(&Action::Drag {
            from: Pos { x: 0, y: 0 },
            to: Pos { x: 100, y: 50 },
            button: Button::Left,
            steps: 4,
            duration_ms: 400,
        })
        .unwrap();
        let pts = mouse_points(&steps);
        assert_eq!(pts.first(), Some(&Point::new(0, 0)));
        assert_eq!(pts.last(), Some(&Point::new(100, 50)));
        assert!(pts.contains(&Point::new(50, 25)));
        let events: Vec<InputEvent> = steps
            .iter()
            .flat_map(|s| match s {
                Step::Batch { events, .. } => events.clone(),
                _ => vec![],
            })
            .collect();
        let downs = events
            .iter()
            .filter(|e| matches!(e, InputEvent::Button { down: true, .. }))
            .count();
        let ups = events
            .iter()
            .filter(|e| matches!(e, InputEvent::Button { down: false, .. }))
            .count();
        assert_eq!((downs, ups), (1, 1));
        assert_eq!(total_sleep_ms(&steps), 400 + 2 * DRAG_SETTLE_MS);
    }

    #[test]
    fn plan_key_combo_order() {
        let combo: KeyCombo = "shift+ctrl+left".parse().unwrap();
        let ev = key_events(&combo);
        let seq: Vec<(u16, bool)> = ev
            .iter()
            .map(|e| match e {
                InputEvent::Key { vk, down, .. } => (*vk, *down),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            seq,
            vec![
                (0x11, true),
                (0x10, true),
                (0x25, true),
                (0x25, false),
                (0x10, false),
                (0x11, false)
            ]
        );
        assert!(matches!(ev[2], InputEvent::Key { extended: true, .. }));
    }

    #[test]
    fn plan_text_surrogates_newlines_and_chunks() {
        let chunks = text_events("a😀\r\nb\tc");
        let flat: Vec<InputEvent> = chunks.concat();
        let units: Vec<u16> = flat
            .iter()
            .filter_map(|e| match e {
                InputEvent::Unicode { unit, down: true } => Some(*unit),
                _ => None,
            })
            .collect();
        assert_eq!(units, vec![0x61, 0xD83D, 0xDE00, 0x62, 0x63]);
        let vks: Vec<u16> = flat
            .iter()
            .filter_map(|e| match e {
                InputEvent::Key { vk, down: true, .. } => Some(*vk),
                _ => None,
            })
            .collect();
        assert_eq!(vks, vec![VK_RETURN, VK_TAB]);
        // 1 文字 1 バッチ (サロゲートペアは同じバッチ、\r\n は 1 つ)
        assert_eq!(chunks.len(), 6);
        assert_eq!(chunks[1].len(), 4);
    }

    #[test]
    fn plan_type_paces_characters_and_key_settles() {
        let steps = plan(&Action::Type {
            text: "abc".into(),
            interval_ms: 20,
        })
        .unwrap();
        let kinds: Vec<&str> = steps
            .iter()
            .map(|s| match s {
                Step::Batch { .. } => "batch",
                Step::Sleep { ms: 20 } => "sleep20",
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, vec!["batch", "sleep20", "batch", "sleep20", "batch"]);
        let fast = plan(&Action::Type {
            text: "abc".into(),
            interval_ms: 0,
        })
        .unwrap();
        assert_eq!(total_sleep_ms(&fast), 0);
        let key = plan(&Action::Key {
            keys: "delete".into(),
        })
        .unwrap();
        assert_eq!(key.last(), Some(&Step::Sleep { ms: KEY_SETTLE_MS }));
    }

    /// クリックの直後のキーと文字の前にだけ待ちを足す (D-059)
    #[test]
    fn settle_after_click_only_before_keys_or_text() {
        let click = Action::Click {
            x: 1,
            y: 2,
            button: Button::Left,
        };
        let double = Action::DoubleClick {
            x: 1,
            y: 2,
            button: Button::Left,
        };
        let text = Action::Type {
            text: "a".into(),
            interval_ms: 20,
        };
        let key = Action::Key {
            keys: "ctrl+a".into(),
        };
        let settle = Some(Step::Sleep {
            ms: CLICK_SETTLE_MS,
        });
        assert_eq!(settle_before_next(&click, Some(&text)), settle);
        assert_eq!(settle_before_next(&click, Some(&key)), settle);
        assert_eq!(settle_before_next(&double, Some(&text)), settle);
        assert_eq!(
            settle_before_next(&click, Some(&Action::Wait { ms: 5 })),
            None
        );
        assert_eq!(settle_before_next(&click, Some(&click)), None);
        assert_eq!(settle_before_next(&click, None), None);
        assert_eq!(settle_before_next(&key, Some(&text)), None);
        assert_eq!(settle_before_next(&text, Some(&text)), None);
    }

    #[test]
    fn plan_scroll_with_and_without_point() {
        let s = plan(&Action::Scroll {
            x: None,
            y: None,
            clicks: -3,
            horizontal: false,
        })
        .unwrap();
        // 1 ノッチ (-120) ずつ 3 つのバッチに分け、間に 20 ms 待つ (D-063)
        let notch = Step::Batch {
            target: Target::Mouse { points: vec![] },
            events: vec![InputEvent::Wheel {
                delta: -120,
                horizontal: false,
            }],
        };
        let gap = Step::Sleep {
            ms: SCROLL_NOTCH_INTERVAL_MS,
        };
        assert_eq!(
            s,
            vec![notch.clone(), gap.clone(), notch.clone(), gap, notch]
        );
        assert_eq!(total_sleep_ms(&s), 2 * SCROLL_NOTCH_INTERVAL_MS);
        let s = plan(&Action::Scroll {
            x: Some(1),
            y: Some(2),
            clicks: 2,
            horizontal: true,
        })
        .unwrap();
        // 点への移動は最初のバッチだけ。どのバッチも点を持つ (ガードが毎回その点の下を見る)
        let p = Point::new(1, 2);
        let wheel = InputEvent::Wheel {
            delta: 120,
            horizontal: true,
        };
        assert_eq!(
            s,
            vec![
                Step::Batch {
                    target: Target::Mouse { points: vec![p] },
                    events: vec![InputEvent::move_to(p), wheel],
                },
                Step::Sleep {
                    ms: SCROLL_NOTCH_INTERVAL_MS
                },
                Step::Batch {
                    target: Target::Mouse { points: vec![p] },
                    events: vec![wheel],
                },
            ]
        );
        assert_eq!(mouse_points(&s), vec![p, p]);
        // 上限 (100 ノッチ) でも待ちは約 2 秒
        let s = plan(&Action::Scroll {
            x: None,
            y: None,
            clicks: -100,
            horizontal: false,
        })
        .unwrap();
        assert_eq!(
            s.iter().filter(|s| matches!(s, Step::Batch { .. })).count(),
            100
        );
        assert_eq!(total_sleep_ms(&s), 99 * SCROLL_NOTCH_INTERVAL_MS);
    }

    /// focus は入力のバッチにならず、点も待ちも持たない (D-050)
    #[test]
    fn plan_focus_is_single_step_without_input() {
        let steps = plan(&Action::Focus {
            process: "notepad.exe".into(),
            title: Some("メモ".into()),
        })
        .unwrap();
        assert_eq!(
            steps,
            vec![Step::Focus {
                process: "notepad.exe".into(),
                title: Some("メモ".into()),
            }]
        );
        assert!(mouse_points(&steps).is_empty());
        assert_eq!(total_sleep_ms(&steps), 0, "待ちではない");
        assert_eq!(
            time_budget_ms(&steps),
            FOCUS_SETTLE_MS,
            "期限の予算には前面になるのを待つ上限を数える"
        );
    }

    /// close は入力のバッチにならず、点も待ちも持たない。予算には消えるのを待つ上限を数える (D-062)
    #[test]
    fn plan_close_is_single_step_without_input() {
        let steps = plan(&Action::Close {
            process: "notepad.exe".into(),
            title: "メモ".into(),
        })
        .unwrap();
        assert_eq!(
            steps,
            vec![Step::Close {
                process: "notepad.exe".into(),
                title: "メモ".into(),
            }]
        );
        assert!(mouse_points(&steps).is_empty());
        assert_eq!(total_sleep_ms(&steps), 0, "待ちではない");
        assert_eq!(time_budget_ms(&steps), CLOSE_SETTLE_MS);
    }

    /// wait_for (D-068) は 1 ステップで、入力のバッチにならず、点も固定の待ちも持たない。
    /// 時間の予算には timeout_ms を全額数え、固定の待ちの合計 (TUI の「待ちの合計」) には数えない
    #[test]
    fn plan_wait_for_is_single_step_counted_in_budget_not_in_sleep() {
        let filter = ElementFilter {
            name: Some("保存".into()),
            control_type: Some("button".into()),
        };
        let steps = plan(&Action::WaitFor {
            process: "notepad.exe".into(),
            title: Some("メモ".into()),
            element: Some(filter.clone()),
            text: Some("保存しました".into()),
            absent: true,
            timeout_ms: 1234,
        })
        .unwrap();
        assert_eq!(
            steps,
            vec![Step::WaitFor {
                process: "notepad.exe".into(),
                title: Some("メモ".into()),
                element: Some(filter),
                text: Some("保存しました".into()),
                absent: true,
                timeout_ms: 1234,
            }]
        );
        assert!(mouse_points(&steps).is_empty());
        assert_eq!(total_sleep_ms(&steps), 0, "固定の待ちではない");
        assert_eq!(
            time_budget_ms(&steps),
            1234,
            "期限の予算には上限を全額数える"
        );
        assert_eq!(WAIT_FOR_POLL_MS, 250);
        // ドライランの出力: None の項目は出さない
        let v = serde_json::to_value(&steps[0]).unwrap();
        assert_eq!(v["step"], "wait_for");
        assert_eq!(v["element"]["type"], "button");
        let minimal = plan(&Action::WaitFor {
            process: "notepad.exe".into(),
            title: None,
            element: None,
            text: None,
            absent: false,
            timeout_ms: 0,
        })
        .unwrap();
        let v = serde_json::to_value(&minimal[0]).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"step":"wait_for","process":"notepad.exe","absent":false,"timeout_ms":0})
        );
        assert_eq!(time_budget_ms(&minimal), 0);
    }

    /// screenshot の window と scale (D-053) と max_edge (D-072) は、そのまま Capture の手順に乗る (矩形は実行時に解く)
    #[test]
    fn plan_screenshot_window_and_scale_carry_into_capture() {
        let target = WindowTarget {
            process: "notepad.exe".into(),
            title: Some("メモ".into()),
        };
        let a = Action::Screenshot {
            path: "a.png".into(),
            region: None,
            window: Some(target.clone()),
            scale: 0.5,
            max_edge: Some(10),
            settle: Some(Settle {
                stable_ms: 400,
                threshold: 0.01,
                timeout_ms: 2500,
            }),
        };
        let steps = plan(&a).unwrap();
        assert_eq!(
            steps,
            vec![Step::Capture {
                rect: None,
                window: Some(target),
                scale: 0.5,
                max_edge: Some(10),
                path: "a.png".into(),
                settle: Some(Settle {
                    stable_ms: 400,
                    threshold: 0.01,
                    timeout_ms: 2500,
                }),
            }]
        );
        let json = serde_json::to_value(&steps[0]).unwrap();
        assert_eq!(json["max_edge"], 10, "plan の出力に max_edge が出る");
        assert_eq!(
            json["settle"],
            serde_json::json!({"stable_ms":400,"threshold":0.01,"timeout_ms":2500}),
            "plan の出力に settle が出る"
        );
        assert_eq!(time_budget_ms(&steps), 2500, "予算に settle.timeout_ms");
        let none = plan(&Action::Screenshot {
            path: "a.png".into(),
            region: None,
            window: None,
            scale: 1.0,
            max_edge: None,
            settle: None,
        })
        .unwrap();
        let json = serde_json::to_value(&none[0]).unwrap();
        assert!(json.get("max_edge").is_none(), "書かなければ出ない: {json}");
        assert!(json.get("settle").is_none(), "書かなければ出ない: {json}");
        assert_eq!(time_budget_ms(&none), 0, "settle が無ければ予算に数えない");
    }

    /// wait_stable と wait_change (D-081) は 1 つの WaitScreen に展開し、対象と値をそのまま運ぶ。
    /// 時間の予算には timeout_ms を全額数え、固定の待ちには数えない。ドライランの出力に until が出る
    #[test]
    fn plan_wait_screen_carries_fields_and_budget() {
        let target = WindowTarget {
            process: "app.exe".into(),
            title: None,
        };
        let stable = plan(&Action::WaitStable {
            region: None,
            window: Some(target.clone()),
            stable_ms: 400,
            threshold: 0.01,
            timeout_ms: 1234,
        })
        .unwrap();
        assert_eq!(
            stable,
            vec![Step::WaitScreen {
                rect: None,
                window: Some(target),
                until: Until::Stable { stable_ms: 400 },
                threshold: 0.01,
                timeout_ms: 1234,
            }]
        );
        assert_eq!(time_budget_ms(&stable), 1234);
        assert_eq!(total_sleep_ms(&stable), 0, "固定の待ちではない");
        assert!(mouse_points(&stable).is_empty());
        let change = plan(&Action::WaitChange {
            region: Some(crate::script::Region {
                x: 1,
                y: 2,
                w: 30,
                h: 40,
            }),
            window: None,
            threshold: 0.001,
            timeout_ms: 500,
        })
        .unwrap();
        assert_eq!(
            change,
            vec![Step::WaitScreen {
                rect: Some(Rect::new(1, 2, 30, 40)),
                window: None,
                until: Until::Change,
                threshold: 0.001,
                timeout_ms: 500,
            }]
        );
        assert_eq!(time_budget_ms(&change), 500);
        assert_eq!(WAIT_SCREEN_POLL_MS, 100);
        let v = serde_json::to_value(&stable[0]).unwrap();
        assert_eq!(v["step"], "wait_screen");
        assert_eq!(
            v["until"],
            serde_json::json!({"kind":"stable","stable_ms":400})
        );
        let v = serde_json::to_value(&change[0]).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"step":"wait_screen","rect":{"x":1,"y":2,"w":30,"h":40},"until":{"kind":"change"},"threshold":0.001,"timeout_ms":500})
        );
    }

    #[test]
    fn interpolate_endpoints() {
        assert_eq!(
            interpolate(Point::new(0, 0), Point::new(10, 0), 1),
            vec![Point::new(10, 0)]
        );
        assert_eq!(
            interpolate(Point::new(-10, -10), Point::new(10, 10), 2),
            vec![Point::new(0, 0), Point::new(10, 10)]
        );
    }
}
