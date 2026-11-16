//! 画面の大きさ (物理ピクセル。DPI 認識の宣言が前提)。

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::geometry::{Point, Rect};

fn metric(index: i32) -> i32 {
    // SAFETY: 整数の索引を渡して整数を受け取るだけ。ポインタを扱わない。
    unsafe { GetSystemMetrics(index) }
}

/// 仮想デスクトップ全体 (すべてのモニタを囲む矩形。原点は負になりうる)。
pub fn virtual_screen() -> Result<Rect, String> {
    let (x, y) = (metric(SM_XVIRTUALSCREEN), metric(SM_YVIRTUALSCREEN));
    let (w, h) = (metric(SM_CXVIRTUALSCREEN), metric(SM_CYVIRTUALSCREEN));
    if w <= 0 || h <= 0 {
        return Err(format!("仮想デスクトップの大きさを取れない ({w}x{h})"));
    }
    Ok(Rect::new(x, y, w as u32, h as u32))
}

/// 主モニタ (原点は常に 0, 0)。
pub fn primary_screen() -> Result<Rect, String> {
    let (w, h) = (metric(SM_CXSCREEN), metric(SM_CYSCREEN));
    if w <= 0 || h <= 0 {
        return Err(format!("主モニタの大きさを取れない ({w}x{h})"));
    }
    Ok(Rect::new(0, 0, w as u32, h as u32))
}

/// 点 (物理ピクセルの画面座標) がどれかのモニターの上か。`MonitorFromPoint` の `MONITOR_DEFAULTTONULL` が
/// NULL でなければ真 (モニターの間の隙間と仮想画面の外は偽。D-151)。失敗の形は持たない
pub fn point_on_monitor(p: Point) -> bool {
    // SAFETY: POINT を値で渡し、ハンドル (ポインタの大きさの値) を受け取るだけ。返ったハンドルは参照も解放もしない
    // (HMONITOR は解放の要らない識別子。NULL かどうかだけを見る)。
    let monitor = unsafe { MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONULL) };
    !monitor.is_null()
}
