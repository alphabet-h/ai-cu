//! プロセスをまたいだ入力の排他の錠 (D-113)。名前付きミューテックスを作るか開き、待たずに取る試みと解放だけを行う。
//! 待つのは実行器 (`run::Executor`) で、ここは待たない (`WaitForSingleObject` の待ち時間は 0)。
//!
//! 名前は `Local\` の名前空間 (セッションごと) に置く。入力が届く範囲 (そのセッションの入力デスクトップ) と一致する。
//! ミューテックスは持ったスレッドでしか解けない (`ReleaseMutex` は持っていないスレッドからは失敗する)。
//! 同じスレッドが持ったまま取り直すと成功し、取った回数だけ解く必要がある (再帰)。
//! 持ち主のスレッドが解かずに終わると「放棄」になり、次に取ったスレッドに `WAIT_ABANDONED` で渡る。

use std::io;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, HANDLE, WAIT_ABANDONED, WAIT_FAILED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexExW, MUTEX_MODIFY_STATE, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE,
    WaitForSingleObject,
};

use super::wide;

/// 既定の錠の名前 (D-113)。同じセッションの ai-cu はすべてこの名前の錠を取り合う。
/// テストはこの名前を使わない (利用者が常駐させている ai-cu の錠を握らない)
pub const INPUT_LOCK_NAME: &str = "Local\\ai-cu-input";

/// 名前付きミューテックスのハンドル。`Drop` で閉じる。
///
/// `Send` にしない (生のハンドルを持つので自動では `Send` にならない)。持ったスレッドでしか解けないので、
/// 作ったスレッドの外へ動かさない
pub struct InputMutex {
    handle: HANDLE,
}

impl InputMutex {
    /// 名前のミューテックスを作るか、既にあれば開く (`CreateMutexExW`)。初めの持ち主にはならない。
    /// 求める権限は `SYNCHRONIZE | MUTEX_MODIFY_STATE` だけ (`CreateMutexW` は既存の物を `MUTEX_ALL_ACCESS` で
    /// 開くので、絞る方が開ける場合が広い)。待つのに要るのは `SYNCHRONIZE`。`ReleaseMutex` に要る権限は文書に無く、
    /// `MUTEX_MODIFY_STATE` は文書では予約 (Reserved for future use) だが、慣例として付けている (microsoft-learn の
    /// Synchronization Object Security and Access Rights。win32-reviewer の指摘)。明示のセキュリティ記述子は付けない。
    /// 作れない・開けない (権限の違うプロセスが先に作った、同じ名前の別の種類の物がある) ときは `GetLastError` の Err
    pub fn open(name: &str) -> io::Result<InputMutex> {
        let wname = wide(name);
        // SAFETY: wname は NUL 終端の UTF-16 で、呼び出しの間は生きている。セキュリティ属性は null (既定)、
        // dwflags は 0 (初めの持ち主にならない)。返り値は所有するハンドルか null で、null なら下で Err にする。
        let handle = unsafe {
            CreateMutexExW(
                std::ptr::null(),
                wname.as_ptr(),
                0,
                SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE,
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(InputMutex { handle })
    }

    /// 待たずに取る試み (`WaitForSingleObject(h, 0)`)。`WAIT_OBJECT_0` (取れた) と `WAIT_ABANDONED`
    /// (前の持ち主が解かずに終わった。持ち主はもういないので取れたとみなす) は真、`WAIT_TIMEOUT` (別の持ち主がいる) は偽、
    /// `WAIT_FAILED` とそれ以外の値は Err
    pub fn try_lock(&self) -> io::Result<bool> {
        // SAFETY: self.handle は open が返した有効なハンドルで、self が生きている間は閉じない。待ち時間 0 で戻る。
        let r = unsafe { WaitForSingleObject(self.handle, 0) };
        match r {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            other => Err(io::Error::other(format!(
                "WaitForSingleObject が想定しない値を返した: {other:#x}"
            ))),
        }
    }

    /// 1 回分解く (`ReleaseMutex`)。呼んだスレッドが持っていなければ失敗する (Err)
    pub fn unlock(&self) -> io::Result<()> {
        // SAFETY: self.handle は open が返した有効なハンドルで、self が生きている間は閉じない。
        let ok = unsafe { ReleaseMutex(self.handle) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for InputMutex {
    fn drop(&mut self) {
        // SAFETY: self.handle は open が返した有効なハンドルで、ここで 1 回だけ閉じる (構造体は複製しない)。
        // 持ったまま閉じても、スレッドが終わるまでは持ち主のまま (終われば放棄になる)。
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// 作れない・開けないエラーが `ERROR_ACCESS_DENIED` (権限の違うプロセスが先に作った錠) か
pub fn is_access_denied(e: &io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
}

/// 作れない・開けないエラーが `ERROR_INVALID_HANDLE` (同じ名前のミューテックスでない物が先にある。
/// CreateMutexExW の文書) か
pub fn is_invalid_handle(e: &io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_INVALID_HANDLE as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// テストごとの錠の名前 (pid と連番を入れる。既定の `INPUT_LOCK_NAME` は使わない)
    fn test_name() -> String {
        format!(
            "Local\\ai-cu-input-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// 別のスレッドが持っている間は取れず (偽)、そのスレッドが解いた後は取れる (真)
    #[test]
    fn held_by_other_thread_then_free() {
        let name = test_name();
        let mine = InputMutex::open(&name).expect("開ける");
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let other_name = name.clone();
        let other = std::thread::spawn(move || {
            let m = InputMutex::open(&other_name).expect("開ける");
            assert!(m.try_lock().expect("試める"), "空いている錠が取れない");
            held_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            m.unlock().expect("持ったスレッドで解ける");
        });
        held_rx.recv().unwrap();
        assert!(
            !mine.try_lock().expect("試める"),
            "別のスレッドが持っている錠が取れた"
        );
        go_tx.send(()).unwrap();
        other.join().expect("相手のスレッド");
        assert!(mine.try_lock().expect("試める"), "解かれた錠が取れない");
        mine.unlock().expect("解ける");
    }

    /// 持ったまま終わったスレッドの錠 (放棄) は取れる。主が先に名前のハンドルを開いておき、物が消えて
    /// 作り直される経路を通らないようにする
    #[test]
    fn abandoned_by_exited_thread_is_acquired() {
        let name = test_name();
        let mine = InputMutex::open(&name).expect("開ける");
        let other_name = name.clone();
        std::thread::spawn(move || {
            let m = InputMutex::open(&other_name).expect("開ける");
            assert!(m.try_lock().expect("試める"), "空いている錠が取れない");
            // 解かずに終わる (ハンドルは閉じるが、持ち主のスレッドが終わるので放棄になる)
        })
        .join()
        .expect("相手のスレッド");
        assert!(mine.try_lock().expect("試める"), "放棄された錠が取れない");
        mine.unlock().expect("解ける");
    }

    /// 同じスレッドは持ったまま取り直せ (再帰)、取った回数だけ解ける
    #[test]
    fn same_thread_is_recursive() {
        let name = test_name();
        let m = InputMutex::open(&name).expect("開ける");
        assert!(m.try_lock().expect("試める"));
        assert!(m.try_lock().expect("試める"), "持っている錠が取り直せない");
        m.unlock().expect("1 回目を解ける");
        m.unlock().expect("2 回目を解ける");
        assert!(m.unlock().is_err(), "持っていない錠が解けた");
    }
}
