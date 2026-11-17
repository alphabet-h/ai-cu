//! クリップボードに文字を置く (D-157)。形式は `CF_UNICODETEXT` だけで、Win+V の履歴とクラウドの同期から外す印を
//! 3 つ重ねて置く。読むのは実機テストの後始末 (置く前の文の写しと、印が付いたかの確かめ) だけで、製品の操作に
//! 「読む」は無い。
//!
//! 置き方の順 (D-157 の「自分で決めたこと」): 印の登録と 4 つの置き場の確保を `OpenClipboard` の前にすべて済ませ、
//! そこで失敗すれば開かずに Err (中身は変わらない)。開いた後は印を先に、文を最後に置き、`EmptyClipboard` の後の
//! どこかで失敗したら空にし直して `Emptied` (人の元の中身は失われた)。`OpenClipboard` には自前の message-only の窓を
//! 渡す (NULL で開くと `EmptyClipboard` が持ち主を NULL にし、`SetClipboardData` が失敗する。OpenClipboard と
//! SetClipboardData の Remarks)。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, GlobalFree, HGLOBAL, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_MESSAGE, RegisterClassExW, WNDCLASSEXW,
};

use crate::device::ClipboardSet;

/// 標準の形式 `CF_UNICODETEXT` の値。windows-sys 0.61.2 ではこの定数が feature `Win32_System_Ole` にあり、
/// 使う feature (`Win32_System_DataExchange`、`Win32_System_Memory`) の外なのでここに持つ。
/// 値は Microsoft Learn の「Standard Clipboard Formats」で 13 に決まっている (D-157)
const CF_UNICODETEXT: u32 = 13;

/// Win+V の履歴とクラウドの同期から外す印の形式の名前 (Microsoft Learn の「Clipboard Formats」の
/// 「Cloud Clipboard and Clipboard History Formats」)。1 つ目だけで両方から外れると書かれているが、版や実装の差に
/// 備えて明示の 2 つも重ねる。中身はどれも DWORD の 0 (D-157)
pub const HISTORY_FORMATS: [&str; 3] = [
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

/// `OpenClipboard` に渡す message-only の窓のクラス名
const OWNER_CLASS: &str = "ai_cu_clipboard_owner";

/// クラスを登録できたか (登録できた後は呼ばない。失敗は覚えず、次の呼び出しで試し直す)
static OWNER_CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// 読み取りで `OpenClipboard` を試し直す間隔と上限 (置いた直後は履歴のサービスなどが短く開くため)
const READ_RETRY_MS: u64 = 50;
const READ_RETRY_LIMIT_MS: u64 = 1000;

/// 確保した置き場。`Drop` で解放する。`SetClipboardData` が受け取ったら持ち主は OS に移るので `release` で手放す
struct Global(HGLOBAL);

impl Global {
    /// `GMEM_MOVEABLE` で `bytes` の大きさを確保して写す (SetClipboardData の文書の求め)
    fn with_bytes(bytes: &[u8]) -> Result<Global, String> {
        // SAFETY: 旗と大きさを渡すだけ。失敗は null で返る。
        let h = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
        if h.is_null() {
            return Err(format!(
                "GlobalAlloc ({} バイト) が失敗 (GetLastError={})",
                bytes.len(),
                super::last_error()
            ));
        }
        let g = Global(h);
        // SAFETY: g.0 はいま確保した有効な可動のメモリで、まだ誰にも渡していない。
        let p = unsafe { GlobalLock(g.0) };
        if p.is_null() {
            return Err(format!(
                "GlobalLock が失敗 (GetLastError={})",
                super::last_error()
            ));
        }
        // SAFETY: p は GlobalLock が返した、少なくとも bytes.len() バイトの書ける領域 (確保した大きさ)。
        //         bytes とは別の領域なので重ならない。
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len()) };
        // SAFETY: 上で 1 回だけ錠を取った g.0 を 1 回解く。戻り値は錠の数が 0 になれば 0 (失敗ではない) なので見ない。
        unsafe { GlobalUnlock(g.0) };
        Ok(g)
    }

    /// 持ち主が OS に移った (SetClipboardData が成功した)。解放しない
    fn release(mut self) {
        self.0 = std::ptr::null_mut();
    }
}

impl Drop for Global {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 はこのプロセスが確保し、OS に渡していない (渡したものは release で null にした)
            //         メモリで、錠は解いてある。1 回だけ解放する。
            unsafe { GlobalFree(self.0) };
        }
    }
}

/// `OpenClipboard` に渡す message-only の窓。`Drop` で壊す (作ったスレッドで、同じ関数の中で壊す)
struct OwnerWindow(HWND);

impl OwnerWindow {
    fn create() -> Result<OwnerWindow, String> {
        let class = super::wide(OWNER_CLASS);
        // SAFETY: null を渡すと、呼び出したプロセスの実行ファイルのモジュールのハンドルが返る。プロセスの実行ファイルは
        //         必ず読み込まれているので、null の引数ではこの呼び出しは失敗しない (GetModuleHandleW の文書)。
        let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
        if !OWNER_CLASS_REGISTERED.load(Ordering::Acquire) {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: hinst,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // SAFETY: wc は初期化済みで cbSize が正しく、class は登録の間生きている (クラス名は登録時に複製される)。
            if unsafe { RegisterClassExW(&wc) } == 0 {
                let err = super::last_error();
                if err != ERROR_CLASS_ALREADY_EXISTS {
                    return Err(format!("RegisterClassExW が失敗 (GetLastError={err})"));
                }
            }
            OWNER_CLASS_REGISTERED.store(true, Ordering::Release);
        }
        // SAFETY: class は NUL 終端の UTF-16 で呼び出しの間生きている。親は HWND_MESSAGE (message-only の窓。見えず、
        //         前面も Z 順も変えない)。題名・メニュー・作成の引数は null。
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                hinst,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(format!(
                "クリップボードの持ち主の窓 (CreateWindowExW) が失敗 (GetLastError={})",
                super::last_error()
            ));
        }
        Ok(OwnerWindow(hwnd))
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        // SAFETY: self.0 はこのスレッドが create で作った窓で、作ったスレッドから 1 回だけ壊す。
        unsafe { DestroyWindow(self.0) };
    }
}

/// 開いたクリップボード。`Drop` で閉じる (成否によらず。持ち主の窓より先に落ちるよう、窓より後に作る)
struct OpenClipboardGuard;

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: このスレッドが OpenClipboard に成功した後にだけ作る値で、1 回だけ閉じる。
        unsafe { CloseClipboard() };
    }
}

/// 形式の名前を登録して値を得る (既にあれば同じ値が返る)。0 なら Err
fn register_format(name: &str) -> Result<u32, String> {
    let w = super::wide(name);
    // SAFETY: w は NUL 終端の UTF-16 で呼び出しの間生きている。
    let id = unsafe { RegisterClipboardFormatW(w.as_ptr()) };
    if id == 0 {
        return Err(format!(
            "RegisterClipboardFormatW({name}) が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    Ok(id)
}

/// クリップボードに `text_crlf` (改行は呼ぶ側が CRLF にそろえたもの) を `CF_UNICODETEXT` で置き、履歴とクラウドの
/// 同期から外す印 (`HISTORY_FORMATS`、中身は DWORD の 0) を付ける (D-157)。
///
/// - 開く前の失敗 (印の登録、置き場の確保、持ち主の窓) は `Err` (中身は変わらない)
/// - 他のアプリが開いていて `OpenClipboard` が失敗したら `Ok(Busy(GetLastError))` (中身は変わらない。試し直しは実行器が行う)
/// - `EmptyClipboard` 自身の失敗は `Err`。失敗したときに中身が残るかは文書に無いので、変わっていない扱いにする (推定)
/// - `EmptyClipboard` の後に `SetClipboardData` が失敗したら、空にし直して `Ok(Emptied(理由))` (人の元の中身は失われた)
/// - 成否によらず `CloseClipboard` の後に持ち主の窓を壊す
///
/// 文の長さ (1〜65536 文字) と制御文字 (改行とタブ以外。U+0000 を含む) は呼ぶ側 (validate) で検査済みの前提。
/// U+0000 が混じると `CF_UNICODETEXT` がそこで切れるので、debug ビルドでは断言する
pub fn set_text(text_crlf: &str) -> Result<ClipboardSet, String> {
    debug_assert!(
        !text_crlf.contains('\0'),
        "クリップボードに置く文に U+0000 が混じっている (validate で断るはず)"
    );
    // (1) 印の登録
    let mut marks = Vec::with_capacity(HISTORY_FORMATS.len());
    for name in HISTORY_FORMATS {
        marks.push((name, register_format(name)?));
    }
    // (2) 置き場の確保 (文は UTF-16 と終端の 0、印は DWORD の 0)。途中で失敗すれば確保済みは Drop で解放する
    let units = super::wide(text_crlf);
    let text_bytes: Vec<u8> = units.iter().flat_map(|u| u.to_ne_bytes()).collect();
    let mut mark_globals = Vec::with_capacity(marks.len());
    for _ in &marks {
        mark_globals.push(Some(Global::with_bytes(&0u32.to_ne_bytes())?));
    }
    let mut text_global = Some(Global::with_bytes(&text_bytes)?);
    // (3) 持ち主の窓
    let owner = OwnerWindow::create()?;
    // (4) 開く。開けなければ Busy と GetLastError (置き場と窓は Drop で片付く)
    // SAFETY: owner.0 はこのスレッドが作った有効な窓。
    if unsafe { OpenClipboard(owner.0) } == 0 {
        return Ok(ClipboardSet::Busy(super::last_error()));
    }
    let opened = OpenClipboardGuard;
    // (5) 空にする (持ち主が owner の窓になる)
    // SAFETY: このスレッドがクリップボードを開いている。
    if unsafe { EmptyClipboard() } == 0 {
        let err = super::last_error();
        drop(opened);
        return Err(format!(
            "EmptyClipboard が失敗 (GetLastError={err})。クリップボードは書き換えていない (推定)"
        ));
    }
    // (6) 印を先に、文を最後に置く
    let mut failure: Option<String> = None;
    for ((name, id), slot) in marks.iter().zip(mark_globals.iter_mut()) {
        let g = slot.take().expect("印の置き場は 1 回だけ渡す");
        if let Err(e) = put(*id, g) {
            failure = Some(format!("印 {name} を置けない: {e}"));
            break;
        }
    }
    if failure.is_none() {
        let g = text_global.take().expect("文の置き場は 1 回だけ渡す");
        if let Err(e) = put(CF_UNICODETEXT, g) {
            failure = Some(format!("文 (CF_UNICODETEXT) を置けない: {e}"));
        }
    }
    let result = match failure {
        None => Ok(ClipboardSet::Set),
        Some(reason) => {
            // 置けた印だけが残らないよう空にし直す (印の無い文を残さない。文は最後なので残っていない)
            // SAFETY: このスレッドがクリップボードを開いている。
            let reason = if unsafe { EmptyClipboard() } == 0 {
                format!(
                    "{reason}。空にし直すのも失敗した (GetLastError={})",
                    super::last_error()
                )
            } else {
                reason
            };
            Ok(ClipboardSet::Emptied(reason))
        }
    };
    // (7) 閉じてから窓を壊す (opened を owner より先に落とす)。まだ渡していない置き場はこの後の Drop で解放する。
    //     CloseClipboard と DestroyWindow の間には何も挟まない (閉じた直後に窓を壊し、持ち主の窓が開いたまま
    //     メッセージを受ける間を作らない)
    drop(opened);
    drop(owner);
    result
}

/// 置き場を 1 つ渡す。成功したら持ち主は OS に移る (解放しない)。失敗なら `g` は Drop で解放される
fn put(format: u32, g: Global) -> Result<(), String> {
    // SAFETY: このスレッドがクリップボードを開いて空にした持ち主で、g.0 は GMEM_MOVEABLE で確保して錠を解いた
    //         メモリ。成功したら持ち主は OS に移るので、この後は書かず解放もしない (release)。
    let r = unsafe { SetClipboardData(format, g.0) };
    if r.is_null() {
        return Err(format!(
            "SetClipboardData が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    g.release();
    Ok(())
}

/// クリップボードの `CF_UNICODETEXT` の文と、`HISTORY_FORMATS` の各印があるかを読む (D-157)。
/// **実機テスト (tests/fixture_real.rs) の後始末と確かめからだけ呼ぶ。** 製品の操作に「読む」は無い。
/// 開くのは `OpenClipboard(NULL)` で、50 ms おきに上限 1000 ms まで試し直す (置いた直後は履歴のサービスなどが
/// 短く開くため)。文が無ければ `None`
#[doc(hidden)]
pub fn read_text_and_formats() -> Result<(Option<String>, Vec<bool>), String> {
    let mut ids = Vec::with_capacity(HISTORY_FORMATS.len());
    for name in HISTORY_FORMATS {
        ids.push(register_format(name)?);
    }
    let start = Instant::now();
    loop {
        // SAFETY: NULL は「今のタスクに結びつけて開く」の意味で、読むだけなので持ち主は要らない。
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            break;
        }
        if start.elapsed() >= Duration::from_millis(READ_RETRY_LIMIT_MS) {
            return Err(format!(
                "クリップボードを {READ_RETRY_LIMIT_MS} ms 開けなかった (GetLastError={})",
                super::last_error()
            ));
        }
        std::thread::sleep(Duration::from_millis(READ_RETRY_MS));
    }
    let _opened = OpenClipboardGuard;
    let formats = ids
        .iter()
        // SAFETY: 形式の値を渡すだけ。
        .map(|&id| unsafe { IsClipboardFormatAvailable(id) } != 0)
        .collect();
    // SAFETY: このスレッドがクリップボードを開いている。返るハンドルは OS の持ち物で、閉じるまで読める。
    let h = unsafe { GetClipboardData(CF_UNICODETEXT) };
    if h.is_null() {
        return Ok((None, formats));
    }
    // SAFETY: h は GetClipboardData が返した有効なメモリのハンドル。
    let size = unsafe { GlobalSize(h) };
    // SAFETY: 同上。錠を取り、読み終えたら解く。
    let p = unsafe { GlobalLock(h) };
    if p.is_null() {
        return Err(format!(
            "GlobalLock が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    let len = size / 2;
    // SAFETY: p は GlobalLock が返した、少なくとも size バイト読める領域。u16 の境界に並ぶ (GlobalAlloc の領域は
    //         8 バイト境界)。読むのは size / 2 単位まで。
    let all = unsafe { std::slice::from_raw_parts(p.cast::<u16>(), len) };
    let end = all.iter().position(|&u| u == 0).unwrap_or(len);
    let text = String::from_utf16_lossy(&all[..end]);
    // SAFETY: 上で 1 回取った錠を 1 回解く。
    unsafe { GlobalUnlock(h) };
    Ok((Some(text), formats))
}
