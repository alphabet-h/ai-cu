//! 入力デスクトップの読み取り (D-066、D-067)。読むだけで、デスクトップの切り替えや設定の変更はしない。
//! 振り分け (セーバー、セッションのロック、Default、別のデスクトップ、判定不能) は unsafe の無い側の純関数
//! `device::classify_input_desktop` が行う。ここは OS から生の値を集めるだけ。

use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK, WTS_SESSIONSTATE_UNLOCK,
    WTSFreeMemory, WTSINFOEXW, WTSQuerySessionInformationW, WTSSessionInfoEx,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, HDESK, OpenInputDesktop, UOI_NAME,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETSCREENSAVERRUNNING, SystemParametersInfoW,
};
use windows_sys::core::BOOL;

use crate::device::{InputDesktopRaw, OpenResult};

/// 入力デスクトップの材料 (`device::InputDesktopRaw`) を読む。手順:
/// 0. WTS でセッションのロックを読む (`session_locked`)。読めなければ None (D-067)
/// 1. `SystemParametersInfoW(SPI_GETSCREENSAVERRUNNING)`。失敗したら「セーバーは動いていない」として 2 に進む
/// 2. `OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS)`。null で `GetLastError() == 5` なら `AccessDenied`
/// 3. 開けたら `GetUserObjectInformationW(UOI_NAME)` で名前を読み、必ず `CloseDesktop` する
pub fn input_desktop() -> InputDesktopRaw {
    InputDesktopRaw {
        session_locked: session_locked(),
        saver_running: screensaver_running(),
        open: open_input_desktop(),
    }
}

/// `WTSQuerySessionInformationW` が返したバッファ。どの経路でも Drop で 1 回だけ `WTSFreeMemory` する
struct WtsBuffer(*mut u16);

impl Drop for WtsBuffer {
    fn drop(&mut self) {
        // SAFETY: self.0 は WTSQuerySessionInformationW が成功して返した null でないバッファで、
        // ここで 1 回だけ返す (構造体は外へ出さず、複製もしない)。
        unsafe {
            WTSFreeMemory(self.0.cast());
        }
    }
}

/// 呼び出したプロセスのセッションがロックされているかを、`WTSINFOEXW` の `Level1.SessionFlags` で読む (D-067)。
/// `WTS_SESSIONSTATE_LOCK` なら `Some(true)`、`WTS_SESSIONSTATE_UNLOCK` なら `Some(false)`。
/// 呼び出しの失敗、短いバッファ、`Level` が 1 でない、それ以外の値 (`UNKNOWN`) は None (読めない)。
/// Windows 7 / Server 2008 R2 で値が逆になる既知の不具合には対処しない (Windows 10 以降が前提)
fn session_locked() -> Option<bool> {
    let mut buf: *mut u16 = std::ptr::null_mut();
    let mut bytes: u32 = 0;
    // SAFETY: サーバーは自分のサーバー (WTS_CURRENT_SERVER_HANDLE)、セッションは呼び出したセッション。
    // buf と bytes は書き込み可能で、呼び出しの間は生きている。成功すれば buf は OS が確保したバッファを指し、
    // 下で WtsBuffer が所有して WTSFreeMemory で返す。
    let ok = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buf,
            &mut bytes,
        )
    };
    if ok == 0 || buf.is_null() {
        return None;
    }
    let owned = WtsBuffer(buf);
    if (bytes as usize) < std::mem::size_of::<WTSINFOEXW>() {
        return None;
    }
    // SAFETY: owned.0 は null でなく、少なくとも size_of::<WTSINFOEXW>() バイトの読める領域 (bytes で確かめた)。
    // WTSINFOEXW は整数と整数の配列だけの Copy な構造体で、どのビット列も有効な値になる。
    // 整列の保証を当てにしないよう read_unaligned で写す。owned は読み終わるまで生きている。
    let info = unsafe { owned.0.cast::<WTSINFOEXW>().read_unaligned() };
    if info.Level != 1 {
        return None;
    }
    // SAFETY: Level が 1 のとき、共用体の有効な腕は WTSInfoExLevel1 (Microsoft Learn「WTSINFOEXW」)。
    let flags = unsafe { info.Data.WTSInfoExLevel1.SessionFlags };
    // SessionFlags は i32、定数は u32。負の値 (UNKNOWN の -1 など) は変換に失敗して None になる
    match u32::try_from(flags).ok()? {
        WTS_SESSIONSTATE_LOCK => Some(true),
        WTS_SESSIONSTATE_UNLOCK => Some(false),
        _ => None,
    }
}

fn screensaver_running() -> bool {
    let mut running: BOOL = 0;
    // SAFETY: SPI_GETSCREENSAVERRUNNING は pvParam に BOOL を 1 つ書く (uiParam は 0、fWinIni は 0 で
    // 設定は変えない)。running は書き込み可能な BOOL で、呼び出しの間は生きている。
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETSCREENSAVERRUNNING,
            0,
            (&mut running as *mut BOOL).cast(),
            0,
        )
    };
    ok != 0 && running != 0
}

/// 開いたデスクトップのハンドル。どの経路でも Drop で 1 回だけ `CloseDesktop` する
struct DesktopHandle(HDESK);

impl Drop for DesktopHandle {
    fn drop(&mut self) {
        // SAFETY: self.0 は OpenInputDesktop が返した null でないハンドルで、ここで 1 回だけ閉じる
        // (構造体は外へ出さず、複製もしない)。閉じる失敗は判定に影響しないので結果は見ない。
        unsafe {
            CloseDesktop(self.0);
        }
    }
}

fn open_input_desktop() -> OpenResult {
    // SAFETY: 引数は値だけ (フラグ 0、継承しない、読み取りの権利だけ)。戻り値のハンドルは
    // null でなければ DesktopHandle が所有して閉じる。
    let h = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    if h.is_null() {
        let code = super::last_error();
        return if code == ERROR_ACCESS_DENIED {
            OpenResult::AccessDenied
        } else {
            OpenResult::Failed(format!("OpenInputDesktop が失敗 (GetLastError={code})"))
        };
    }
    let handle = DesktopHandle(h);
    OpenResult::Opened(object_name(&handle))
}

/// `UOI_NAME` を UTF-16 で読む。バッファが足りなければ、返された必要なバイト数で 1 回だけ取り直す
fn object_name(handle: &DesktopHandle) -> Option<String> {
    let mut buf: Vec<u16> = vec![0; 64];
    for _ in 0..2 {
        let len_bytes = u32::try_from(buf.len() * 2).ok()?;
        let mut needed: u32 = 0;
        // SAFETY: handle.0 は開いたままのデスクトップのハンドル。buf は len_bytes バイトの書き込み可能な
        // 領域で、関数は nLength を超えて書かない (足りなければ何も写さずに失敗する)。needed は
        // 書き込み可能な u32。どちらも呼び出しの間は生きている。
        let ok = unsafe {
            GetUserObjectInformationW(
                handle.0,
                UOI_NAME,
                buf.as_mut_ptr().cast(),
                len_bytes,
                &mut needed,
            )
        };
        if ok != 0 {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            return Some(String::from_utf16_lossy(&buf[..end]));
        }
        if needed <= len_bytes {
            // 長さ以外の理由で失敗した
            return None;
        }
        buf = vec![0; (needed as usize).div_ceil(2)];
    }
    None
}
