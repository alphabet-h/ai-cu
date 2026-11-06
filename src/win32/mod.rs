//! Win32 の境界。`unsafe` はこのモジュールの下にだけ置く (CLAUDE.md の実装方針)。
//! 外へは安全な関数だけを出す。各 `unsafe` ブロックには SAFETY コメントで根拠を書く。

pub mod capture;
pub mod console;
pub mod desktop;
pub mod dpi;
pub mod fixture_window;
pub mod input;
pub mod input_mutex;
pub mod known_folder;
pub mod overlay;
pub mod process;
pub mod screen;
pub mod uia;
pub mod window;

/// NUL 終端の UTF-16 文字列。
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn last_error() -> u32 {
    // SAFETY: 引数を取らず、呼び出したスレッドの最後のエラー値を読むだけ。
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}
