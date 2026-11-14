//! 実行前の検証 (`validate` サブコマンド、`--dry-run`、実行の冒頭で共通)。装置には触れない。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::ErrorKind;
use crate::geometry::{Point, Rect};
use crate::image;
use crate::keys::KeyCombo;
use crate::plan::{self, Step};
use crate::policy::{Policy, RULE_SAVE_PATH};
use crate::script::{Action, INLINE_PATH, Region, Script, WindowTarget, shape_issue};

/// 待ちの合計に足す、実行の余裕 (送信、ガードの確認、撮影の時間)
pub const DEADLINE_MARGIN_MS: u64 = 1000;

/// 設定に関係なく効く保存先の規則 (形と既存ファイル) の断りに付ける次の手 (D-075)。
/// `BUILTIN_HINT` と同じく、呼ばれ方 (CLI / MCP) に依らない文にする
pub const SAVE_PATH_HINT: &str = "保存先の規則 (.png のファイル、PNG でない既存ファイルを上書きしない) は設定では緩められない。人間に頼まず path を直す";

/// 撮影の保存先を断った理由 (D-075)。`rule` は設定では緩められない規則 (形と既存ファイル) なら
/// `RULE_SAVE_PATH`、`capture.save_dirs` の外れならそのキー。`fix` は人間向けの直し方 (D-079)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRefusal {
    pub message: String,
    pub rule: &'static str,
    pub fix: Option<String>,
}

impl SaveRefusal {
    /// 設定で緩められない規則なら `SAVE_PATH_HINT`。save_dirs の外れは None (終了コード 4 の既定の次の手)
    pub fn hint(&self) -> Option<&'static str> {
        (self.rule == RULE_SAVE_PATH).then_some(SAVE_PATH_HINT)
    }
}

/// 撮影の保存先の判定 (D-075)。操作列の検証と `shot` (CLI と MCP) が共有する。`INLINE_PATH` は判定しない。
/// 順に、パスの形 → `cwd` に足して字面で畳む → `capture.save_dirs` → 既にあるものが PNG か
pub fn check_save_target(policy: &Policy, path: &str, cwd: &Path) -> Result<(), SaveRefusal> {
    if path == INLINE_PATH {
        return Ok(());
    }
    let fixed = |message: String| SaveRefusal {
        message,
        rule: RULE_SAVE_PATH,
        fix: None,
    };
    image::check_save_path_shape(path).map_err(fixed)?;
    let abs = image::normalize_save_path(path, cwd);
    policy.check_save_dir(&abs).map_err(|d| SaveRefusal {
        message: d.message,
        // 設定のキーは policy.rs の check_save_dir だけが書く (Policy::denied は rule を必ず持つ)
        rule: d.rule.expect("Policy::denied は rule を持つ"),
        fix: d.fix,
    })?;
    image::check_overwritable(&abs).map_err(fixed)
}

/// 相対の保存先を足す作業ディレクトリ。読めなければ空 (相対のまま。絶対の save_dirs からは外れて断られる = 安全側)
pub(crate) fn current_dir_or_empty() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// 操作列の何番目か。操作列全体の問題なら None
    pub action_index: Option<usize>,
    /// `invalid_script` (形の誤り) か `policy_denied` (設定による拒否)
    pub code: &'static str,
    pub message: String,
    /// 状況に合わせた次の手 (組み込みの拒否など)。無ければ終了コードの既定
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// 設定による拒否の、当たった設定のキー (`policy::CONFIG_RULES` のどれか、`builtin`、`save_path`。D-079)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<&'static str>,
    /// 人間向けの直し方の 1 文 (D-079)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Issue {
    fn shape(i: Option<usize>, m: impl Into<String>) -> Self {
        Issue {
            action_index: i,
            code: ErrorKind::InvalidScript.code(),
            message: m.into(),
            hint: None,
            rule: None,
            fix: None,
        }
    }

    fn policy(i: Option<usize>, m: impl Into<String>) -> Self {
        Issue {
            action_index: i,
            code: ErrorKind::PolicyDenied.code(),
            message: m.into(),
            hint: None,
            rule: None,
            fix: None,
        }
    }

    /// 拒否の理由から作り、rule と fix を写す (D-079)。組み込みの拒否なら、その旨の次の手を付ける
    fn denied(i: Option<usize>, d: crate::policy::Denied) -> Self {
        let builtin = d.is_builtin();
        let mut issue = Issue::policy(i, d.message);
        issue.rule = d.rule;
        issue.fix = d.fix;
        if builtin {
            issue.hint = Some(crate::policy::BUILTIN_HINT.to_string());
        }
        issue
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Validation {
    pub issues: Vec<Issue>,
    /// 各操作の手順 (ドライランの出力に使う)。形の誤りがある操作は空
    #[serde(skip)]
    pub plans: Vec<Vec<Step>>,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.issues.is_empty()
    }

    /// 問題の種類から、終了コードの種類を決める。形の誤りを優先する。
    pub fn error_kind(&self) -> Option<ErrorKind> {
        if self
            .issues
            .iter()
            .any(|i| i.code == ErrorKind::InvalidScript.code())
        {
            Some(ErrorKind::InvalidScript)
        } else if self.issues.is_empty() {
            None
        } else {
            Some(ErrorKind::PolicyDenied)
        }
    }

    /// 終了コードの種類 (`error_kind`) を決めた最初の問題 (問題が無ければ None)。添字、次の手、
    /// rule と fix を写す元で、`run::first_issue_error` と TUI の事前検査が共有する (D-079)
    pub fn first_issue(&self) -> Option<&Issue> {
        let kind = self.error_kind()?;
        self.issues.iter().find(|i| i.code == kind.code())
    }
}

/// 操作列を検証する。`screen` を渡すと、座標が仮想デスクトップの中にあるかも確かめる。
pub fn validate(script: &Script, policy: &Policy, screen: Option<Rect>) -> Validation {
    let mut issues = Vec::new();
    let mut plans = Vec::new();
    if let Err(d) = policy.check_action_count(script.actions.len()) {
        issues.push(Issue::denied(None, d));
    }
    let mut total_sleep: u64 = 0;
    let cwd = current_dir_or_empty();
    for (i, action) in script.actions.iter().enumerate() {
        if let Some(m) = shape_issue(action) {
            issues.push(Issue::shape(Some(i), m));
            plans.push(Vec::new());
            continue;
        }
        let mut steps = match plan::plan(action) {
            Ok(s) => s,
            Err(m) => {
                issues.push(Issue::shape(Some(i), m));
                plans.push(Vec::new());
                continue;
            }
        };
        match policy.check_action_kind(action.kind()) {
            Err(d) => issues.push(Issue::denied(Some(i), d)),
            // 修飾キーつきのクリック (D-137): 操作の種類が通ったときだけ見る (input_enabled が偽の設定で、
            // 同じ理由の issue を 2 つ出さない)
            Ok(()) => {
                if let Action::Click { modifiers, .. } | Action::DoubleClick { modifiers, .. } =
                    action
                    && let Err(d) = policy.check_click_modifiers(modifiers)
                {
                    issues.push(Issue::denied(Some(i), d));
                }
            }
        }
        check_action_policy(i, action, &steps, policy, screen, &mut issues);
        // 撮影の保存先 (D-075): region / window / 全画面のどれでも、撮る前に判定する
        if let Action::Screenshot { path, .. } = action
            && let Err(r) = check_save_target(policy, path, &cwd)
        {
            let mut issue = Issue::policy(Some(i), r.message.clone());
            issue.hint = r.hint().map(str::to_string);
            issue.rule = Some(r.rule);
            issue.fix = r.fix;
            issues.push(issue);
        }
        steps.extend(plan::settle_before_next(action, script.actions.get(i + 1)));
        total_sleep = total_sleep.saturating_add(plan::time_budget_ms(&steps));
        plans.push(steps);
    }
    // 実行には待ち以外の時間もかかる。検証を通った操作列が実行時の期限で必ず止まることのないよう、余裕を見る
    // (focus が前面になるのを待つ上限、close で消えるのを待つ上限、wait_for、wait_stable、wait_change、
    // screenshot の settle の timeout_ms も数える)
    if let Err(d) = policy.check_duration(total_sleep, DEADLINE_MARGIN_MS) {
        issues.push(Issue::denied(None, d));
    }
    Validation { issues, plans }
}

fn check_action_policy(
    i: usize,
    action: &Action,
    steps: &[Step],
    policy: &Policy,
    screen: Option<Rect>,
    issues: &mut Vec<Issue>,
) {
    let mut points: Vec<Point> = plan::mouse_points(steps);
    points.dedup();
    for p in points {
        if let Some(s) = screen
            && !s.contains(p)
        {
            issues.push(Issue::shape(
                Some(i),
                format!("座標 ({}, {}) が画面 {s} の外", p.x, p.y),
            ));
            continue;
        }
        if let Err(d) = policy.check_point(p) {
            issues.push(Issue::denied(Some(i), d));
        }
    }
    match action {
        Action::Key { keys } => {
            if let Ok(combo) = keys.parse::<KeyCombo>()
                && let Err(d) = policy.check_key(&combo)
            {
                issues.push(Issue::denied(Some(i), d));
            }
        }
        Action::Type { text, .. } => {
            if let Err(d) = policy.check_text_len(text) {
                issues.push(Issue::denied(Some(i), d));
            }
            // 改行とタブは Enter と Tab のキーとして送るので、キーの規則にも通す
            for (c, name) in [('\n', "enter"), ('\r', "enter"), ('\t', "tab")] {
                if text.contains(c) {
                    let combo: KeyCombo = name.parse().expect("組み込みの名前");
                    if let Err(d) = policy.check_key(&combo) {
                        issues.push(Issue::denied(Some(i), d));
                    }
                }
            }
        }
        // 撮る対象 (screenshot と画面の待ち。D-081): 判定は 1 つの関数で通す
        Action::Screenshot { region, window, .. } => check_capture_target(
            i,
            "screenshot",
            region.as_ref(),
            window.as_ref(),
            policy,
            screen,
            issues,
        ),
        Action::WaitStable { region, window, .. } => check_capture_target(
            i,
            "wait_stable",
            region.as_ref(),
            window.as_ref(),
            policy,
            screen,
            issues,
        ),
        Action::WaitChange { region, window, .. } => check_capture_target(
            i,
            "wait_change",
            region.as_ref(),
            window.as_ref(),
            policy,
            screen,
            issues,
        ),
        // 位置なしの scroll は、実行時のカーソルの位置に届く。領域の規則があれば、事前に判定できるよう位置を求める
        Action::Scroll { x: None, .. } if policy.has_region_rules() => issues.push(Issue::policy(
            Some(i),
            "領域の規則があるので、scroll には x と y を指定する",
        )),
        // 前面にする対象のプロセス (D-050)、閉じる対象のプロセス (D-062)、待つ対象のプロセス (D-068)。
        // クラスと起動した側は実行時に、探す候補を絞る述語で判定する (通らない窓は無いと数える。D-139)。権限は見つけた窓で判定する
        Action::Focus { process, .. }
        | Action::Close { process, .. }
        | Action::WaitFor { process, .. } => {
            if let Err(d) = policy.check_process(process) {
                issues.push(Issue::denied(Some(i), d));
            }
        }
        _ => {}
    }
}

/// 撮る対象 (`region` / `window` / 全画面) の判定 (D-049、D-053)。`screenshot` と画面の待ち (`wait_stable` /
/// `wait_change`、D-081) が共有する。`name` は文に出す操作の名前 (screenshot の文は D-081 の前と同じ)
fn check_capture_target(
    i: usize,
    name: &str,
    region: Option<&Region>,
    window: Option<&WindowTarget>,
    policy: &Policy,
    screen: Option<Rect>,
    issues: &mut Vec<Issue>,
) {
    match (region, window) {
        (Some(r), _) => {
            let rect = Rect::from(*r);
            if let Some(s) = screen
                && !s.contains_rect(&rect)
            {
                issues.push(Issue::shape(
                    Some(i),
                    format!("撮影の矩形 {rect} が画面 {s} からはみ出す"),
                ));
            }
            if let Err(d) = policy.check_capture_rect(&rect) {
                issues.push(Issue::denied(Some(i), d));
            }
            // 画面全体を覆う矩形は全画面 (D-049)。画面を知らなければ判定しない
            if let Err(d) = policy.check_full_screen(Some(&rect), screen.as_ref()) {
                issues.push(Issue::denied(Some(i), d));
            }
        }
        // 対象をウィンドウで指定 (D-053): 矩形は実行時に解いて判定する。ここでは対象のプロセスを focus と同じく通す
        (None, Some(w)) => {
            if let Err(d) = policy.check_process(&w.process) {
                issues.push(Issue::denied(Some(i), d));
            }
        }
        // region の省略は全画面 (D-049)。画面の大きさを知らなくても判定でき、拒否ならそれで終わり
        (None, None) => match policy.check_full_screen(None, screen.as_ref()) {
            Err(d) => issues.push(Issue::denied(Some(i), d)),
            Ok(()) => match screen {
                Some(s) => {
                    if let Err(d) = policy.check_capture_rect(&s) {
                        issues.push(Issue::denied(Some(i), d));
                    }
                }
                // 画面の大きさを読めなかった検証 (DPI 認識の宣言の失敗など。D-058) では判定できない。領域の規則があれば通さない
                None if policy.has_region_rules() => issues.push(Issue::policy(
                    Some(i),
                    format!(
                        "領域の規則があるので、全画面の {name} は実行時まで判定できない。region を指定する"
                    ),
                )),
                None => {}
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Layer};

    fn policy(toml: &str) -> Policy {
        let cfg = Config::parse(&format!("version = 1\n{toml}")).unwrap();
        Policy::new(vec![Layer::from_config(&cfg, "t").unwrap()])
    }

    const ENABLED: &str = "[general]\ninput_enabled = true\ncapture_enabled = true\n";

    fn script(json_actions: &str) -> Script {
        Script::from_json(&format!("{{\"version\":1,\"actions\":[{json_actions}]}}")).unwrap()
    }

    #[test]
    fn validate_ok_when_allowed() {
        let v = validate(
            &script(r#"{"type":"click","x":1,"y":1},{"type":"key","keys":"ctrl+s"}"#),
            &policy(ENABLED),
            None,
        );
        assert!(v.ok(), "{:?}", v.issues);
        assert_eq!(v.error_kind(), None);
        assert_eq!(v.plans.len(), 2);
    }

    /// クリックの直後のキーと文字の前に待ちが入り、計画と時間の予算に出る。間に wait があれば足さない (D-059)
    #[test]
    fn validate_inserts_click_settle_before_keys_and_text() {
        let settle = Step::Sleep {
            ms: plan::CLICK_SETTLE_MS,
        };
        let v = validate(
            &script(
                r#"{"type":"click","x":1,"y":1},{"type":"type","text":"ab"},{"type":"double_click","x":1,"y":1},{"type":"key","keys":"ctrl+a"},{"type":"click","x":1,"y":1},{"type":"wait","ms":10},{"type":"type","text":"c"}"#,
            ),
            &policy(ENABLED),
            None,
        );
        assert!(v.ok(), "{:?}", v.issues);
        assert_eq!(v.plans[0].last(), Some(&settle));
        assert_eq!(v.plans[2].last(), Some(&settle));
        assert_ne!(v.plans[4].last(), Some(&settle));
        assert!(!v.plans[1].contains(&settle) && !v.plans[3].contains(&settle));
        // 足した待ちも時間の予算に数える: click → key は 200 + 50 と余裕 1000 で 1250 ミリ秒
        let s = script(r#"{"type":"click","x":1,"y":1},{"type":"key","keys":"enter"}"#);
        let limit = |ms: u64| policy(&format!("{ENABLED}[limits]\nmax_duration_ms = {ms}\n"));
        assert!(validate(&s, &limit(1250), None).ok());
        assert!(!validate(&s, &limit(1249), None).ok());
    }

    /// 修飾キーつきのクリック (D-137) は、操作の種類 key の許可にも通し、allow.keys を使う設定では断り、
    /// ctrl と alt の組は組み込みの拒否で断る。deny.keys は見ない
    #[test]
    fn validate_click_modifiers_checks_key_kind_allow_keys_and_builtin() {
        let ctrl = script(r#"{"type":"click","x":1,"y":1,"modifiers":["ctrl"]}"#);
        let ctrl_dbl = script(r#"{"type":"double_click","x":1,"y":1,"modifiers":["ctrl"]}"#);
        let plain = script(r#"{"type":"click","x":1,"y":1}"#);
        let ctrl_alt = script(r#"{"type":"click","x":1,"y":1,"modifiers":["ctrl","alt"]}"#);
        let ctrl_shift = script(r#"{"type":"click","x":1,"y":1,"modifiers":["ctrl","shift"]}"#);
        let only = |v: &Validation| -> Issue {
            assert_eq!(v.issues.len(), 1, "{:?}", v.issues);
            assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
            v.issues[0].clone()
        };

        // 操作の種類 key を断る設定: 修飾つきは 4、修飾なしは通る
        let deny_key = policy(&format!("{ENABLED}[deny]\nactions = [\"key\"]\n"));
        for s in [&ctrl, &ctrl_dbl] {
            let issue = only(&validate(s, &deny_key, None));
            assert_eq!(issue.rule, Some("deny.actions"));
            assert!(issue.message.contains("修飾キー"), "{}", issue.message);
        }
        assert!(validate(&plain, &deny_key, None).ok());

        // allow.actions に key が無い設定: 修飾つきは 4、修飾なしは通る
        let click_only = policy(&format!("{ENABLED}[allow]\nactions = [\"click\"]\n"));
        assert_eq!(
            only(&validate(&ctrl, &click_only, None)).rule,
            Some("allow.actions")
        );
        assert!(validate(&plain, &click_only, None).ok());

        // allow.keys を使う設定: 修飾つきは 4 (rule は allow.keys、fix は「を空にする」)
        let allow_keys = policy(&format!("{ENABLED}[allow]\nkeys = [\"ctrl+s\"]\n"));
        let issue = only(&validate(&ctrl, &allow_keys, None));
        assert_eq!(issue.rule, Some("allow.keys"));
        assert!(
            issue
                .fix
                .as_deref()
                .is_some_and(|f| f.contains("allow.keys を空にする")),
            "{:?}",
            issue.fix
        );
        assert!(
            issue.message.contains("modifiers なし"),
            "{}",
            issue.message
        );
        assert!(validate(&plain, &allow_keys, None).ok());

        // input_enabled が偽: 同じ理由の issue は 1 つだけ
        let disabled = policy("[general]\ninput_enabled = false\n");
        assert_eq!(
            only(&validate(&ctrl, &disabled, None)).rule,
            Some("general.input_enabled")
        );

        // deny.keys だけなら見ない
        let deny_keys = policy(&format!("{ENABLED}[deny]\nkeys = [\"ctrl+s\"]\n"));
        assert!(validate(&ctrl, &deny_keys, None).ok());

        // ctrl と alt の組は、設定によらず組み込みの拒否 (rule は builtin、fix なし、組み込みの次の手)
        let allow_click_key = policy(&format!(
            "{ENABLED}[allow]\nactions = [\"click\", \"key\"]\n"
        ));
        for p in [&policy(ENABLED), &allow_click_key, &deny_key, &allow_keys] {
            let issue = only(&validate(&ctrl_alt, p, None));
            assert_eq!(issue.rule, Some(crate::policy::RULE_BUILTIN), "{issue:?}");
            assert_eq!(issue.fix, None);
            assert_eq!(issue.hint.as_deref(), Some(crate::policy::BUILTIN_HINT));
        }

        // click と key を許す設定なら ctrl+shift は通る
        assert!(validate(&ctrl_shift, &allow_click_key, None).ok());
        assert!(validate(&ctrl_shift, &policy(ENABLED), None).ok());
    }

    #[test]
    fn validate_reports_policy_denial_without_config() {
        let v = validate(
            &script(r#"{"type":"click","x":1,"y":1}"#),
            &Policy::closed(),
            None,
        );
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert_eq!(v.issues[0].action_index, Some(0));
    }

    #[test]
    fn validate_shape_errors_take_priority() {
        let v = validate(
            &script(r#"{"type":"click","x":1,"y":1},{"type":"key","keys":"ctrl+bogus"}"#),
            &Policy::closed(),
            None,
        );
        assert_eq!(v.error_kind(), Some(ErrorKind::InvalidScript));
    }

    #[test]
    fn validate_checks_interpolated_drag_points_against_regions() {
        // 始点と終点は許可領域の中だが、途中が deny の領域を横切る
        let p = policy(&format!("{ENABLED}[deny]\nregions=[\"40,0,20,100\"]\n"));
        let v = validate(
            &script(r#"{"type":"drag","from":{"x":0,"y":50},"to":{"x":100,"y":50},"steps":10}"#),
            &p,
            None,
        );
        assert_eq!(
            v.error_kind(),
            Some(ErrorKind::PolicyDenied),
            "{:?}",
            v.issues
        );
    }

    #[test]
    fn validate_builtin_keys_and_text_newlines() {
        let p = policy(&format!("{ENABLED}[deny]\nkeys=[\"enter\"]\n"));
        let v = validate(
            &script(r#"{"type":"key","keys":"alt+f4"},{"type":"type","text":"a\nb"}"#),
            &p,
            None,
        );
        let idx: Vec<_> = v.issues.iter().map(|i| i.action_index).collect();
        assert_eq!(idx, vec![Some(0), Some(1)]);
    }

    /// 全画面の screenshot (D-049): region の省略は画面の大きさを知らなくても拒否、画面全体を覆う矩形は
    /// 画面を知るときに拒否、一部なら通る。capture.full_screen = true ならどれも通る
    #[test]
    fn validate_rejects_full_screen_screenshot_unless_allowed() {
        let screen = Rect::new(0, 0, 3840, 2160);
        let p = policy(ENABLED);
        let omitted = script(r#"{"type":"screenshot","path":"a.png"}"#);
        for s in [None, Some(screen)] {
            let v = validate(&omitted, &p, s);
            assert!(!v.ok(), "{s:?}");
            assert!(
                v.issues[0].message.contains("full_screen"),
                "{}",
                v.issues[0].message
            );
        }
        let whole = script(
            r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":3840,"h":2160}}"#,
        );
        assert!(!validate(&whole, &p, Some(screen)).ok());
        assert!(
            validate(&whole, &p, None).ok(),
            "画面を知らなければ矩形は判定しない"
        );
        let part = script(
            r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":800,"h":600}}"#,
        );
        assert!(validate(&part, &p, Some(screen)).ok());
        let open = policy(&format!("{ENABLED}[capture]\nfull_screen = true\n"));
        assert!(validate(&omitted, &open, None).ok());
        assert!(validate(&whole, &open, Some(screen)).ok());
    }

    /// focus の対象 (D-050): deny.processes が勝ち、allow.processes があればその中、無ければ通る。
    /// 総スイッチが無効なら他の操作と同じく拒否
    #[test]
    fn validate_focus_checks_target_process_against_allow_and_deny() {
        let focus = script(r#"{"type":"focus","process":"Notepad.exe"}"#);
        assert!(validate(&focus, &policy(ENABLED), None).ok());
        let allowed = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        assert!(validate(&focus, &allowed, None).ok(), "正規化して比べる");
        let other = policy(&format!("{ENABLED}[allow]\nprocesses=[\"chrome\"]\n"));
        let v = validate(&focus, &other, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("allow.processes"),
            "{:?}",
            v.issues
        );
        let denied = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nprocesses=[\"notepad.exe\"]\n"
        ));
        let v = validate(&focus, &denied, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("deny.processes"),
            "{:?}",
            v.issues
        );
        assert_eq!(
            validate(&focus, &Policy::closed(), None).error_kind(),
            Some(ErrorKind::PolicyDenied),
            "設定が無ければ focus も拒否"
        );
        let off = policy(&format!("{ENABLED}[deny]\nactions=[\"focus\"]\n"));
        assert_eq!(
            validate(&focus, &off, None).error_kind(),
            Some(ErrorKind::PolicyDenied),
            "deny.actions で止められる"
        );
        // focus が前面になるのを待つ上限 (500 ms) は時間の予算に数える: 1 つなら余裕 1000 と合わせて 1500 で通り、
        // 2 つなら 2000 で limits.max_duration_ms = 1600 を超える
        let tight = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 1600\n"));
        assert!(validate(&focus, &tight, None).ok());
        let two = script(
            r#"{"type":"focus","process":"notepad.exe"},{"type":"focus","process":"notepad.exe"}"#,
        );
        let v = validate(&two, &tight, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(v.issues[0].message.contains("focus"), "{:?}", v.issues);
    }

    /// close の対象 (D-062): focus と同じく対象のプロセスを allow / deny に通し、deny.actions でも止められる。
    /// title は必須 (形の誤り)。消えるのを待つ上限 (1000 ms) は時間の予算に数える
    #[test]
    fn validate_close_checks_process_requires_title_and_counts_budget() {
        let close = script(r#"{"type":"close","process":"Notepad.exe","title":"メモ"}"#);
        assert!(validate(&close, &policy(ENABLED), None).ok());
        let other = policy(&format!("{ENABLED}[allow]\nprocesses=[\"chrome\"]\n"));
        let v = validate(&close, &other, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("allow.processes"),
            "{:?}",
            v.issues
        );
        let off = policy(&format!("{ENABLED}[deny]\nactions=[\"close\"]\n"));
        assert_eq!(
            validate(&close, &off, None).error_kind(),
            Some(ErrorKind::PolicyDenied)
        );
        let only_click = policy(&format!("{ENABLED}[allow]\nactions=[\"click\"]\n"));
        assert_eq!(
            validate(&close, &only_click, None).error_kind(),
            Some(ErrorKind::PolicyDenied),
            "allow.actions を書いた設定では close も書かないと通らない"
        );
        assert_eq!(
            validate(&close, &Policy::closed(), None).error_kind(),
            Some(ErrorKind::PolicyDenied)
        );
        let blank = script(r#"{"type":"close","process":"notepad.exe","title":" "}"#);
        assert_eq!(
            validate(&blank, &policy(ENABLED), None).error_kind(),
            Some(ErrorKind::InvalidScript)
        );
        assert!(
            Script::from_json(
                r#"{"version":1,"actions":[{"type":"close","process":"notepad.exe"}]}"#
            )
            .is_err(),
            "title の無い close は読めない"
        );
        // 1000 + 余裕 1000 = 2000 は 2000 で通り、1999 では超える
        let fits = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 2000\n"));
        assert!(validate(&close, &fits, None).ok());
        let tight = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 1999\n"));
        let v = validate(&close, &tight, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(v.issues[0].message.contains("close"), "{:?}", v.issues);
    }

    /// wait_for の対象 (D-068): 撮影の総スイッチ (capture_enabled) だけで通り、input_enabled は要らない。
    /// capture_enabled = false、deny.actions、allow.actions に無い、allow.processes に無いプロセスは 4。
    /// timeout_ms は時間の予算に全額数え、文に wait_for を含む
    #[test]
    fn validate_wait_for_checks_capture_switch_actions_process_and_budget() {
        let wait = script(r#"{"type":"wait_for","process":"Notepad.exe","timeout_ms":500}"#);
        let capture_only = policy("[general]\ncapture_enabled = true\n");
        assert!(
            validate(&wait, &capture_only, None).ok(),
            "input_enabled は要らない"
        );
        let no_capture = policy("[general]\ninput_enabled = true\n");
        let v = validate(&wait, &no_capture, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("capture_enabled"),
            "{:?}",
            v.issues
        );
        let off = policy(&format!("{ENABLED}[deny]\nactions=[\"wait_for\"]\n"));
        assert_eq!(
            validate(&wait, &off, None).error_kind(),
            Some(ErrorKind::PolicyDenied)
        );
        let only_click = policy(&format!("{ENABLED}[allow]\nactions=[\"click\"]\n"));
        assert_eq!(
            validate(&wait, &only_click, None).error_kind(),
            Some(ErrorKind::PolicyDenied),
            "allow.actions を書いた設定では wait_for も並べないと通らない"
        );
        let allowed = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        assert!(validate(&wait, &allowed, None).ok(), "正規化して比べる");
        let other = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let chrome = script(r#"{"type":"wait_for","process":"chrome.exe"}"#);
        let v = validate(&chrome, &other, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("allow.processes"),
            "{:?}",
            v.issues
        );
        assert_eq!(
            validate(&wait, &Policy::closed(), None).error_kind(),
            Some(ErrorKind::PolicyDenied),
            "設定が無ければ wait_for も拒否"
        );
        // timeout_ms を全額数える: 500 + 余裕 1000 = 1500 は 1500 で通り、1499 では超える
        let fits = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 1500\n"));
        assert!(validate(&wait, &fits, None).ok());
        let tight = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 1499\n"));
        let v = validate(&wait, &tight, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(v.issues[0].message.contains("wait_for"), "{:?}", v.issues);
        // 上限の 600000 は既定の max_duration_ms (600000) に余裕が入らず 4
        let max = script(r#"{"type":"wait_for","process":"notepad.exe","timeout_ms":600000}"#);
        let v = validate(&max, &policy(ENABLED), None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(v.issues[0].message.contains("待ちの合計"), "{:?}", v.issues);
        // 形の誤りは 3
        let blank = script(r#"{"type":"wait_for","process":" "}"#);
        assert_eq!(
            validate(&blank, &policy(ENABLED), None).error_kind(),
            Some(ErrorKind::InvalidScript)
        );
    }

    /// 画面の待ち (D-081) は screenshot と同じ撮影の規則に通る: region の画面外は 3、全画面 (region の省略と
    /// 画面を覆う矩形) は 4、window のプロセスの拒否は 4、capture_enabled = false は 4 (input_enabled は要らない)、
    /// deny.actions の wait_stable は 4。全画面の文は操作の名前で書く。timeout_ms は時間の予算に全額数える
    #[test]
    fn validate_wait_screen_applies_capture_rules() {
        let screen = Some(Rect::new(0, 0, 3840, 2160));
        let p = policy(ENABLED);
        let part = r#""region":{"x":0,"y":0,"w":800,"h":600}"#;
        for kind in ["wait_stable", "wait_change"] {
            let s =
                |extra: &str| script(&format!(r#"{{"type":"{kind}",{extra}"timeout_ms":500}}"#));
            assert!(validate(&s(&format!("{part},")), &p, screen).ok());
            let capture_only = policy("[general]\ncapture_enabled = true\n");
            assert!(
                validate(&s(&format!("{part},")), &capture_only, screen).ok(),
                "{kind}: input_enabled は要らない"
            );
            let outside = s(r#""region":{"x":3800,"y":0,"w":100,"h":10},"#);
            assert_eq!(
                validate(&outside, &p, screen).error_kind(),
                Some(ErrorKind::InvalidScript),
                "{kind}"
            );
            let v = validate(&s(""), &p, None);
            assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied), "{kind}");
            assert!(
                v.issues[0].message.contains("full_screen"),
                "{:?}",
                v.issues
            );
            let whole = s(r#""region":{"x":0,"y":0,"w":3840,"h":2160},"#);
            assert_eq!(
                validate(&whole, &p, screen).error_kind(),
                Some(ErrorKind::PolicyDenied),
                "{kind}"
            );
            let win = s(r#""window":{"process":"Notepad.exe"},"#);
            assert!(validate(&win, &p, screen).ok());
            let denied = policy(&format!("{ENABLED}[deny]\nprocesses=[\"notepad\"]\n"));
            let v = validate(&win, &denied, screen);
            assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied), "{kind}");
            assert!(
                v.issues[0].message.contains("deny.processes"),
                "{:?}",
                v.issues
            );
            let no_capture = policy("[general]\ninput_enabled = true\n");
            let v = validate(&s(&format!("{part},")), &no_capture, screen);
            assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied), "{kind}");
            assert!(
                v.issues[0].message.contains("capture_enabled"),
                "{:?}",
                v.issues
            );
            // 画面の大きさを読めない検証で領域の規則があれば、全画面は判定できない (文は操作の名前で書く)
            let open = policy(&format!(
                "{ENABLED}[deny]\nregions=[\"0,0,10,10\"]\n[capture]\nfull_screen = true\n"
            ));
            let v = validate(&s(""), &open, None);
            assert!(
                v.issues[0]
                    .message
                    .contains(&format!("全画面の {kind} は実行時まで判定できない")),
                "{:?}",
                v.issues
            );
            // timeout_ms を全額数える: 500 + 余裕 1000 = 1500 は 1500 で通り、1499 では超える
            let fits = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 1500\n"));
            assert!(validate(&s(&format!("{part},")), &fits, screen).ok());
            let tight = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 1499\n"));
            assert_eq!(
                validate(&s(&format!("{part},")), &tight, screen).error_kind(),
                Some(ErrorKind::PolicyDenied)
            );
        }
        let off = policy(&format!("{ENABLED}[deny]\nactions=[\"wait_stable\"]\n"));
        let stable = script(&format!(r#"{{"type":"wait_stable",{part}}}"#));
        let v = validate(&stable, &off, screen);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert_eq!(v.issues[0].rule, Some("deny.actions"), "{:?}", v.issues);
        let change = script(&format!(r#"{{"type":"wait_change",{part}}}"#));
        assert!(
            validate(&change, &off, screen).ok(),
            "wait_change は別の種類"
        );
        let only_shot = policy(&format!("{ENABLED}[allow]\nactions=[\"screenshot\"]\n"));
        assert_eq!(
            validate(&stable, &only_shot, screen).error_kind(),
            Some(ErrorKind::PolicyDenied),
            "allow.actions を書いた設定では wait_stable も並べないと通らない"
        );
        // screenshot の全画面の文は今と同じ
        let open = policy(&format!(
            "{ENABLED}[deny]\nregions=[\"0,0,10,10\"]\n[capture]\nfull_screen = true\n"
        ));
        let v = validate(
            &script(r#"{"type":"screenshot","path":"a.png"}"#),
            &open,
            None,
        );
        assert_eq!(
            v.issues[0].message,
            "領域の規則があるので、全画面の screenshot は実行時まで判定できない。region を指定する"
        );
    }

    /// screenshot の window (D-053): 検証では対象のプロセスを allow / deny に通すだけで、矩形は実行時に判定する
    #[test]
    fn validate_screenshot_window_checks_process_and_defers_rect_to_runtime() {
        let shot =
            script(r#"{"type":"screenshot","path":"a.png","window":{"process":"Notepad.exe"}}"#);
        assert!(validate(&shot, &policy(ENABLED), None).ok());
        assert!(validate(&shot, &policy(ENABLED), Some(Rect::new(0, 0, 3840, 2160))).ok());
        // 領域の規則があっても、矩形は実行時に判定する (全画面の省略とは違う)
        let regions = policy(&format!("{ENABLED}[deny]\nregions=[\"0,0,10,10\"]\n"));
        let v = validate(&shot, &regions, None);
        assert!(v.ok(), "{:?}", v.issues);
        let other = policy(&format!("{ENABLED}[allow]\nprocesses=[\"chrome\"]\n"));
        let v = validate(&shot, &other, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("allow.processes"),
            "{:?}",
            v.issues
        );
        let denied = policy(&format!("{ENABLED}[deny]\nprocesses=[\"notepad\"]\n"));
        assert_eq!(
            validate(&shot, &denied, None).error_kind(),
            Some(ErrorKind::PolicyDenied)
        );
    }

    /// 撮影の保存先 (D-075): 形の誤りは設定で緩められない hint つきの 4、save_dirs の外れは hint なしの 4。
    /// `mcp:inline` は save_dirs があっても判定しない
    #[test]
    fn validate_rejects_screenshot_path_outside_shape_or_save_dirs() {
        let shot = |path: &str| {
            let j = serde_json::json!({
                "type": "screenshot", "path": path, "region": { "x": 0, "y": 0, "w": 4, "h": 4 }
            });
            script(&j.to_string())
        };
        let v = validate(&shot("a.txt"), &policy(ENABLED), None);
        assert_eq!(v.issues.len(), 1, "{:?}", v.issues);
        assert_eq!(v.issues[0].code, ErrorKind::PolicyDenied.code());
        assert_eq!(v.issues[0].action_index, Some(0));
        assert!(v.issues[0].message.contains(".png"), "{:?}", v.issues);
        assert_eq!(v.issues[0].hint.as_deref(), Some(SAVE_PATH_HINT));

        let dir = crate::testing::unique_temp_dir("validate-save-dirs");
        let p = policy(&format!(
            "{ENABLED}[capture]\nsave_dirs = ['{}']\n",
            dir.display()
        ));
        let inside = dir.join("x.png");
        let v = validate(&shot(&inside.to_string_lossy()), &p, None);
        assert!(v.ok(), "{:?}", v.issues);
        let v = validate(&shot("C:\\elsewhere\\x.png"), &p, None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("capture.save_dirs"),
            "{:?}",
            v.issues
        );
        assert_eq!(v.issues[0].hint, None, "設定由来なので既定の次の手");
        assert!(validate(&shot(INLINE_PATH), &p, None).ok());
        // 全画面の省略と window の screenshot でも判定する
        let omitted = script(r#"{"type":"screenshot","path":"a.txt"}"#);
        let open = policy(&format!("{ENABLED}[capture]\nfull_screen = true\n"));
        let win = script(r#"{"type":"screenshot","path":"a.txt","window":{"process":"n.exe"}}"#);
        for s in [&omitted, &win] {
            let v = validate(s, &open, None);
            assert_eq!(v.issues.len(), 1, "{:?}", v.issues);
            assert!(v.issues[0].message.contains(".png"), "{:?}", v.issues);
            assert_eq!(v.issues[0].hint.as_deref(), Some(SAVE_PATH_HINT));
        }
    }

    /// 保存先の断りの rule と fix (D-079): 形の誤りは save_path で直し方なし、save_dirs の外れはそのキーと直し方
    #[test]
    fn save_refusal_rules() {
        let cwd = Path::new("C:\\work");
        let r = check_save_target(&policy(ENABLED), "a.txt", cwd).unwrap_err();
        assert_eq!(r.rule, RULE_SAVE_PATH);
        assert_eq!(r.fix, None);
        assert_eq!(r.hint(), Some(SAVE_PATH_HINT));
        let p = policy(&format!("{ENABLED}[capture]\nsave_dirs = ['C:\\Shots']\n"));
        let r = check_save_target(&p, "C:\\elsewhere\\sub\\x.png", cwd).unwrap_err();
        assert_eq!(r.rule, "capture.save_dirs");
        assert_eq!(r.hint(), None);
        let fix = r.fix.expect("設定の層で断ったので直し方がある");
        assert!(
            fix.contains("capture.save_dirs に C:\\elsewhere\\sub を足す"),
            "{fix}"
        );
    }

    /// 問題に rule と fix が写る (D-079): 許可に無いプロセス、組み込みのキー (fix なし)、deny.actions、保存先。
    /// JSON では値があるときだけ出る
    #[test]
    fn validate_issues_carry_rule_and_fix() {
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"calc.exe\"]\n[deny]\nactions=[\"close\"]\n"
        ));
        let v = validate(
            &script(
                r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"win+r"},{"type":"close","process":"calc.exe","title":"x"},{"type":"screenshot","path":"a.txt","region":{"x":0,"y":0,"w":2,"h":2}}"#,
            ),
            &p,
            None,
        );
        let first = &v.issues[0];
        assert_eq!(first.action_index, Some(0));
        assert_eq!(first.rule, Some("allow.processes"));
        let fix = first
            .fix
            .as_deref()
            .expect("基本の層で断ったので直し方がある");
        assert!(
            fix.contains("allow.processes に \"notepad.exe\" を足す"),
            "{fix}"
        );
        let at = |i: usize| v.issues.iter().find(|x| x.action_index == Some(i)).unwrap();
        assert_eq!(at(1).rule, Some(crate::policy::RULE_BUILTIN));
        assert_eq!(at(1).fix, None);
        assert_eq!(at(1).hint.as_deref(), Some(crate::policy::BUILTIN_HINT));
        assert_eq!(at(2).rule, Some("deny.actions"));
        assert!(
            at(2)
                .fix
                .as_deref()
                .unwrap()
                .contains("deny.actions から close を外す"),
            "{:?}",
            at(2)
        );
        assert_eq!(at(3).rule, Some(RULE_SAVE_PATH));
        assert_eq!(at(3).fix, None);
        let json = serde_json::to_value(&v.issues).unwrap();
        assert_eq!(json[0]["rule"], "allow.processes");
        assert!(json[0]["fix"].is_string());
        assert!(json[1].get("fix").is_none(), "{}", json[1]);
        // 形の誤りには rule も fix も出ない
        let shape = validate(&script(r#"{"type":"key","keys":"ctrl+bogus"}"#), &p, None);
        let json = serde_json::to_value(&shape.issues).unwrap();
        assert!(json[0].get("rule").is_none() && json[0].get("fix").is_none());
    }

    /// 上限の判定を Policy に寄せた後も (D-079)、数と待ちの合計の問題は今の文で、操作列全体の問題 (番号なし) のまま
    #[test]
    fn validate_limit_issues_keep_message_and_whole_script_index() {
        let p = policy(&format!(
            "{ENABLED}[limits]\nmax_actions = 1\nmax_duration_ms = 1500\n"
        ));
        let v = validate(
            &script(r#"{"type":"wait","ms":400},{"type":"wait","ms":400}"#),
            &p,
            None,
        );
        assert_eq!(v.issues.len(), 2, "{:?}", v.issues);
        assert!(
            v.issues[0].message.contains("limits.max_actions (1)"),
            "{:?}",
            v.issues
        );
        assert!(
            v.issues[1]
                .message
                .contains("limits.max_duration_ms (1500)"),
            "{:?}",
            v.issues
        );
        assert!(v.issues.iter().all(|i| i.action_index.is_none()));
    }

    /// 既にあるファイルが PNG でなければ (名前が .png でも) 撮る前に断り、設定で緩められない hint を付ける (D-075)
    #[test]
    fn validate_rejects_screenshot_over_existing_non_png() {
        let dir = crate::testing::unique_temp_dir("validate-non-png");
        let text = dir.join("notes.png");
        std::fs::write(&text, "not a png").unwrap();
        let j = serde_json::json!({
            "type": "screenshot", "path": text.to_string_lossy(), "region": { "x": 0, "y": 0, "w": 4, "h": 4 }
        });
        let v = validate(&script(&j.to_string()), &policy(ENABLED), None);
        assert_eq!(v.error_kind(), Some(ErrorKind::PolicyDenied));
        assert!(
            v.issues[0].message.contains("PNG ではない"),
            "{:?}",
            v.issues
        );
        assert_eq!(v.issues[0].hint.as_deref(), Some(SAVE_PATH_HINT));
        assert_eq!(std::fs::read(&text).unwrap(), b"not a png");
    }

    #[test]
    fn validate_limits_and_screen_bounds() {
        let p = policy(&format!(
            "{ENABLED}[limits]\nmax_actions = 1\nmax_duration_ms = 1500\n"
        ));
        let v = validate(
            &script(r#"{"type":"wait","ms":600},{"type":"click","x":5000,"y":5}"#),
            &p,
            Some(Rect::new(0, 0, 3840, 2160)),
        );
        assert_eq!(v.issues.len(), 3, "{:?}", v.issues);
        assert_eq!(v.error_kind(), Some(ErrorKind::InvalidScript));
    }

    #[test]
    fn validate_waits_equal_to_deadline_are_rejected_with_margin() {
        let p = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 5000\n"));
        let at_limit = validate(&script(r#"{"type":"wait","ms":5000}"#), &p, None);
        assert_eq!(at_limit.error_kind(), Some(ErrorKind::PolicyDenied));
        let with_room = validate(&script(r#"{"type":"wait","ms":4000}"#), &p, None);
        assert!(with_room.ok(), "{:?}", with_room.issues);
    }

    #[test]
    fn validate_without_screen_cannot_approve_full_screen_capture_or_pointless_scroll_under_region_rules()
     {
        // 全画面は許す (D-049) 上で、領域の規則の分岐だけを確かめる
        let p = policy(&format!(
            "{ENABLED}[deny]\nregions=[\"0,0,10,10\"]\n[capture]\nfull_screen = true\n"
        ));
        let shot = validate(&script(r#"{"type":"screenshot","path":"a.png"}"#), &p, None);
        assert_eq!(
            shot.error_kind(),
            Some(ErrorKind::PolicyDenied),
            "{:?}",
            shot.issues
        );
        assert!(
            shot.issues[0].message.contains("実行時まで判定できない"),
            "{}",
            shot.issues[0].message
        );
        let scroll = validate(&script(r#"{"type":"scroll","clicks":1}"#), &p, None);
        assert_eq!(scroll.error_kind(), Some(ErrorKind::PolicyDenied));
        // 領域の規則が無ければ通す (全画面の撮影は設定で許す。D-049)
        let plain = policy(&format!("{ENABLED}[capture]\nfull_screen = true\n"));
        assert!(
            validate(
                &script(r#"{"type":"screenshot","path":"a.png"},{"type":"scroll","clicks":1}"#),
                &plain,
                None
            )
            .ok()
        );
    }
}
