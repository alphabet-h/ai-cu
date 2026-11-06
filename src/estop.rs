//! 緊急停止 (D-019、D-032)。停止要因を `StopSource` として束ね、各バッチの前と待ちの途中で確かめる。
//! 最大実行時間と実ユーザーのマウス移動は、時計とカーソルを持つ実行器 (`run`) が判定する。

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StopReason {
    /// 停止の目印のファイルが現れた
    Sentinel { path: String },
    /// 最大実行時間を超えた
    Deadline { limit_ms: u64 },
    /// 利用者がマウスを動かした
    UserMouse { distance_px: f64 },
    /// その他の停止要因 (テストや将来の拡張)
    External { message: String },
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::Sentinel { path } => write!(f, "停止の目印のファイルが現れた: {path}"),
            StopReason::Deadline { limit_ms } => write!(f, "最大実行時間 {limit_ms} ms を超えた"),
            StopReason::UserMouse { distance_px } => {
                write!(
                    f,
                    "利用者がマウスを動かした (予定の位置から {distance_px:.0} px)"
                )
            }
            StopReason::External { message } => write!(f, "停止: {message}"),
        }
    }
}

pub trait StopSource {
    fn poll(&mut self) -> Option<StopReason>;
}

/// ファイルが存在したら停止する。開始時に既に存在するなら、開始しない (最初の確認で止まる)。
pub struct SentinelStop {
    paths: Vec<PathBuf>,
}

impl SentinelStop {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        SentinelStop { paths }
    }
}

impl StopSource for SentinelStop {
    fn poll(&mut self) -> Option<StopReason> {
        self.paths
            .iter()
            .find(|p| p.exists())
            .map(|p| StopReason::Sentinel {
                path: p.display().to_string(),
            })
    }
}

/// 停止要因の束。どれか 1 つが止めれば止まる。
#[derive(Default)]
pub struct StopSet {
    sources: Vec<Box<dyn StopSource>>,
}

impl StopSet {
    pub fn new() -> Self {
        StopSet::default()
    }

    pub fn push(&mut self, s: Box<dyn StopSource>) {
        self.sources.push(s);
    }

    pub fn poll(&mut self) -> Option<StopReason> {
        self.sources.iter_mut().find_map(|s| s.poll())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct After(u32);

    impl StopSource for After {
        fn poll(&mut self) -> Option<StopReason> {
            if self.0 == 0 {
                return Some(StopReason::External {
                    message: "test".into(),
                });
            }
            self.0 -= 1;
            None
        }
    }

    #[test]
    fn estop_sentinel_file_triggers_when_present() {
        let f = crate::testing::unique_temp_dir("estop").join("STOP");
        let mut s = SentinelStop::new(vec![f.clone()]);
        assert_eq!(s.poll(), None);
        std::fs::write(&f, b"").unwrap();
        assert!(matches!(s.poll(), Some(StopReason::Sentinel { .. })));
    }

    #[test]
    fn estop_set_stops_when_any_source_stops() {
        let mut set = StopSet::new();
        set.push(Box::new(After(5)));
        set.push(Box::new(After(2)));
        assert_eq!(set.poll(), None);
        assert_eq!(set.poll(), None);
        assert!(set.poll().is_some());
        assert!(StopSet::new().poll().is_none());
    }
}
