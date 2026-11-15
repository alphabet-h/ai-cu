//! 前面化の補助プロセス (D-057)。
//!
//! 常駐の MCP サーバーは、起動の後に人が何か入力すると前面化の権利を失う (D-056 の実験の X2)。前面がホスト (AI の
//! 端末など) のときに新しく起動したプロセスは、親が権利を失っていても前面化できる (X4)。そこで、断られて前面が
//! ホストのときだけ、自分の実行ファイルを隠しのサブコマンドで起動し、その子に `SetForegroundWindow` を 1 回だけ
//! 呼ばせる。子も親と同じ判定 (設定の基本の層と `AI_CU_CONFIG`、停止の目印、入力デスクトップ、窓の判定、無操作の
//! 確認) を前面化の前に掛ける (D-138。直接起動されても `focus` で前面にできない窓は前面にしない)。理由の分類は親
//! (`run` の `focus`) が持つ。
//!
//! 隠しのサブコマンドは clap の外 (`main` の最初) で見分けるので、`--help` にも `schema` にも出ない。

use std::ffi::OsString;
use std::path::Path;

use crate::config::{ConfigError, Layer};
use crate::device::{BackendError, HelperOutcome, InputDesktop, TopWindow};
use crate::error::ErrorKind;
use crate::hint::{Hint, Surface};
use crate::policy::Policy;
use crate::run::{WindowGuard, focus_idle_check, input_desktop_error, judge_window};
use crate::script::ActionKind;
use crate::win32::process::HostProcess;

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
/// 子の終了コード: 設定か窓の判定で断った (呼んでいない。D-138)
pub const EXIT_DENIED: i32 = 4;
/// 子の終了コード: 人が操作中なので断った (呼んでいない。D-138)
pub const EXIT_USER_ACTIVE: i32 = 5;

/// 子が前面化の前に掛ける判定の結果 (D-138)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// 判定をすべて通った。前面化に進む
    Pass,
    /// 設定か窓の判定で断った。文はどの判定で断ったかの固定の文で、窓を特定する情報と設定のパスを載せない
    Denied(&'static str),
    /// 無操作の確認で、人が操作中と分かった
    UserActive,
    /// 開発シェルの最下層の拒否 (D-022)。窓の一覧を読む前に断る
    Forbidden,
}

// 断る文は固定で、窓のプロセス名、クラス、pid、題名と、設定ファイルのパスを載せない (D-138)。

/// 断る文: 設定の層を読めない (壊れている、指定のファイルが無い)
pub const DENY_CONFIG: &str = "設定を読めない";
/// 断る文: 停止の目印のファイルがある
pub const DENY_STOP: &str = "停止の目印がある";
/// 断る文: 総スイッチか actions が focus を許さない
pub const DENY_ACTION: &str = "総スイッチか actions が focus を許さない";
/// 断る文: 入力デスクトップが使えない (ロック中、スクリーンセーバー、別のデスクトップ、読めない)
pub const DENY_DESKTOP: &str = "入力デスクトップが使えない";
/// 断る文: 対象の候補の一覧に無い
pub const DENY_NOT_CANDIDATE: &str = "focus の対象になる窓ではない";
/// 断る文: 対象が最小化されている
pub const DENY_MINIMIZED: &str = "対象が最小化されている";
/// 断る文: 起動した側の連鎖が読めないか空
pub const DENY_HOST_CHAIN: &str = "起動した側を確かめられない";
/// 断る文: 窓の判定で断った
pub const DENY_WINDOW: &str = "窓の判定 (設定、起動した側、ゴースト、管理者権限) が許さない";
/// 断る文: 無操作の時間を読めない
pub const DENY_IDLE_UNREADABLE: &str = "無操作を確かめられない";
/// 人が操作中で断ったときの文
const USER_ACTIVE_TEXT: &str = "人が操作中";

/// 子の判定の材料 (D-138)。実際の装置から集めるのは `real_gate`、テストでは直に作る。
#[derive(Debug, Clone)]
pub struct GateInputs {
    /// 対象の候補 (`top_level_windows_detailed` の一覧) の中の、hwnd が一致する窓。無ければ None
    pub target: Option<TopWindow>,
    /// 起動した側の連鎖の pid (D-055)。読めなければ Err
    pub host_pids: Result<Vec<u32>, String>,
    /// 子自身が管理者権限で動いているか
    pub self_elevated: bool,
    /// 入力デスクトップの状態 (D-066)。読めなければ Err
    pub desktop: Result<InputDesktop, String>,
}

/// 子が前面化の前に掛ける判定 (D-138)。
/// 停止の目印 → 総スイッチと actions → 入力デスクトップ → 対象の候補 → 起動した側の連鎖 → 窓の判定
/// (親と共有の `judge_window`) → 最小化 → 無操作の確認 (親と共有の `focus_idle_check`)。
/// 最小化は窓の判定の後に見る (親の `focus` と同じ順)。先に見ると、deny の窓が最小化かどうかが断る文の違いで読める
/// 断る文は判定ごとの固定の文で、`AppError` の message (窓のプロセス名、クラス、pid、設定の場所を含む) は使わない
pub fn judge_target(
    policy: &Policy,
    inputs: &GateInputs,
    stop_present: bool,
    idle_ms: &dyn Fn() -> Result<u64, BackendError>,
) -> Gate {
    if stop_present {
        return Gate::Denied(DENY_STOP);
    }
    if policy.check_action_kind(ActionKind::Focus).is_err() {
        return Gate::Denied(DENY_ACTION);
    }
    match &inputs.desktop {
        Ok(state) if input_desktop_error(state).is_none() => {}
        _ => return Gate::Denied(DENY_DESKTOP),
    }
    let Some(target) = &inputs.target else {
        return Gate::Denied(DENY_NOT_CANDIDATE);
    };
    let host_pids = match &inputs.host_pids {
        Ok(p) if !p.is_empty() => p.as_slice(),
        _ => return Gate::Denied(DENY_HOST_CHAIN),
    };
    let guard = WindowGuard {
        self_hwnd: None,
        host_pids,
        self_elevated: inputs.self_elevated,
        surface: Surface::Cli,
    };
    if judge_window(
        policy,
        &target.info,
        &guard,
        "前面にする対象",
        Hint::FocusDenied,
    )
    .is_err()
    {
        return Gate::Denied(DENY_WINDOW);
    }
    if target.minimized {
        return Gate::Denied(DENY_MINIMIZED);
    }
    match focus_idle_check(policy, idle_ms, Surface::Cli) {
        Ok(()) => Gate::Pass,
        Err(e) if e.kind == ErrorKind::UserActive => Gate::UserActive,
        Err(_) => Gate::Denied(DENY_IDLE_UNREADABLE),
    }
}

/// 層を方針にして `judge_target` を掛ける。層が読めなければ断る (D-018 の fail-closed。D-138)。
/// `stop_present` は方針 (層の停止の目印) から目印の有無を答える
pub fn gate_from_parts(
    layers: Result<Vec<Layer>, ConfigError>,
    inputs: &GateInputs,
    stop_present: &dyn Fn(&Policy) -> bool,
    idle_ms: &dyn Fn() -> Result<u64, BackendError>,
) -> Gate {
    let Ok(layers) = layers else {
        return Gate::Denied(DENY_CONFIG);
    };
    let policy = Policy::new(layers);
    judge_target(&policy, inputs, stop_present(&policy), idle_ms)
}

/// 集めた材料を判定の形にそろえる。`windows` の中から hwnd が一致する窓を対象に選び、連鎖を pid の並びにする
pub fn gate_inputs(
    hwnd: usize,
    windows: Vec<TopWindow>,
    chain: Result<Vec<HostProcess>, String>,
    self_elevated: bool,
    desktop: Result<InputDesktop, String>,
) -> GateInputs {
    GateInputs {
        target: windows.into_iter().find(|w| w.info.hwnd == hwnd),
        host_pids: chain.map(|c| c.iter().map(|h| h.pid).collect()),
        self_elevated,
        desktop,
    }
}

/// 実際の装置で子の判定を掛ける (D-138)。開発シェルでは窓の一覧を読む前に断る (親の `find_window` の前の
/// `focus_allowed` と同じ門。D-050)。層は基本の層と `AI_CU_CONFIG` (親の `--config` は渡らない)。
/// 停止の目印は親と同じ `runner::stop_set` で見る
fn real_gate(hwnd: usize) -> Gate {
    use crate::config::{ProcessEnv, SystemLocator};
    use crate::device::Desktop;
    if crate::backend::focus_allowed(None, &ProcessEnv).is_err() {
        return Gate::Forbidden;
    }
    let paths = crate::config::resolve_paths(None, &ProcessEnv, &SystemLocator);
    let layers = crate::config::load_layers(&paths);
    let desktop = crate::backend::Win32Desktop::new();
    let inputs = gate_inputs(
        hwnd,
        crate::win32::window::top_level_windows_detailed(),
        crate::win32::process::host_chain(),
        desktop.self_elevated(),
        desktop.input_desktop().map_err(|e| e.to_string()),
    );
    gate_from_parts(
        layers,
        &inputs,
        &|policy| {
            crate::runner::stop_set(policy, &ProcessEnv)
                .poll()
                .is_some()
        },
        &|| desktop.idle_ms(),
    )
}

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

/// 子の本体。hwnd が有効なトップレベルのウィンドウであることを確かめ、`gate` (前面化の前の判定。D-138) を
/// 1 回だけ掛けて、通れば `set` (`SetForegroundWindow` を 1 回) を呼ぶ。戻り値は終了コード。
/// 設定か窓の判定で断ったとき (4 / 5) は標準エラーに固定の文を 1 行書く。開発シェルの拒否 (3) では何も書かない。
/// どの場合も `set` を呼ばない。
pub fn child_main(
    rest: &[OsString],
    is_top_level: &dyn Fn(usize) -> bool,
    gate: &dyn Fn(usize) -> Gate,
    set: &dyn Fn(usize) -> Result<bool, BackendError>,
) -> i32 {
    let Some(hwnd) = parse_hwnd(rest) else {
        return EXIT_USAGE;
    };
    if !is_top_level(hwnd) {
        return EXIT_USAGE;
    }
    match gate(hwnd) {
        Gate::Pass => {}
        Gate::Denied(why) => {
            eprintln!("ai-cu {SUBCOMMAND}: 前面にしない: {why}");
            return EXIT_DENIED;
        }
        Gate::UserActive => {
            eprintln!("ai-cu {SUBCOMMAND}: 前面にしない: {USER_ACTIVE_TEXT}");
            return EXIT_USER_ACTIVE;
        }
        Gate::Forbidden => return EXIT_FORBIDDEN,
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
        EXIT_DENIED => HelperOutcome::Failed("補助プロセスが設定か窓の判定で前面化を断った".into()),
        EXIT_USER_ACTIVE => {
            HelperOutcome::Failed("補助プロセスが人の操作中なので前面化を断った".into())
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
/// 前面化の前に、親と同じ判定 (`real_gate`) を掛ける (D-138)。前面化は既存の境界
/// (`Win32Desktop::set_foreground`。開発シェルの最下層の拒否を含む) を通す。
pub fn real_main_if_helper(args: &[OsString]) -> Option<i32> {
    if !is_helper_invocation(args) {
        return None;
    }
    let desktop = crate::backend::Win32Desktop::new();
    Some(child_main(
        &args[2..],
        &crate::win32::window::is_top_level,
        &real_gate,
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
        let pass = |_: usize| Gate::Pass;
        let accept = |h: usize| {
            calls.borrow_mut().push(h);
            Ok(true)
        };
        assert_eq!(
            child_main(&args(&["32"]), &top, &pass, &accept),
            EXIT_ACCEPTED
        );
        assert_eq!(*calls.borrow(), vec![0x20]);
        let refuse = |h: usize| {
            calls.borrow_mut().push(h);
            Ok(false)
        };
        assert_eq!(
            child_main(&args(&["32"]), &top, &pass, &refuse),
            EXIT_REFUSED
        );
        let forbid = |_: usize| Err(BackendError::Forbidden("dev".into()));
        assert_eq!(
            child_main(&args(&["32"]), &top, &pass, &forbid),
            EXIT_FORBIDDEN
        );
        // 有効なトップレベルではない、引数の誤り: 呼ばない
        calls.borrow_mut().clear();
        assert_eq!(child_main(&args(&["33"]), &top, &pass, &accept), EXIT_USAGE);
        assert_eq!(child_main(&args(&["x"]), &top, &pass, &accept), EXIT_USAGE);
        assert_eq!(child_main(&args(&[]), &top, &pass, &accept), EXIT_USAGE);
        assert!(calls.borrow().is_empty());
    }

    /// 判定が断れば前面化を呼ばず、断り方ごとの終了コードで終わる (D-138)
    #[test]
    fn child_main_does_not_call_set_foreground_when_gate_refuses() {
        use std::cell::Cell;
        let top = |h: usize| h == 0x20;
        let calls = Cell::new(0u32);
        let set = |_: usize| {
            calls.set(calls.get() + 1);
            Ok(true)
        };
        for (gate, want) in [
            (Gate::Denied(DENY_WINDOW), EXIT_DENIED),
            (Gate::UserActive, EXIT_USER_ACTIVE),
            (Gate::Forbidden, EXIT_FORBIDDEN),
        ] {
            let g = move |_: usize| gate;
            assert_eq!(child_main(&args(&["32"]), &top, &g, &set), want, "{gate:?}");
        }
        assert_eq!(calls.get(), 0);
    }

    /// 判定はトップレベルの確認の後に 1 回だけ掛ける。引数の誤りとトップレベルでない hwnd では掛けない
    #[test]
    fn child_main_checks_gate_after_top_level_only_once() {
        use std::cell::RefCell;
        let seen = RefCell::new(Vec::new());
        let top = |h: usize| h == 0x20;
        let gate = |h: usize| {
            seen.borrow_mut().push(h);
            Gate::Pass
        };
        let set = |_: usize| Ok(true);
        assert_eq!(child_main(&args(&["33"]), &top, &gate, &set), EXIT_USAGE);
        assert_eq!(child_main(&args(&["x"]), &top, &gate, &set), EXIT_USAGE);
        assert_eq!(child_main(&args(&[]), &top, &gate, &set), EXIT_USAGE);
        assert!(seen.borrow().is_empty());
        assert_eq!(child_main(&args(&["32"]), &top, &gate, &set), EXIT_ACCEPTED);
        assert_eq!(*seen.borrow(), vec![0x20]);
    }

    #[test]
    fn exit_codes_map_to_outcomes() {
        assert_eq!(outcome_of_exit(EXIT_ACCEPTED), HelperOutcome::Accepted);
        assert_eq!(outcome_of_exit(EXIT_REFUSED), HelperOutcome::Refused);
        for code in [
            EXIT_USAGE,
            EXIT_FORBIDDEN,
            EXIT_DENIED,
            EXIT_USER_ACTIVE,
            -1,
            101,
        ] {
            assert!(
                matches!(outcome_of_exit(code), HelperOutcome::Failed(_)),
                "{code}"
            );
        }
        // 4 と 5 は別の文で、想定外の終了コードの文でもない
        let text = |code| match outcome_of_exit(code) {
            HelperOutcome::Failed(s) => s,
            other => panic!("{other:?}"),
        };
        let (denied, active) = (text(EXIT_DENIED), text(EXIT_USER_ACTIVE));
        assert_ne!(denied, active);
        for s in [&denied, &active] {
            assert!(!s.contains("想定外"), "{s}");
            assert_ne!(s, &text(EXIT_FORBIDDEN));
        }
    }

    // ---- 子の判定 (D-138)。基準の入力は「全部が通れば Pass」。各テストは基準から 1 つだけ変える ----

    use crate::device::WindowInfo;
    use crate::geometry::Rect;

    const ENABLED: &str = "[general]\ninput_enabled = true\ncapture_enabled = true\n";
    const ALLOW_NOTEPAD: &str = "[allow]\nprocesses = [\"notepad.exe\"]\n";

    fn layers_of(toml: &str) -> Vec<Layer> {
        let cfg = crate::config::Config::parse(&format!("version = 1\n{toml}")).unwrap();
        vec![Layer::from_config(&cfg, "test").unwrap()]
    }

    fn policy_of(toml: &str) -> Policy {
        Policy::new(layers_of(toml))
    }

    /// 基準の層: 総スイッチが入り、notepad.exe を許す (min_idle_ms は既定の 3000)
    fn base_policy() -> Policy {
        policy_of(&format!("{ENABLED}{ALLOW_NOTEPAD}"))
    }

    fn base_window() -> TopWindow {
        TopWindow {
            info: WindowInfo {
                hwnd: 0x20,
                pid: 100,
                process: "notepad.exe".into(),
                class: "Notepad".into(),
                title: "t".into(),
                elevated: Some(false),
            },
            rect: Some(Rect::new(0, 0, 100, 100)),
            minimized: false,
        }
    }

    fn base_inputs() -> GateInputs {
        GateInputs {
            target: Some(base_window()),
            host_pids: Ok(vec![1]),
            self_elevated: false,
            desktop: Ok(InputDesktop::Available),
        }
    }

    fn idle_ok() -> Result<u64, BackendError> {
        Ok(60_000)
    }

    /// 窓だけを変えた入力
    fn with_window(f: impl FnOnce(&mut TopWindow)) -> GateInputs {
        let mut inputs = base_inputs();
        f(inputs.target.as_mut().unwrap());
        inputs
    }

    #[test]
    fn judge_target_passes_allowed_window() {
        assert_eq!(
            judge_target(&base_policy(), &base_inputs(), false, &idle_ok),
            Gate::Pass
        );
    }

    #[test]
    fn judge_target_denies_when_stop_file_present() {
        let g = judge_target(&base_policy(), &base_inputs(), true, &|| {
            panic!("停止の目印で断るときは無操作を読まない")
        });
        assert_eq!(g, Gate::Denied(DENY_STOP));
    }

    #[test]
    fn judge_target_denies_when_input_disabled() {
        let p = policy_of(&format!(
            "[general]\ninput_enabled = false\ncapture_enabled = true\n{ALLOW_NOTEPAD}"
        ));
        assert_eq!(
            judge_target(&p, &base_inputs(), false, &idle_ok),
            Gate::Denied(DENY_ACTION)
        );
    }

    #[test]
    fn judge_target_denies_focus_in_deny_actions_or_missing_from_allow_actions() {
        let deny_focus = policy_of(&format!(
            "{ENABLED}{ALLOW_NOTEPAD}[deny]\nactions = [\"focus\"]\n"
        ));
        assert_eq!(
            judge_target(&deny_focus, &base_inputs(), false, &idle_ok),
            Gate::Denied(DENY_ACTION)
        );
        let only_click = policy_of(&format!(
            "{ENABLED}[allow]\nprocesses = [\"notepad.exe\"]\nactions = [\"click\"]\n"
        ));
        assert_eq!(
            judge_target(&only_click, &base_inputs(), false, &idle_ok),
            Gate::Denied(DENY_ACTION)
        );
    }

    #[test]
    fn judge_target_denies_unavailable_or_unreadable_input_desktop() {
        for desktop in [
            Ok(InputDesktop::Locked { name: None }),
            Ok(InputDesktop::ScreenSaver { name: None }),
            Ok(InputDesktop::Other {
                name: Some("Winlogon".into()),
            }),
            Err("読めない".to_string()),
        ] {
            let mut inputs = base_inputs();
            inputs.desktop = desktop.clone();
            assert_eq!(
                judge_target(&base_policy(), &inputs, false, &idle_ok),
                Gate::Denied(DENY_DESKTOP),
                "{desktop:?}"
            );
        }
    }

    #[test]
    fn judge_target_denies_minimized_target_and_empty_host_chain() {
        let minimized = with_window(|w| w.minimized = true);
        assert_eq!(
            judge_target(&base_policy(), &minimized, false, &idle_ok),
            Gate::Denied(DENY_MINIMIZED)
        );
        let mut empty = base_inputs();
        empty.host_pids = Ok(vec![]);
        assert_eq!(
            judge_target(&base_policy(), &empty, false, &idle_ok),
            Gate::Denied(DENY_HOST_CHAIN)
        );
    }

    #[test]
    fn judge_target_denies_denied_or_unlisted_process() {
        let denied = policy_of(&format!(
            "{ENABLED}{ALLOW_NOTEPAD}[deny]\nprocesses = [\"notepad.exe\"]\n"
        ));
        assert_eq!(
            judge_target(&denied, &base_inputs(), false, &idle_ok),
            Gate::Denied(DENY_WINDOW)
        );
        let unlisted = with_window(|w| w.info.process = "calc.exe".into());
        assert_eq!(
            judge_target(&base_policy(), &unlisted, false, &idle_ok),
            Gate::Denied(DENY_WINDOW)
        );
    }

    #[test]
    fn judge_target_denies_host_ghost_and_elevated_windows() {
        let mut host = base_inputs();
        host.host_pids = Ok(vec![1, 100]);
        let ghost = with_window(|w| w.info.class = "Ghost".into());
        let elevated = with_window(|w| w.info.elevated = Some(true));
        let unknown = with_window(|w| w.info.elevated = None);
        for (name, inputs) in [
            ("host", host),
            ("ghost", ghost),
            ("elevated", elevated),
            ("unknown", unknown),
        ] {
            assert_eq!(
                judge_target(&base_policy(), &inputs, false, &idle_ok),
                Gate::Denied(DENY_WINDOW),
                "{name}"
            );
        }
    }

    #[test]
    fn judge_target_maps_idle_to_user_active_or_denied() {
        let p = policy_of(&format!(
            "{ENABLED}{ALLOW_NOTEPAD}[capture]\nmin_idle_ms = 1000\n"
        ));
        let inputs = base_inputs();
        assert_eq!(
            judge_target(&p, &inputs, false, &|| Ok(10)),
            Gate::UserActive
        );
        assert_eq!(judge_target(&p, &inputs, false, &|| Ok(5000)), Gate::Pass);
        assert_eq!(
            judge_target(&p, &inputs, false, &|| Err(BackendError::Os("x".into()))),
            Gate::Denied(DENY_IDLE_UNREADABLE)
        );
    }

    #[test]
    fn judge_target_denies_missing_target_and_unreadable_host_chain() {
        let mut missing = base_inputs();
        missing.target = None;
        assert_eq!(
            judge_target(&base_policy(), &missing, false, &idle_ok),
            Gate::Denied(DENY_NOT_CANDIDATE)
        );
        let mut unreadable = base_inputs();
        unreadable.host_pids = Err("読めない".into());
        assert_eq!(
            judge_target(&base_policy(), &unreadable, false, &idle_ok),
            Gate::Denied(DENY_HOST_CHAIN)
        );
    }

    /// 順を押さえる: 停止の目印は対象の有無より先
    #[test]
    fn judge_target_checks_stop_before_missing_target() {
        let mut missing = base_inputs();
        missing.target = None;
        assert_eq!(
            judge_target(&base_policy(), &missing, true, &idle_ok),
            Gate::Denied(DENY_STOP)
        );
    }

    /// 断る文に窓を特定する情報を載せない (`windows` が伏せる deny の窓を読む経路を開かない)
    #[test]
    fn judge_target_denial_text_does_not_name_the_window() {
        let p = policy_of(&format!(
            "{ENABLED}[allow]\nprocesses = [\"secret.exe\"]\n[deny]\nprocesses = [\"secret.exe\"]\n"
        ));
        let inputs = with_window(|w| {
            w.info.process = "secret.exe".into();
            w.info.class = "SecretClass".into();
            w.info.pid = 4242;
        });
        let Gate::Denied(why) = judge_target(&p, &inputs, false, &idle_ok) else {
            panic!("断るはず");
        };
        assert_eq!(why, DENY_WINDOW);
        for word in ["secret", "SecretClass", "4242"] {
            assert!(!why.contains(word), "{why}");
        }
    }

    /// deny の窓が最小化でも、窓の判定の文で断る (最小化は窓の判定の後。先に見ると、最小化かどうかが文の違いで読める)
    #[test]
    fn judge_target_checks_window_before_minimized() {
        let denied = policy_of(&format!(
            "{ENABLED}{ALLOW_NOTEPAD}[deny]\nprocesses = [\"notepad.exe\"]\n"
        ));
        for minimized in [false, true] {
            let inputs = with_window(|w| w.minimized = minimized);
            assert_eq!(
                judge_target(&denied, &inputs, false, &idle_ok),
                Gate::Denied(DENY_WINDOW),
                "minimized={minimized}"
            );
        }
    }

    #[test]
    fn gate_from_parts_denies_when_layers_unreadable() {
        let g = gate_from_parts(
            Err(ConfigError::new(None, "壊れている")),
            &base_inputs(),
            &|_| panic!("層が読めなければ目印を見ない"),
            &idle_ok,
        );
        assert_eq!(g, Gate::Denied(DENY_CONFIG));
    }

    #[test]
    fn gate_inputs_picks_window_by_hwnd_and_keeps_chain_error() {
        let a = base_window();
        let mut b = base_window();
        b.info.hwnd = 0x30;
        b.info.process = "calc.exe".into();
        let chain = || {
            Ok(vec![
                HostProcess {
                    pid: 7,
                    exe: "a.exe".into(),
                },
                HostProcess {
                    pid: 8,
                    exe: "b.exe".into(),
                },
            ])
        };
        let desk = || Ok(InputDesktop::Available);
        let picked = gate_inputs(0x30, vec![a.clone(), b.clone()], chain(), true, desk());
        assert_eq!(picked.target, Some(b.clone()));
        assert_eq!(picked.host_pids, Ok(vec![7, 8]));
        assert!(picked.self_elevated);
        let none = gate_inputs(0x40, vec![a.clone(), b.clone()], chain(), false, desk());
        assert_eq!(none.target, None);
        let err = gate_inputs(0x20, vec![a, b], Err("読めない".into()), false, desk());
        assert_eq!(err.host_pids, Err("読めない".to_string()));
    }

    #[test]
    fn gate_from_parts_passes_when_all_parts_ok_and_passes_stop_closure() {
        use std::cell::Cell;
        let toml = format!("{ENABLED}{ALLOW_NOTEPAD}");
        for (stop, want) in [(false, Gate::Pass), (true, Gate::Denied(DENY_STOP))] {
            let calls = Cell::new(0u32);
            let g = gate_from_parts(
                Ok(layers_of(&toml)),
                &base_inputs(),
                &|_| {
                    calls.set(calls.get() + 1);
                    stop
                },
                &idle_ok,
            );
            assert_eq!(g, want, "{stop}");
            assert_eq!(calls.get(), 1);
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
