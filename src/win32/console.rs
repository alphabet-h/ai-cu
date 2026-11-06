//! 標準入出力が本物のコンソールか。mintty (Git Bash) の pty は `IsTerminal` では端末に見えるが、
//! crossterm はコンソールの入力バッファを読むので、そこではキーを受け取れない (D-023 の補足、D-038)。

use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

fn is_console(which: STD_HANDLE) -> bool {
    // SAFETY: 標準ハンドルの識別子を渡して、ハンドル (無ければ null か INVALID_HANDLE_VALUE) を受け取るだけ。
    let h = unsafe { GetStdHandle(which) };
    if h.is_null() || h as isize == -1 {
        return false;
    }
    let mut mode = 0u32;
    // SAFETY: h は GetStdHandle が返したハンドル (閉じない)。mode は書き込み可能な u32。
    // コンソールでなければ失敗して 0 が返るだけ。
    unsafe { GetConsoleMode(h, &mut mode) != 0 }
}

pub fn stdin_is_console() -> bool {
    is_console(STD_INPUT_HANDLE)
}

pub fn stdout_is_console() -> bool {
    is_console(STD_OUTPUT_HANDLE)
}
