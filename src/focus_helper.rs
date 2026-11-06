//! 前面化の補助プロセス (D-057)。
//!
//! 常駐の MCP サーバーは、起動の後に人が何か入力すると前面化の権利を失う (D-056 の実験の X2)。前面がホスト (AI の
//! 端末など) のときに新しく起動したプロセスは、親が権利を失っていても前面化できる (X4)。そこで、断られて前面が
//! ホストのときだけ、自分の実行ファイルを隠しのサブコマンドで起動し、その子に `SetForegroundWindow` を 1 回だけ
//! 呼ばせる。子は前面化だけを行い、設定、ガード、無操作の確認、理由の分類は親 (`run` の `focus`) が持つ。
//!
//! 隠しのサブコマンドは clap の外 (`main` の最初) で見分けるので、`--help` にも `schema` にも出ない。

use std::ffi::OsString;
use std::path::Path;

use crate::device::{BackendError, HelperOutcome};

/// 隠しのサブコマンドの名前。引数は対象の hwnd (10 進) 1 つだけ
pub const SUBCOMMAND: &str = "__focus-helper";
/// 子の終了を待つ上限 (ミリ秒)。超えたら kill して断られた扱い
pub const TIMEOUT_MS: u64 = 3000;
/// 子の終了を見る間隔 (ミリ秒)
const POLL_MS: u64 = 10;

/// 子の終了コード: `SetForegroundWindow` が 1 を返した
pub const EXIT_ACCEPTED: i32 = 0;
/// 子の終了コード: `SetForegroundWindow` が 0 を返した
pub const EXIT_REFUSED: i32 = 1;
/// 子の終了コード: 引数の誤り、または有効なトップレベルのウィンドウではない (呼んでいない)
pub const EXIT_USAGE: i32 = 2;
/// 子の終了コード: 最下層の拒否 (開発シェル。D-022) で呼んでいない
pub const EXIT_FORBIDDEN: i32 = 3;

/// 起動の引数 (プログラム名を含む) が補助プロセスの呼び出しか。1 番目の引数だけを見る。
pub fn is_helper_invocation(args: &[OsString]) -> bool {
    args.get(1).is_some_and(|a| a == SUBCOMMAND)
}

/// サブコマンドの後ろの引数から hwnd を読む。10 進の 0 でない数 1 つだけを受け付ける。
pub fn parse_hwnd(rest: &[OsString]) -> Option<usize> {
    let [one] = rest else {
        return None;
    };
    let s = one.to_str()?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<usize>().ok().filter(|&h| h != 0)
}

/// 子の本体。設定もガードも読まず、hwnd が有効なトップレベルのウィンドウであることだけを確かめて、
/// `set` (`SetForegroundWindow` を 1 回) を呼ぶ。戻り値は終了コード。
pub fn child_main(
    rest: &[OsString],
    is_top_level: &dyn Fn(usize) -> bool,
    set: &dyn Fn(usize) -> Result<bool, BackendError>,
) -> i32 {
    let Some(hwnd) = parse_hwnd(rest) else {
        return EXIT_USAGE;
    };
    if !is_top_level(hwnd) {
        return EXIT_USAGE;
    }
    match set(hwnd) {
        Ok(true) => EXIT_ACCEPTED,
        Ok(false) => EXIT_REFUSED,
        Err(BackendError::Forbidden(_)) | Err(BackendError::OutOfScope(_)) => EXIT_FORBIDDEN,
        // TimedOut (UIA の時間切れ。D-104) はここには来ないが、網羅のため Os と同じ扱い
        Err(BackendError::Os(_))
        | Err(BackendError::DesktopUnavailable(_))
        | Err(BackendError::TimedOut(_)) => EXIT_REFUSED,
    }
}

/// 子の終了コードを結果に直す。
pub fn outcome_of_exit(code: i32) -> HelperOutcome {
    match code {
        EXIT_ACCEPTED => HelperOutcome::Accepted,
        EXIT_REFUSED => HelperOutcome::Refused,
        EXIT_USAGE => HelperOutcome::Failed("対象が有効なトップレベルのウィンドウではない".into()),
        EXIT_FORBIDDEN => {
            HelperOutcome::Failed("補助プロセスが前面化を拒否した (開発シェル)".into())
        }
        other => HelperOutcome::Failed(format!(
            "補助プロセスが想定外の終了コード {other} で終わった"
        )),
    }
}

/// 待つ相手の子プロセス (テストでは差し替える)。
pub trait HelperChild {
    /// 終わっていれば終了コード、まだなら None
    fn try_wait(&mut self) -> std::io::Result<Option<i32>>;
    /// 強制終了して回収する (失敗は無視する)
    fn kill(&mut self);
}

impl HelperChild for std::process::Child {
    fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        // Windows では終了コードが必ずある。無ければ想定外の終わり方として扱う
        Ok(std::process::Child::try_wait(self)?.map(|s| s.code().unwrap_or(-1)))
    }

    fn kill(&mut self) {
        let _ = std::process::Child::kill(self);
        let _ = self.wait();
    }
}

/// 子の終了を `timeout_ms` まで待つ。超えたら kill して `TimedOut`。待ちは `sleep` で刻む (テストでは時計を進める)。
pub fn wait_bounded(
    child: &mut dyn HelperChild,
    timeout_ms: u64,
    sleep: &mut dyn FnMut(u64),
) -> HelperOutcome {
    let mut waited = 0;
    loop {
        match child.try_wait() {
            Ok(Some(code)) => return outcome_of_exit(code),
            Ok(None) => {}
            Err(e) => {
                child.kill();
                return HelperOutcome::Failed(format!("補助プロセスの終了を待てない: {e}"));
            }
        }
        if waited >= timeout_ms {
            child.kill();
            return HelperOutcome::TimedOut;
        }
        sleep(POLL_MS);
        waited += POLL_MS;
    }
}

/// `exe` を隠しのサブコマンドで、窓を作らずに (CREATE_NO_WINDOW) 起動し、終了を待つ。標準入出力は渡さない
/// (MCP の標準出力に子が書かないように)。
pub fn spawn_and_wait(exe: &Path, hwnd: usize, timeout_ms: u64) -> HelperOutcome {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let child = Command::new(exe)
        .arg(SUBCOMMAND)
        .arg(hwnd.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match child {
        Ok(mut c) => wait_bounded(&mut c, timeout_ms, &mut |ms| {
            std::thread::sleep(std::time::Duration::from_millis(ms))
        }),
        Err(e) => HelperOutcome::Failed(format!("補助プロセスを起動できない: {e}")),
    }
}

/// `main` の最初に呼ぶ。補助プロセスの呼び出しなら子の本体を実際の装置で走らせて終了コードを返す。
/// 前面化は既存の境界 (`Win32Desktop::set_foreground`。開発シェルの最下層の拒否を含む) を通す。
pub fn real_main_if_helper(args: &[OsString]) -> Option<i32> {
    if !is_helper_invocation(args) {
        return None;
    }
    let desktop = crate::backend::Win32Desktop::new();
    Some(child_main(
        &args[2..],
        &crate::win32::window::is_top_level,
        &|h| crate::device::Desktop::set_foreground(&desktop, h),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    /// 隠しのサブコマンドは 1 番目の引数で見分ける (前置のオプションの後ろや、別のサブコマンドの引数では見ない)
    #[test]
    fn helper_invocation_is_recognized_only_as_first_argument() {
        assert!(is_helper_invocation(&args(&["ai-cu", SUBCOMMAND, "1"])));
        assert!(!is_helper_invocation(&args(&["ai-cu"])));
        assert!(!is_helper_invocation(&args(&[
            "ai-cu", "focus", SUBCOMMAND
        ])));
        assert!(!is_helper_invocation(&args(&[
            "ai-cu",
            "--config=x",
            SUBCOMMAND,
            "1"
        ])));
    }

    /// hwnd は 10 進の 0 でない数 1 つだけ
    #[test]
    fn parse_hwnd_accepts_single_nonzero_decimal() {
        assert_eq!(parse_hwnd(&args(&["4660"])), Some(4660));
        for bad in [
            vec![],
            vec!["0"],
            vec!["0x1234"],
            vec!["-1"],
            vec!["abc"],
            vec!["1", "2"],
        ] {
            assert_eq!(parse_hwnd(&args(&bad)), None, "{bad:?}");
        }
    }

    /// 子は、有効なトップレベルのウィンドウにだけ 1 回呼ぶ。結果を終了コードで返す
    #[test]
    fn child_main_calls_set_foreground_once_for_valid_top_level_window() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let top = |h: usize| h == 0x20;
        let accept = |h: usize| {
            calls.borrow_mut().push(h);
            Ok(true)
        };
        assert_eq!(child_main(&args(&["32"]), &top, &accept), EXIT_ACCEPTED);
        assert_eq!(*calls.borrow(), vec![0x20]);
        let refuse = |h: usize| {
            calls.borrow_mut().push(h);
            Ok(false)
        };
        assert_eq!(child_main(&args(&["32"]), &top, &refuse), EXIT_REFUSED);
        let forbid = |_: usize| Err(BackendError::Forbidden("dev".into()));
        assert_eq!(child_main(&args(&["32"]), &top, &forbid), EXIT_FORBIDDEN);
        // 有効なトップレベルではない、引数の誤り: 呼ばない
        calls.borrow_mut().clear();
        assert_eq!(child_main(&args(&["33"]), &top, &accept), EXIT_USAGE);
        assert_eq!(child_main(&args(&["x"]), &top, &accept), EXIT_USAGE);
        assert_eq!(child_main(&args(&[]), &top, &accept), EXIT_USAGE);
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn exit_codes_map_to_outcomes() {
        assert_eq!(outcome_of_exit(EXIT_ACCEPTED), HelperOutcome::Accepted);
        assert_eq!(outcome_of_exit(EXIT_REFUSED), HelperOutcome::Refused);
        for code in [EXIT_USAGE, EXIT_FORBIDDEN, -1, 101] {
            assert!(
                matches!(outcome_of_exit(code), HelperOutcome::Failed(_)),
                "{code}"
            );
        }
    }

    /// 偽の子: `exit_after` 回目の try_wait で `code` を返す (None なら終わらない)
    struct FakeChild {
        polls: u32,
        exit_after: Option<u32>,
        code: i32,
        fail: bool,
        killed: bool,
    }

    impl FakeChild {
        fn new(exit_after: Option<u32>, code: i32) -> Self {
            FakeChild {
                polls: 0,
                exit_after,
                code,
                fail: false,
                killed: false,
            }
        }
    }

    impl HelperChild for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
            if self.fail {
                return Err(std::io::Error::other("wait"));
            }
            self.polls += 1;
            Ok(match self.exit_after {
                Some(n) if self.polls >= n => Some(self.code),
                _ => None,
            })
        }

        fn kill(&mut self) {
            self.killed = true;
        }
    }

    /// 子が時間内に終われば、その終了コードの結果。kill しない
    #[test]
    fn wait_bounded_returns_child_result_within_timeout() {
        for (code, want) in [
            (EXIT_ACCEPTED, HelperOutcome::Accepted),
            (EXIT_REFUSED, HelperOutcome::Refused),
        ] {
            let mut c = FakeChild::new(Some(3), code);
            let mut slept = 0;
            let r = wait_bounded(&mut c, 1000, &mut |ms| slept += ms);
            assert_eq!(r, want);
            assert!(!c.killed);
            assert!(slept < 1000, "{slept}");
        }
    }

    /// 時間切れなら kill して TimedOut。上限を超えて待たない
    #[test]
    fn wait_bounded_kills_child_on_timeout() {
        let mut c = FakeChild::new(None, 0);
        let mut slept = 0;
        let r = wait_bounded(&mut c, 100, &mut |ms| slept += ms);
        assert_eq!(r, HelperOutcome::TimedOut);
        assert!(c.killed);
        assert!((100..=100 + POLL_MS).contains(&slept), "{slept}");
    }

    /// 待ちの失敗は Failed (kill して回収する)
    #[test]
    fn wait_bounded_reports_wait_error_as_failed() {
        let mut c = FakeChild::new(None, 0);
        c.fail = true;
        let r = wait_bounded(&mut c, 100, &mut |_| {});
        assert!(matches!(r, HelperOutcome::Failed(_)), "{r:?}");
        assert!(c.killed);
    }

    /// 起動できない実行ファイルは Failed (子プロセスは作られない)
    #[test]
    fn spawn_failure_is_reported_as_failed() {
        let missing = crate::testing::unique_temp_dir("focus-helper-missing").join("none.exe");
        let r = spawn_and_wait(&missing, 0x20, 100);
        assert!(matches!(r, HelperOutcome::Failed(_)), "{r:?}");
    }

    /// 補助プロセスの呼び出しでなければ、何もしない (通常の CLI に進む)
    #[test]
    fn real_main_ignores_normal_invocations() {
        assert_eq!(real_main_if_helper(&args(&["ai-cu", "schema"])), None);
        assert_eq!(real_main_if_helper(&args(&["ai-cu"])), None);
    }

    /// 隠しのサブコマンドは clap に無い (help と schema に出ず、clap では誤りになる)
    #[test]
    fn hidden_subcommand_is_not_in_help_or_schema() {
        use clap::{CommandFactory, Parser};
        let help = crate::cli::Cli::command().render_long_help().to_string();
        assert!(!help.contains(SUBCOMMAND), "{help}");
        assert!(!help.contains("focus-helper"), "{help}");
        assert!(crate::cli::Cli::try_parse_from(["ai-cu", SUBCOMMAND, "1"]).is_err());
        let schema = crate::cli::schema().to_string();
        assert!(!schema.contains("focus-helper"));
    }
}
