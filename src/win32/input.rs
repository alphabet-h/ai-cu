//! `SendInput` とカーソル位置。ここは「送る」だけで、送ってよいかの判断はしない。
//! 判断は呼び出し側 (`backend::RealSink` の実行時ガードとフィクスチャの範囲確認、`run` のガード) が行う。

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, POINT};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, LASTINPUTINFO, MAPVK_VK_TO_VSC,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW,
    SendInput, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_RWIN, VK_XBUTTON1, VK_XBUTTON2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::device::{HeldInputs, InputEvent};
use crate::geometry::{Point, Rect};
use crate::script::Button;

/// 仮想デスクトップ上の物理ピクセルを、`MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` の 0..=65535 に正規化する。
/// Windows は正規化した値 n を `floor(n * 幅 / 65536)` のピクセルに対応づけるので、切り上げで求めると
/// 元のピクセルに戻る (幅が 65536 以下のとき)。実機の往復はフィクスチャのテストで確かめる。
pub fn normalize(p: Point, screen: Rect) -> (i32, i32) {
    fn axis(v: i32, origin: i32, size: u32) -> i32 {
        let size = i64::from(size.max(1));
        let off = (i64::from(v) - i64::from(origin)).clamp(0, size - 1);
        let n = (off * 65536 + size - 1) / size;
        n.clamp(0, 65535) as i32
    }
    (axis(p.x, screen.x, screen.w), axis(p.y, screen.y, screen.h))
}

/// `normalize` の逆 (Windows の対応づけの模型)。テストで往復を確かめるために使う。
pub fn denormalize(n: (i32, i32), screen: Rect) -> Point {
    fn axis(n: i32, origin: i32, size: u32) -> i32 {
        (i64::from(origin) + (i64::from(n) * i64::from(size)) / 65536) as i32
    }
    Point::new(axis(n.0, screen.x, screen.w), axis(n.1, screen.y, screen.h))
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn scan_code(vk: u16) -> u16 {
    // SAFETY: 整数を渡して整数を受け取るだけ。対応が無ければ 0 が返る。
    let sc = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) };
    (sc & 0xFFFF) as u16
}

/// イベント列を `INPUT` の配列に変換する (純粋な部分。スキャンコードの引き当てだけ OS を呼ぶ)。
pub fn to_inputs(events: &[InputEvent], screen: Rect) -> Vec<INPUT> {
    events
        .iter()
        .map(|e| match *e {
            InputEvent::MoveTo { x, y } => {
                let (nx, ny) = normalize(Point::new(x, y), screen);
                mouse_input(
                    nx,
                    ny,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                )
            }
            InputEvent::Button { button, down } => {
                let flags = match (button, down) {
                    (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
                    (Button::Left, false) => MOUSEEVENTF_LEFTUP,
                    (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                    (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
                    (Button::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                    (Button::Middle, false) => MOUSEEVENTF_MIDDLEUP,
                };
                mouse_input(0, 0, 0, flags)
            }
            InputEvent::Wheel { delta, horizontal } => {
                let flags = if horizontal {
                    MOUSEEVENTF_HWHEEL
                } else {
                    MOUSEEVENTF_WHEEL
                };
                // mouseData は DWORD だが、ホイールの量は符号付きとして解釈される
                mouse_input(0, 0, delta as u32, flags)
            }
            InputEvent::Key { vk, down, extended } => {
                let mut flags = 0;
                if !down {
                    flags |= KEYEVENTF_KEYUP;
                }
                if extended {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                key_input(vk, scan_code(vk), flags)
            }
            InputEvent::Unicode { unit, down } => {
                let flags = if down {
                    KEYEVENTF_UNICODE
                } else {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                };
                key_input(0, unit, flags)
            }
        })
        .collect()
}

/// 1 回の `SendInput` で送る。挿入できた数が足りなければエラー。
/// UIPI で遮られた場合は戻り値でも GetLastError でも区別できない (docs/design-notes.md) ので、
/// 管理者権限のウィンドウへの送信は呼び出し側のガードで事前に拒否する。
pub fn send(events: &[InputEvent], screen: Rect) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let inputs = to_inputs(events, screen);
    let count = u32::try_from(inputs.len()).map_err(|_| "イベントが多すぎる".to_string())?;
    // SAFETY: inputs は count 個の初期化済みの INPUT を持つ連続した領域で、呼び出しの間は生きている。
    // cbSize は INPUT 構造体の大きさをそのまま渡す。
    let sent = unsafe { SendInput(count, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if sent != count {
        return Err(format!(
            "SendInput が {count} 個のうち {sent} 個しか挿入しなかった (GetLastError={})",
            super::last_error()
        ));
    }
    Ok(())
}

/// `GetCursorPos` の失敗。5 (`ERROR_ACCESS_DENIED`) だけを分ける (D-066)。
/// Display はメッセージの文をそのまま出す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    /// `ERROR_ACCESS_DENIED`。画面のロックや別のデスクトップ (UAC など) が前面のことが多い
    AccessDenied(String),
    /// それ以外の失敗
    Other(String),
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CursorError::AccessDenied(m) | CursorError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CursorError {}

/// `GetLastError` の値から失敗を組み立てる (純関数。テストから使う)
pub(crate) fn cursor_error(code: u32) -> CursorError {
    // 5 (ERROR_ACCESS_DENIED) は、画面のロックや別のデスクトップ (UAC など) が前面のことが多い。
    // 原因は言い切らない。再試行では直らないので、人に確かめてもらう (docs/design-notes.md)
    if code == ERROR_ACCESS_DENIED {
        CursorError::AccessDenied(format!(
            "GetCursorPos が失敗 (GetLastError={code})。画面のロックや別のデスクトップ (UAC など) が前面のことが多い"
        ))
    } else {
        CursorError::Other(format!("GetCursorPos が失敗 (GetLastError={code})"))
    }
}

pub fn cursor_pos() -> Result<Point, CursorError> {
    let mut pt = POINT { x: 0, y: 0 };
    // SAFETY: pt は書き込み可能な POINT で、呼び出しの間は生きている。
    let ok = unsafe { GetCursorPos(&mut pt) };
    if ok == 0 {
        return Err(cursor_error(super::last_error()));
    }
    Ok(Point::new(pt.x, pt.y))
}

/// `GetAsyncKeyState` の戻り値が「押されている」か。最上位ビットだけを見る (最下位ビットの
/// 「前回の呼び出しから押された」は他のプロセスの呼び出しにも左右されるので見ない。D-066)
fn is_down(state: i16) -> bool {
    (state as u16) & 0x8000 != 0
}

fn async_down(vk: u16) -> bool {
    // SAFETY: 値の引数 (仮想キーコード) を 1 つ取り、キーの状態を返すだけ。書き込みは無い。
    is_down(unsafe { GetAsyncKeyState(i32::from(vk)) })
}

/// 見るマウスボタン (仮想キーコード, ボタンの名前)。`GetAsyncKeyState` は物理のボタンを見る
/// (左右の入れ替えの設定は反映しない。Microsoft Learn の GetAsyncKeyState)
const HELD_BUTTONS: [(u16, &str); 5] = [
    (VK_LBUTTON, "left"),
    (VK_RBUTTON, "right"),
    (VK_MBUTTON, "middle"),
    (VK_XBUTTON1, "x1"),
    (VK_XBUTTON2, "x2"),
];

/// 押されたままの修飾キー (Shift、Ctrl、Alt、左右の Win) とマウスボタン (D-066)。読むだけで離さない。
/// 修飾キーの表は `keys::Modifier` のものを使う (表を 2 つ持たない)。`Win` は `VK_LWIN` に加えて
/// `VK_RWIN` も見て、どちらかなら `win` を 1 回入れる。デスクトップが active でないか UIPI に
/// 遮られると 0 が返るので、見落としはあっても誤検出の向きには倒れない
pub fn keys_held() -> HeldInputs {
    let mut held = HeldInputs::default();
    for m in crate::keys::Modifier::ALL {
        let down = async_down(m.vk()) || (m == crate::keys::Modifier::Win && async_down(VK_RWIN));
        if down {
            held.keys.push(m.name());
        }
    }
    for (vk, name) in HELD_BUTTONS {
        if async_down(vk) {
            held.buttons.push(name);
        }
    }
    held
}

/// セッションの最後の入力 (人の操作。このプロセスを含む `SendInput` も数える) からの経過 (ミリ秒。D-049)。
/// `GetTickCount` は 49.7 日で一周するので、差は `wrapping_sub` で取る。
pub fn idle_ms() -> Result<u64, String> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: info は cbSize を正しく入れた LASTINPUTINFO で、呼び出しの間は生きている。関数はそこへ書くだけ。
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if ok == 0 {
        return Err(format!(
            "GetLastInputInfo が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    // SAFETY: 引数を取らず、起動からのミリ秒を返すだけ。
    let now = unsafe { GetTickCount() };
    Ok(u64::from(idle_from(now, info.dwTime)))
}

/// 最後の入力の tick から経過ミリ秒を出す。`dwTime` は単調とは限らない (SendInput は自分の tick を持ち込む。
/// microsoft-learn の GetLastInputInfo の注意) ので、「いま」より少し先の値は直前の入力とみなして 0 にする
/// (fail-closed)。一周 (49.7 日) をまたぐ通常の経過は `wrapping_sub` で正しく出る。
fn idle_from(now: u32, last: u32) -> u32 {
    const FUTURE_SLACK_MS: u32 = 60_000;
    let diff = now.wrapping_sub(last);
    if diff > u32::MAX - FUTURE_SLACK_MS {
        0
    } else {
        diff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GetCursorPos の 5 だけを AccessDenied に分け、原因は言い切らない (D-066)
    #[test]
    fn cursor_error_separates_access_denied_and_keeps_message() {
        let denied = cursor_error(5);
        assert!(matches!(denied, CursorError::AccessDenied(_)), "{denied:?}");
        assert_eq!(
            denied.to_string(),
            "GetCursorPos が失敗 (GetLastError=5)。画面のロックや別のデスクトップ (UAC など) が前面のことが多い"
        );
        let other = cursor_error(6);
        assert!(matches!(other, CursorError::Other(_)), "{other:?}");
        assert_eq!(other.to_string(), "GetCursorPos が失敗 (GetLastError=6)");
    }

    /// 押下の判定は最上位ビットだけを見る (最下位ビットの「前回から押された」は見ない。D-066)
    #[test]
    fn is_down_reads_only_the_high_bit() {
        assert!(!is_down(0));
        assert!(!is_down(0x0001), "最下位ビットだけは押されていない");
        assert!(!is_down(0x7fff));
        assert!(is_down(i16::MIN), "0x8000");
        assert!(is_down(-1), "0xffff");
        assert!(is_down(i16::MIN | 0x0001));
    }

    /// dwTime は単調でない (SendInput が自分の tick を持ち込む)。未来の値は直前の入力 (0) とみなし、
    /// 一周 (49.7 日) をまたぐ通常の経過は正しく出す (D-049)
    #[test]
    fn idle_from_handles_wrap_and_future_ticks() {
        assert_eq!(idle_from(10, 5), 5);
        assert_eq!(idle_from(5, u32::MAX - 4), 10, "一周またぎ");
        assert_eq!(
            idle_from(100, 105),
            0,
            "未来 (SendInput の tick) は直前の入力"
        );
        assert_eq!(idle_from(0, 1), 0);
        assert_eq!(idle_from(100, 100), 0);
        assert_eq!(idle_from(100, 100 + 60_000), 0, "余裕幅の内側");
        assert_eq!(
            idle_from(3_000_000_000, 0),
            3_000_000_000,
            "長い無操作は 0 にしない"
        );
    }

    #[test]
    fn normalize_roundtrips_every_pixel_on_this_pc_size() {
        let screen = Rect::new(0, 0, 3840, 2160);
        for x in (0..3840).step_by(7).chain([0, 1, 3838, 3839]) {
            let p = Point::new(x, (x * 9 / 16).min(2159));
            assert_eq!(denormalize(normalize(p, screen), screen), p, "{p:?}");
        }
    }

    #[test]
    fn normalize_roundtrips_with_negative_origin_and_multi_monitor() {
        let screen = Rect::new(-1920, -300, 1920 + 3840, 2160 + 300);
        for p in [
            Point::new(-1920, -300),
            Point::new(-1, 0),
            Point::new(0, 0),
            Point::new(3839, 2159),
            Point::new(1234, -17),
        ] {
            assert_eq!(denormalize(normalize(p, screen), screen), p, "{p:?}");
        }
    }

    #[test]
    fn normalize_clamps_out_of_range() {
        let screen = Rect::new(0, 0, 100, 100);
        assert_eq!(
            normalize(Point::new(-50, 500), screen),
            (0, 65536 * 99 / 100 + 1)
        );
        assert!(normalize(Point::new(10_000, 0), screen).0 <= 65535);
    }

    #[test]
    fn to_inputs_flags() {
        let screen = Rect::new(0, 0, 100, 100);
        let ev = [
            InputEvent::Unicode {
                unit: 0x3042,
                down: false,
            },
            InputEvent::Wheel {
                delta: -120,
                horizontal: false,
            },
        ];
        let inputs = to_inputs(&ev, screen);
        assert_eq!(inputs[0].r#type, INPUT_KEYBOARD);
        // SAFETY: to_inputs は Unicode のイベントに対して ki を設定している。
        let ki = unsafe { inputs[0].Anonymous.ki };
        assert_eq!(ki.dwFlags, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        assert_eq!(ki.wScan, 0x3042);
        // SAFETY: to_inputs は Wheel のイベントに対して mi を設定している。
        let mi = unsafe { inputs[1].Anonymous.mi };
        assert_eq!(mi.mouseData as i32, -120);
        assert_eq!(mi.dwFlags, MOUSEEVENTF_WHEEL);
    }
}
