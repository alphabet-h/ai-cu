//! 既知のフォルダ (RoamingAppData) の場所。環境変数の `APPDATA` は使わない (D-029)。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{
    FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows_sys::core::PWSTR;

pub fn roaming_app_data() -> Result<PathBuf, String> {
    let mut raw: PWSTR = std::ptr::null_mut();
    // SAFETY: rfid は静的な GUID への有効なポインタ。htoken は null (現在のユーザー)。
    // ppszpath は書き込み先として有効な PWSTR 変数を指す。成否にかかわらず、返された領域は CoTaskMemFree で解放する。
    let hr = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_RoamingAppData,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut raw,
        )
    };
    if hr < 0 || raw.is_null() {
        if !raw.is_null() {
            // SAFETY: SHGetKnownFolderPath が確保した領域で、まだ解放していない。
            unsafe { CoTaskMemFree(raw as *const _) };
        }
        return Err(format!("SHGetKnownFolderPath が失敗 (HRESULT={hr:#010x})"));
    }
    // SAFETY: 成功時、raw は NUL 終端の UTF-16 文字列を指す。NUL までを数える間、領域は解放されない。
    let len = unsafe {
        let mut n = 0usize;
        while *raw.add(n) != 0 {
            n += 1;
        }
        n
    };
    // SAFETY: raw から len 個の u16 は上で読めることを確かめた範囲。複製してから解放する。
    let path = unsafe { OsString::from_wide(std::slice::from_raw_parts(raw, len)) };
    // SAFETY: SHGetKnownFolderPath が確保した領域を 1 回だけ解放する。以後 raw は使わない。
    unsafe { CoTaskMemFree(raw as *const _) };
    Ok(PathBuf::from(path))
}
