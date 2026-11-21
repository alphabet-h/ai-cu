//! Win32 の境界。`unsafe` はこのモジュールの下にだけ置く (CLAUDE.md の実装方針)。
//! 外へは安全な関数だけを出す。各 `unsafe` ブロックには SAFETY コメントで根拠を書く。

pub mod capture;
pub mod clipboard;
pub mod console;
pub mod desktop;
pub mod dpi;
pub mod fixture_window;
pub mod foreground_hook;
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

#[cfg(test)]
mod tests {
    use super::wide;

    /// サロゲートペアは 2 単位のまま並び、終端の 0 は 1 つだけ (クリップボードの文の UTF-16 化に使う。D-157)
    #[test]
    fn wide_keeps_surrogate_pair_and_single_terminator() {
        assert_eq!(wide("a😀"), vec![0x61, 0xD83D, 0xDE00, 0]);
        assert_eq!(wide(""), vec![0]);
    }
}
