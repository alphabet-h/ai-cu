//! 画面の矩形の撮影 (GDI の BitBlt、D-034)。全画面ではなく、指定の矩形だけを撮る。

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC,
    HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};

use crate::device::Image;
use crate::geometry::Rect;

/// 1 回に撮れる最大の画素数 (8K の 4 枚分)。巨大な確保で落ちないように
const MAX_PIXELS: u64 = 7680 * 4320 * 4;

struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: GetDC(null) で得た DC を、同じ hwnd (null) で 1 回だけ解放する。
        unsafe { ReleaseDC(std::ptr::null_mut(), self.0) };
    }
}

struct MemDc(HDC);

impl Drop for MemDc {
    fn drop(&mut self) {
        // SAFETY: CreateCompatibleDC で作った DC を 1 回だけ削除する。選択中のビットマップは先に戻してある。
        unsafe { DeleteDC(self.0) };
    }
}

struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: CreateCompatibleBitmap で作ったビットマップを 1 回だけ削除する。DC からは選択を外してある。
        unsafe { DeleteObject(self.0 as HGDIOBJ) };
    }
}

pub fn capture(rect: Rect) -> Result<Image, String> {
    if rect.is_empty() {
        return Err("撮影の矩形が空".into());
    }
    if u64::from(rect.w) * u64::from(rect.h) > MAX_PIXELS
        || rect.w > i32::MAX as u32
        || rect.h > i32::MAX as u32
    {
        return Err(format!("撮影の矩形が大きすぎる: {rect}"));
    }
    let (w, h) = (rect.w as i32, rect.h as i32);
    // SAFETY: null を渡すと画面全体の DC が返る。失敗なら null。
    let screen = unsafe { GetDC(std::ptr::null_mut()) };
    if screen.is_null() {
        return Err("GetDC が失敗".into());
    }
    let screen = ScreenDc(screen);
    // SAFETY: screen.0 は有効な DC。
    let mem = unsafe { CreateCompatibleDC(screen.0) };
    if mem.is_null() {
        return Err("CreateCompatibleDC が失敗".into());
    }
    let mem = MemDc(mem);
    // SAFETY: screen.0 は有効な DC。w と h は正の値。
    let bmp = unsafe { CreateCompatibleBitmap(screen.0, w, h) };
    if bmp.is_null() {
        return Err("CreateCompatibleBitmap が失敗".into());
    }
    let bmp = Bitmap(bmp);
    // SAFETY: mem.0 と bmp.0 は有効。戻り値の旧オブジェクトは、削除の前に選択し直す。
    let old = unsafe { SelectObject(mem.0, bmp.0 as HGDIOBJ) };
    // SAFETY: どちらの DC も有効。転送先は (0,0)-(w,h) でビットマップの大きさと一致する。
    let blt = unsafe {
        BitBlt(
            mem.0,
            0,
            0,
            w,
            h,
            screen.0,
            rect.x,
            rect.y,
            SRCCOPY | CAPTUREBLT,
        )
    };
    // SAFETY: 元のオブジェクトを選択し直して、bmp を DC から外す (GetDIBits と削除の前提)。
    unsafe { SelectObject(mem.0, old) };
    if blt == 0 {
        return Err(format!(
            "BitBlt が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // 負の高さで上から下の並びにする
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (rect.w as usize) * (rect.h as usize) * 4];
    // SAFETY: bmp.0 はどの DC にも選択されていない有効なビットマップ。bgra は 32 ビット x w x h の大きさ
    // (biBitCount=32 なら行の詰め物は無い) で、info はその形を正しく記述している。
    let lines = unsafe {
        GetDIBits(
            screen.0,
            bmp.0,
            0,
            rect.h,
            bgra.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if lines != h {
        return Err(format!(
            "GetDIBits が {lines} 行しか返さなかった (期待 {h})"
        ));
    }
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 0xFF;
    }
    Ok(Image {
        width: rect.w,
        height: rect.h,
        rgba: bgra,
    })
}
