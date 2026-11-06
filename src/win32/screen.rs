//! 画面の大きさ (物理ピクセル。DPI 認識の宣言が前提)。

use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::geometry::Rect;

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
