//! 前面の切り替わりの記録 (D-166)。常駐の MCP の間だけ、見張り (`win32::foreground_hook`) が
//! 前面のルートの窓が移るたびに 1 件ずつ足し、`status` が `foreground.since_ms` と
//! `foreground_history` に読む。ここは Win32 に触れない純粋な部分で、上限、重複の落とし方、
//! 見張りの状態 (一方向に移る) を持つ。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::device::WindowInfo;

/// 残す件数の上限。溢れたら古い方から捨てて `dropped` を数える
pub const CAP: usize = 64;

/// 前面が移った 1 件。`window` が None は前面が無い (ロックの直後など)
#[derive(Debug, Clone)]
pub struct ForegroundEntry {
    /// 記録した時刻 (単調な時計。`ago_ms` と `since_ms` の比較に使う)
    pub at: Instant,
    /// 記録した壁の時刻 (UNIX_EPOCH からの ms。表示に使う)
    pub at_unix_ms: u64,
    pub window: Option<WindowInfo>,
    /// 見張りの起動の直後に読んだ前面か (いつから前面かは分からない)
    pub initial: bool,
}

/// 見張りの状態。`Starting → Running → Failed`、`Starting → Failed` の一方向にだけ移る
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchState {
    /// まだ起動の直後の前面 (initial) を記録していない
    Starting,
    /// 記録している
    Running,
    /// 止まった (立てられない、止めた、スレッドが終わった)。理由を持つ。ここから戻らない
    Failed(String),
}

/// 読む時点の記録の複製
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 古い順
    pub entries: Vec<ForegroundEntry>,
    /// 上限を超えて捨てた件数
    pub dropped: u64,
    pub state: WatchState,
}

#[derive(Debug)]
struct Inner {
    entries: VecDeque<ForegroundEntry>,
    dropped: u64,
    state: WatchState,
}

/// 前面の切り替わりの記録。見張りのスレッドが書き、`status` が読む
#[derive(Debug)]
pub struct ForegroundLog {
    inner: Mutex<Inner>,
}

impl Default for ForegroundLog {
    fn default() -> Self {
        Self::new()
    }
}

impl ForegroundLog {
    pub fn new() -> Self {
        ForegroundLog {
            inner: Mutex::new(Inner {
                entries: VecDeque::with_capacity(CAP),
                dropped: 0,
                state: WatchState::Starting,
            }),
        }
    }

    /// 毒の入った Mutex も読み続ける (記録の失敗で `status` を落とさない)
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 1 件を足す。最後の件と hwnd が同じ (前面が無い件どうしも同じ) なら足さず false。
    /// 上限を超えたら先頭を捨てて `dropped` を数える
    pub fn record(
        &self,
        window: Option<WindowInfo>,
        at: Instant,
        at_unix_ms: u64,
        initial: bool,
    ) -> bool {
        let mut g = self.lock();
        if let Some(last) = g.entries.back() {
            let same = match (&last.window, &window) {
                (None, None) => true,
                (Some(a), Some(b)) => a.hwnd == b.hwnd,
                _ => false,
            };
            if same {
                return false;
            }
        }
        g.entries.push_back(ForegroundEntry {
            at,
            at_unix_ms,
            window,
            initial,
        });
        while g.entries.len() > CAP {
            g.entries.pop_front();
            g.dropped += 1;
        }
        true
    }

    /// 状態を移す。`Failed` からは戻らず、`Running` から `Starting` へも戻らない (その移りは無視する)
    pub fn set_state(&self, s: WatchState) {
        let mut g = self.lock();
        let backward = matches!(
            (&g.state, &s),
            (WatchState::Failed(_), _) | (WatchState::Running, WatchState::Starting)
        );
        if !backward {
            g.state = s;
        }
    }

    pub fn state(&self) -> WatchState {
        self.lock().state.clone()
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        Snapshot {
            entries: g.entries.iter().cloned().collect(),
            dropped: g.dropped,
            state: g.state.clone(),
        }
    }
}

/// 今の壁の時刻 (UNIX_EPOCH からの ms)。時計が UNIX_EPOCH より前なら 0
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn win(hwnd: usize) -> WindowInfo {
        let mut w = testing::window("app.exe", "AppClass");
        w.hwnd = hwnd;
        w
    }

    fn hwnds(s: &Snapshot) -> Vec<Option<usize>> {
        s.entries
            .iter()
            .map(|e| e.window.as_ref().map(|w| w.hwnd))
            .collect()
    }

    #[test]
    fn record_skips_same_hwnd_and_repeated_none() {
        let log = ForegroundLog::new();
        let t = Instant::now();
        assert!(log.record(Some(win(1)), t, 10, true));
        assert!(!log.record(Some(win(1)), t, 11, false));
        assert!(log.record(None, t, 12, false));
        assert!(!log.record(None, t, 13, false));
        assert!(log.record(Some(win(2)), t, 14, false));
        assert!(log.record(Some(win(1)), t, 15, false));
        let s = log.snapshot();
        assert_eq!(hwnds(&s), vec![Some(1), None, Some(2), Some(1)]);
        assert_eq!(
            s.entries.iter().map(|e| e.at_unix_ms).collect::<Vec<_>>(),
            vec![10, 12, 14, 15]
        );
        assert_eq!(s.dropped, 0);
    }

    #[test]
    fn record_drops_oldest_beyond_cap_and_counts_dropped() {
        let log = ForegroundLog::new();
        let t = Instant::now();
        for i in 0..=CAP {
            assert!(log.record(Some(win(100 + i)), t, i as u64, false));
        }
        let s = log.snapshot();
        assert_eq!(s.entries.len(), CAP);
        assert_eq!(s.dropped, 1);
        assert_eq!(s.entries[0].window.as_ref().map(|w| w.hwnd), Some(101));
        assert_eq!(
            s.entries
                .last()
                .and_then(|e| e.window.as_ref())
                .map(|w| w.hwnd),
            Some(100 + CAP)
        );
    }

    #[test]
    fn record_keeps_initial_flag() {
        let log = ForegroundLog::new();
        let t = Instant::now();
        log.record(Some(win(1)), t, 1, true);
        log.record(Some(win(2)), t, 2, false);
        let s = log.snapshot();
        assert_eq!(
            s.entries.iter().map(|e| e.initial).collect::<Vec<_>>(),
            vec![true, false]
        );
    }

    #[test]
    fn state_starts_as_starting_and_can_fail() {
        let log = ForegroundLog::default();
        assert_eq!(log.state(), WatchState::Starting);
        assert_eq!(log.snapshot().state, WatchState::Starting);
        assert!(log.snapshot().entries.is_empty());
        log.set_state(WatchState::Running);
        assert_eq!(log.state(), WatchState::Running);
        log.set_state(WatchState::Failed("止めた".into()));
        assert_eq!(log.snapshot().state, WatchState::Failed("止めた".into()));

        let direct = ForegroundLog::new();
        direct.set_state(WatchState::Failed("立てられない".into()));
        assert_eq!(direct.state(), WatchState::Failed("立てられない".into()));
    }

    #[test]
    fn state_never_leaves_failed() {
        let log = ForegroundLog::new();
        log.set_state(WatchState::Failed("一つ目".into()));
        log.set_state(WatchState::Running);
        assert_eq!(log.state(), WatchState::Failed("一つ目".into()));
        log.set_state(WatchState::Starting);
        log.set_state(WatchState::Failed("二つ目".into()));
        assert_eq!(log.state(), WatchState::Failed("一つ目".into()));

        let running = ForegroundLog::new();
        running.set_state(WatchState::Running);
        running.set_state(WatchState::Starting);
        assert_eq!(running.state(), WatchState::Running);
    }

    #[test]
    fn now_unix_ms_is_after_2020() {
        assert!(now_unix_ms() > 1_577_836_800_000);
    }
}
