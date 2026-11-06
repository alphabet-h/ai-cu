//! DPI 認識の宣言。座標を物理ピクセルにそろえるため、最初のウィンドウを作る前に 1 回だけ呼ぶ。
//! テストの実行ファイルは `main` を通らないので、テスト側からも呼ぶ (docs/design-notes.md)。

use std::sync::OnceLock;

use windows_sys::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};

const ERROR_ACCESS_DENIED: u32 = 5;

static RESULT: OnceLock<Result<(), String>> = OnceLock::new();

/// Per-Monitor V2 を宣言し、実際にそうなっていることを確かめる。何度呼んでも 1 回だけ実行する。
pub fn ensure_per_monitor_v2() -> Result<(), String> {
    RESULT.get_or_init(declare).clone()
}

fn declare() -> Result<(), String> {
    // SAFETY: 引数は windows-sys が定める擬似ハンドルの定数で、ポインタとして参照されない。
    let ok = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if ok == 0 {
        let err = super::last_error();
        // 既にプロセスの DPI 認識が設定済み (マニフェストや先の呼び出し) なら ERROR_ACCESS_DENIED になる。下で確かめる
        if err != ERROR_ACCESS_DENIED {
            return Err(format!(
                "SetProcessDpiAwarenessContext が失敗 (GetLastError={err})"
            ));
        }
    }
    if !thread_is_per_monitor_v2() {
        return Err("DPI 認識が Per-Monitor V2 になっていない".into());
    }
    Ok(())
}

/// 呼び出したスレッドの DPI 認識が Per-Monitor V2 か (読むだけ)。
/// スレッドごとに上書きできるので、窓を作る別のスレッドはそのスレッドで確かめる (D-093、枠のスレッド)。
pub fn thread_is_per_monitor_v2() -> bool {
    // SAFETY: 引数なし。呼び出したスレッドの DPI 認識のコンテキスト (擬似ハンドル) を返すだけ。
    let current = unsafe { GetThreadDpiAwarenessContext() };
    // SAFETY: どちらの引数も DPI 認識のコンテキストの値で、関数は値を比べるだけ。
    let same = unsafe {
        AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
    };
    same != 0
}
