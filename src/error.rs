//! 終了コードと、標準出力に出すエラーの形 (D-031)。

use std::fmt;

use serde::Serialize;

/// 失敗の種類。番号は終了コード。既存の番号は変えない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Internal,
    Usage,
    InvalidScript,
    PolicyDenied,
    Config,
    Stopped,
    ForegroundGuard,
    OsError,
    ForbiddenDevEnv,
    /// 人が操作中なので撮らなかった (D-049)
    UserActive,
    /// `focus` で対象を前面にできなかった (D-050)。何も送っていない
    FocusFailed,
    /// 撮る・読む対象のウィンドウが見つからない、最小化されている、または画面の外 (D-053、D-054)。
    /// そのウィンドウには何もしていない (操作の後の撮影なら、それより前の操作は済んでいる)
    WindowNotFound,
    /// `text` で、文字列を持つ要素が無い (D-054)。何もしていない
    ElementNotFound,
    /// 画面がロックされているか、スクリーンセーバーや UAC などの別の画面が前面にある (D-066)。
    /// 待っても再試行しても直らない
    DesktopUnavailable,
    /// `wait_for` の条件が時間内に満たされなかった (D-068)。何も送っていない
    WaitTimeout,
    /// 別の ai-cu が画面を変える操作を走らせていて、待ちの上限までに終わらなかった。何も送っていない (D-113)
    InputBusy,
}

impl ErrorKind {
    pub const ALL: [ErrorKind; 16] = [
        ErrorKind::Internal,
        ErrorKind::Usage,
        ErrorKind::InvalidScript,
        ErrorKind::PolicyDenied,
        ErrorKind::Config,
        ErrorKind::Stopped,
        ErrorKind::ForegroundGuard,
        ErrorKind::OsError,
        ErrorKind::ForbiddenDevEnv,
        ErrorKind::UserActive,
        ErrorKind::FocusFailed,
        ErrorKind::WindowNotFound,
        ErrorKind::ElementNotFound,
        ErrorKind::DesktopUnavailable,
        ErrorKind::WaitTimeout,
        ErrorKind::InputBusy,
    ];

    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Internal => 1,
            ErrorKind::Usage => 2,
            ErrorKind::InvalidScript => 3,
            ErrorKind::PolicyDenied => 4,
            ErrorKind::Config => 5,
            ErrorKind::Stopped => 6,
            ErrorKind::ForegroundGuard => 7,
            ErrorKind::OsError => 8,
            ErrorKind::ForbiddenDevEnv => 9,
            ErrorKind::UserActive => 10,
            ErrorKind::FocusFailed => 11,
            ErrorKind::WindowNotFound => 12,
            ErrorKind::ElementNotFound => 13,
            ErrorKind::DesktopUnavailable => 14,
            ErrorKind::WaitTimeout => 15,
            ErrorKind::InputBusy => 16,
        }
    }

    /// 呼び出し側 (AI) への、次の手の 1 行の案内 (`ai-cu guide` の表と同じ内容)。
    pub fn hint(self) -> &'static str {
        match self {
            ErrorKind::Internal => "ai-cu の内部エラー。入力と出力を控えて人間に伝える",
            ErrorKind::Usage => "引数を直す。`ai-cu --help` と `ai-cu guide` を見る",
            ErrorKind::InvalidScript => {
                "操作列の JSON を直す。書式は `ai-cu schema`。未知のフィールドは書かない"
            }
            ErrorKind::PolicyDenied => {
                "人間の設定で拒否されている。回避せず、何を許可してほしいかを人間に伝える。error.fix があれば、その文をそのまま人間に伝える (設定は自分で書き換えない。今の許可は ai-cu config show で確かめられる)"
            }
            ErrorKind::Config => "設定ファイルが壊れているか見つからない。人間に直してもらう",
            ErrorKind::Stopped => {
                "人間か停止の仕組みが実行を止めた。勝手に再実行せず、人間に確かめる"
            }
            ErrorKind::ForegroundGuard => {
                "何も送っていない。入力が届く先のウィンドウが許可されていないか、前面にない。`ai-cu windows` で対象を確かめ、先に focus で対象を前面にしてから再試行する (一手ずつなら `ai-cu focus`、操作列なら先頭に focus)。focus が 11 なら人間に前面に出してもらう"
            }
            ErrorKind::OsError => {
                "OS の呼び出しが失敗した。少し待って 1 回だけ再試行し、だめなら人間に伝える"
            }
            ErrorKind::ForbiddenDevEnv => {
                "ai-cu の開発用の環境 (AI_CU_DEV_FORBID_REAL_INPUT) で実行している。人間に伝える"
            }
            ErrorKind::UserActive => {
                "何もしていない (shot は撮らず、focus は前面にしていない)。人がマウスやキーボードを使っている最中 (ai-cu 自身の run の直後も含む)。数秒待って 1 回だけ再試行し、だめなら人間に確かめる"
            }
            ErrorKind::FocusFailed => {
                "何も送っていない。対象を前面にできなかった (見つからない、最小化されている、閉じられた、または Windows が前面の切り替えを断った。どれかは message と reason で分かる)。繰り返さず、人間に対象を前面に出してもらう"
            }
            ErrorKind::WindowNotFound => {
                "対象のウィンドウが見つからない、最小化されている、または画面の外。撮影も読み取りもしていない (操作の後の `--shot` や操作列の `screenshot` なら、それより前の操作は済んでいる)。`ai-cu windows` で実行ファイル名と題名を確かめる。起動した直後なら数秒待つ。最小化は人間に復元してもらう"
            }
            ErrorKind::ElementNotFound => {
                "何もしていない。その名前と種類で文字列を持つ要素が無い。`ai-cu find --window PROCESS` で要素の名前と種類を確かめる (name は部分一致、大文字小文字を無視)"
            }
            // コマンド名を含めない (CLI と MCP で同じ文。D-066)
            ErrorKind::DesktopUnavailable => {
                "画面がロックされているか、スクリーンセーバー、UAC などの別の画面が前面にある。待っても再試行しても直らない。人間に解除を頼み、解除されたと聞いてから、状態を見直して続ける (結果に executed が無ければ、何も送っていない)。撮影を付けない close と、wait_for だけの操作列は、ロック中でも使える"
            }
            // コマンド名を含めない (CLI と MCP で同じ文。D-068)
            ErrorKind::WaitTimeout => {
                "条件が時間内に満たされなかった。何も送っていない (操作列なら、それより前の操作は済んでいる。executed を見る)。同じ条件で繰り返し待たない。windows、find、text、shot で今の状態を見て、条件 (実行ファイル名、題名、要素の名前と種類、文字列) が実際の表示と合っているかを確かめる。画面が進んでいないなら、その原因 (ダイアログ、人の操作、アプリの停止) を見てから次を決める。長く待つなら人間に相談する"
            }
            // コマンドと道具の名前を含めない (CLI と MCP で同じ文。D-113)
            ErrorKind::InputBusy => {
                "何も送っていない。別の ai-cu (別の AI か、人が動かしている ai-cu) が画面を操作している最中で、終わるのを待ちきれなかった。数十秒待ってから 1 回だけやり直す。続けて断られるなら、人間に別の AI の動きを確かめてもらう"
            }
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::Internal => "internal",
            ErrorKind::Usage => "usage",
            ErrorKind::InvalidScript => "invalid_script",
            ErrorKind::PolicyDenied => "policy_denied",
            ErrorKind::Config => "config",
            ErrorKind::Stopped => "stopped",
            ErrorKind::ForegroundGuard => "foreground_guard",
            ErrorKind::OsError => "os_error",
            ErrorKind::ForbiddenDevEnv => "forbidden_dev_env",
            ErrorKind::UserActive => "user_active",
            ErrorKind::FocusFailed => "focus_failed",
            ErrorKind::WindowNotFound => "window_not_found",
            ErrorKind::ElementNotFound => "element_not_found",
            ErrorKind::DesktopUnavailable => "desktop_unavailable",
            ErrorKind::WaitTimeout => "wait_timeout",
            ErrorKind::InputBusy => "input_busy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    /// 操作列の何番目 (0 始まり) で起きたか
    pub action_index: Option<usize>,
    /// 状況に合わせた次の手。無ければ終了コードごとの既定 (`ErrorKind::hint`)
    pub hint: Option<String>,
    /// 同じ終了コードの中での理由 (機械が読む。`reason` の定数。D-056)
    pub reason: Option<&'static str>,
    /// 理由の補足と、ガード・停止・無操作の状況 (機械が読む。7 の対象の `window` と入力の `point`、
    /// 6 の利用者のマウスの `cursor` など、10 の `idle_ms`。題名は入れない。D-056、D-065)
    pub detail: Option<Box<serde_json::Value>>,
    /// 実行器が操作に入った後の失敗で、失敗の時点の進み具合 (`RunReport` と `partial`) を
    /// JSON にしたもの。`to_json` が最上位に展開する (`ok` と `error` は上書きしない。D-065)。
    /// `detail` とともに `Box` なのは、`Result<_, AppError>` を 128 バイト未満に保つ (clippy の result_large_err) ため
    pub progress: Option<Box<serde_json::Value>>,
    /// 設定による拒否の、当たった設定のキー (`error.rule`) と人間向けの直し方 (`error.fix`) (D-079)。
    /// 2 つを `Box` 1 つにまとめるのは、`detail` と同じく `Result<_, AppError>` を 128 バイト未満に保つため
    pub deny: Option<Box<DenyRule>>,
}

/// 設定による拒否の `error.rule` と `error.fix` (D-079)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyRule {
    /// 当たった設定のキー (`policy::CONFIG_RULES` のどれか)、`builtin`、`save_path`
    pub rule: &'static str,
    /// 人間向けの直し方の 1 文。設定で直せないとき、床の場所を引けないときは None
    pub fix: Option<String>,
}

/// `error.reason` の値 (D-056)。既存の値は変えない
pub mod reason {
    /// focus: 対象が見つからない
    pub const NOT_FOUND: &str = "not_found";
    /// focus: 対象が最小化されている
    pub const MINIMIZED: &str = "minimized";
    /// focus: 呼んだ後に対象が閉じられた (または題名が変わった)
    pub const CLOSED: &str = "closed";
    /// focus: 前面は ai-cu を起動した側なのに断られた
    pub const REFUSED_HOST_FRONT: &str = "refused_host_front";
    /// focus: 前面は別のアプリ (人が使っている) で断られた
    pub const REFUSED_OTHER_APP: &str = "refused_other_app";
    /// focus: 断られ、前面が無いか読めない
    pub const REFUSED_UNKNOWN: &str = "refused_unknown";
    /// MCP: 起動した側を特定できないので実入力を断る
    pub const HOST_UNKNOWN: &str = "host_unknown";
    /// close: 対象がモーダルのダイアログを開いていて無効 (D-062)
    pub const TARGET_DISABLED: &str = "target_disabled";
    /// 14: スクリーンセーバーが動いている (D-066)
    pub const SCREENSAVER: &str = "screensaver";
    /// 14: セッションがロックされている (D-067)
    pub const LOCKED: &str = "locked";
    /// 14: 入力デスクトップが自分のもの (Default) ではない、開けない、または GetCursorPos が 5 (D-066)。
    /// WTS でロックと読めたときは `LOCKED` (D-067)。読めなかったロック、UAC、Ctrl+Alt+Del の画面はこちら
    pub const OTHER_DESKTOP: &str = "other_desktop";
    /// 10: キーかマウスのボタンが押されたまま (D-066)
    pub const KEYS_HELD: &str = "keys_held";
    /// 10: 予告の間 (枠を出してから動かすまで) に人の入力があった (D-093)
    pub const LEAD_INPUT: &str = "lead_input";
    /// 7: 1 つの操作のキーの束の途中で、前面が別のプロセスの窓に変わった。それより前の束は送ってある (D-100)
    pub const TARGET_CHANGED: &str = "target_changed";
}

/// JSON にした失敗の error オブジェクト (`to_json()` の `error`) が、人か停止要因に止められた実行のものか (D-093)。
/// `AppError::stopped_by_user` と同じ判定を、AppError が JSON に変わった後 (MCP のクールダウン) で行う
pub fn stopped_by_user_json(error: &serde_json::Value) -> bool {
    error["code"] == ErrorKind::Stopped.code() || error["reason"] == reason::LEAD_INPUT
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        AppError {
            kind,
            message: message.into(),
            action_index: None,
            hint: None,
            reason: None,
            detail: None,
            progress: None,
            deny: None,
        }
    }

    /// 設定による拒否の rule と fix を付ける (D-079)。rule が None なら何もしない
    /// (rule の無い fix は付けない)
    pub fn with_deny(mut self, rule: Option<&'static str>, fix: Option<String>) -> Self {
        if let Some(rule) = rule {
            self.deny = Some(Box::new(DenyRule { rule, fix }));
        }
        self
    }

    /// 人間向けの直し方 (`error.fix`)。無ければ None
    pub fn fix(&self) -> Option<&str> {
        self.deny.as_ref().and_then(|d| d.fix.as_deref())
    }

    pub fn with_reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(Box::new(detail));
        self
    }

    /// 失敗の時点の進み具合を付ける (D-065)。オブジェクトのキーが `to_json` の最上位に入る
    pub fn with_progress(mut self, progress: serde_json::Value) -> Self {
        self.progress = Some(Box::new(progress));
        self
    }

    pub fn at(mut self, index: usize) -> Self {
        self.action_index = Some(index);
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn hint(&self) -> &str {
        self.hint.as_deref().unwrap_or(self.kind.hint())
    }

    /// 人か停止要因に止められた実行か (D-093): 6 (マウス、目印、取り消し、時間の上限) と、予告の間の人の入力の 10
    /// (`reason: lead_input`)。実行器はこれが真なら枠を残さない。JSON 版は `stopped_by_user_json`
    pub fn stopped_by_user(&self) -> bool {
        self.kind == ErrorKind::Stopped || self.reason == Some(reason::LEAD_INPUT)
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "ok": false,
            "error": {
                "code": self.kind.code(),
                "exit_code": self.kind.exit_code(),
                "message": self.message,
                "hint": self.hint(),
                "action_index": self.action_index,
            }
        });
        // 理由と補足は値があるときだけ出す (既存の項目と形は変えない。D-056)
        if let Some(r) = self.reason {
            v["error"]["reason"] = serde_json::json!(r);
        }
        if let Some(d) = &self.detail {
            v["error"]["detail"] = (**d).clone();
        }
        // 設定による拒否の rule と fix も、値があるときだけ出す (D-079)
        if let Some(d) = &self.deny {
            v["error"]["rule"] = serde_json::json!(d.rule);
            if let Some(f) = &d.fix {
                v["error"]["fix"] = serde_json::json!(f);
            }
        }
        // 進み具合は最上位に、成功と同じ名前で足す。`ok` と `error` は上書きしない (D-065)
        let progress = self.progress.as_deref().and_then(|p| p.as_object());
        if let (Some(p), Some(top)) = (progress, v.as_object_mut()) {
            for (k, val) in p {
                if k == "ok" || k == "error" {
                    continue;
                }
                top.insert(k.clone(), val.clone());
            }
        }
        v
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.action_index {
            // 検証の問題をまとめた文は、既に各問題に添字を付けている
            Some(i) if !self.message.starts_with(&format!("actions[{i}]")) => {
                write!(f, "[{}] actions[{i}]: {}", self.kind.code(), self.message)
            }
            _ => write!(f, "[{}] {}", self.kind.code(), self.message),
        }
    }
}

impl std::error::Error for AppError {}

#[cfg(test)]
mod tests {
    use super::*;

    const README: &str = include_str!("../README.md");

    /// `heading` と等しい行 (末尾の空白を除く) の次から、次の `## ` の行の手前までにある表の行を
    /// (コード、名前、3 列目) で読む。1 列目が数でない行 (見出しの行、区切りの行) は外す。
    /// 名前はバッククォートを外し、欠けた列は空の文字列にする。見出しが無ければ None (D-076)
    fn exit_code_rows(doc: &str, heading: &str) -> Option<Vec<(i32, String, String)>> {
        let mut lines = doc.lines();
        lines.by_ref().find(|l| l.trim_end() == heading)?;
        let mut rows = Vec::new();
        for line in lines.take_while(|l| !l.starts_with("## ")) {
            if !line.starts_with('|') {
                continue;
            }
            let cols: Vec<&str> = line.split('|').collect();
            let col = |i: usize| cols.get(i).map(|s| s.trim()).unwrap_or("");
            let Ok(code) = col(1).parse::<i32>() else {
                continue;
            };
            rows.push((
                code,
                col(2).trim_matches('`').to_string(),
                col(3).to_string(),
            ));
        }
        Some(rows)
    }

    fn expected(zero_name: &str) -> Vec<(i32, String)> {
        std::iter::once((0, zero_name.to_string()))
            .chain(
                ErrorKind::ALL
                    .iter()
                    .map(|k| (k.exit_code(), k.code().to_string())),
            )
            .collect()
    }

    #[test]
    fn exit_code_rows_reads_only_the_table_under_heading() {
        let doc = "| 4 | policy_denied | x |\n## 終了コード\n\n| コード | 名前 | 意味 |\n|---|---|---|\n| 0 | - | 成功 |\n| 1 | `internal` | 内部 |\n\n## 次\n| 4 | policy_denied | x |\n";
        assert_eq!(
            exit_code_rows(doc, "## 終了コード"),
            Some(vec![
                (0, "-".to_string(), "成功".to_string()),
                (1, "internal".to_string(), "内部".to_string()),
            ])
        );
        assert_eq!(exit_code_rows("| 0 | - | 成功 |\n", "## 終了コード"), None);
        assert_eq!(
            exit_code_rows("## 終了コード\n| 13 | x |\n", "## 終了コード"),
            Some(vec![(13, "x".to_string(), String::new())])
        );
    }

    /// README と guide の終了コードの表が、`ErrorKind` の並びと完全に一致する (D-076)
    #[test]
    fn exit_code_tables_in_readme_and_guide_match_error_kinds() {
        let docs = [
            ("README.md", README, "## 終了コード", "-"),
            (
                "src/guide.md",
                crate::cli::GUIDE,
                "## 終了コードと次の手",
                "成功",
            ),
        ];
        for (file, doc, heading, zero_name) in docs {
            let rows = exit_code_rows(doc, heading)
                .unwrap_or_else(|| panic!("{file} に見出し {heading} が無い"));
            let got: Vec<(i32, String)> = rows.iter().map(|(c, n, _)| (*c, n.clone())).collect();
            assert_eq!(
                got,
                expected(zero_name),
                "{file} の {heading} の表が ErrorKind と合わない"
            );
            for (code, _, text) in &rows {
                assert!(
                    !text.is_empty(),
                    "{file} の {heading} の表で、コード {code} の 3 列目が空"
                );
            }
        }
    }

    /// `--help` の after_help の終了コードの行が、0 と `ALL` の順の番号をそろえて持つ (D-076)
    #[test]
    fn help_exit_code_line_lists_every_code_in_order() {
        let help = <crate::cli::Cli as clap::CommandFactory>::command()
            .get_after_help()
            .expect("after_help が無い")
            .to_string();
        let line = help
            .lines()
            .find_map(|l| l.strip_prefix("終了コード: "))
            .expect("after_help に「終了コード: 」で始まる行が無い");
        let got: Vec<i32> = line
            .split(" / ")
            .map(|item| {
                let head = item.split_whitespace().next().unwrap_or("");
                head.parse::<i32>()
                    .unwrap_or_else(|_| panic!("after_help の項目 {item:?} が数で始まらない"))
            })
            .collect();
        let want: Vec<i32> = std::iter::once(0)
            .chain(ErrorKind::ALL.iter().map(|k| k.exit_code()))
            .collect();
        assert_eq!(
            got, want,
            "after_help の終了コードの並びが ErrorKind と合わない"
        );
    }

    /// `ErrorKind::ALL` が enum の種類をすべて持つ (重なりが無いことは
    /// `exit_codes_are_unique_and_stable` が見る)。enum の行を字面で数える (D-076)
    #[test]
    fn error_kind_all_has_every_variant() {
        let src = include_str!("error.rs");
        let mut lines = src.lines();
        lines
            .by_ref()
            .find(|l| l.trim() == "pub enum ErrorKind {")
            .expect("pub enum ErrorKind { の行が無い");
        let count = lines
            .take_while(|l| l.trim() != "}")
            .filter(|l| {
                let Some(name) = l.trim().strip_suffix(',') else {
                    return false;
                };
                name.starts_with(|c: char| c.is_ascii_uppercase())
                    && name.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .count();
        assert_eq!(
            count,
            ErrorKind::ALL.len(),
            "ErrorKind の種類の数と ALL の長さが合わない"
        );
    }

    #[test]
    fn exit_codes_are_unique_and_stable() {
        let codes: Vec<i32> = ErrorKind::ALL.iter().map(|k| k.exit_code()).collect();
        assert_eq!(
            codes,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert_eq!(ErrorKind::WindowNotFound.code(), "window_not_found");
        assert_eq!(ErrorKind::ElementNotFound.code(), "element_not_found");
        assert_eq!(ErrorKind::DesktopUnavailable.code(), "desktop_unavailable");
        assert_eq!(ErrorKind::DesktopUnavailable.exit_code(), 14);
        assert_eq!(ErrorKind::WaitTimeout.code(), "wait_timeout");
        assert_eq!(ErrorKind::WaitTimeout.exit_code(), 15);
        let mut names: Vec<&str> = ErrorKind::ALL.iter().map(|k| k.code()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ErrorKind::ALL.len());
    }

    #[test]
    fn error_json_shape() {
        let e = AppError::new(ErrorKind::PolicyDenied, "だめ").at(3);
        let v = e.to_json();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "policy_denied");
        assert_eq!(v["error"]["exit_code"], 4);
        assert_eq!(v["error"]["action_index"], 3);
        assert_eq!(v["error"]["hint"], ErrorKind::PolicyDenied.hint());
        for k in ErrorKind::ALL {
            assert!(!k.hint().is_empty(), "{k:?}");
        }
        assert_eq!(e.to_string(), "[policy_denied] actions[3]: だめ");
        let summary = AppError::new(
            ErrorKind::PolicyDenied,
            "actions[3]: だめ / actions[4]: これも",
        )
        .at(3);
        assert_eq!(
            summary.to_string(),
            "[policy_denied] actions[3]: だめ / actions[4]: これも"
        );
    }

    /// 既存の項目はそのまま。reason と detail は値があるときだけ出す (D-056)
    #[test]
    fn error_json_keeps_existing_keys_and_adds_reason_detail_only_when_set() {
        let plain = AppError::new(ErrorKind::FocusFailed, "だめ")
            .at(0)
            .to_json();
        let keys: Vec<&String> = plain["error"].as_object().unwrap().keys().collect();
        assert_eq!(
            keys.len(),
            5,
            "code、exit_code、message、hint、action_index: {keys:?}"
        );
        assert!(plain["error"].get("reason").is_none());
        assert!(plain["error"].get("detail").is_none());
        let e = AppError::new(ErrorKind::FocusFailed, "だめ")
            .at(0)
            .with_reason(reason::REFUSED_HOST_FRONT)
            .with_detail(serde_json::json!({
                "set_foreground": 0,
                "foreground_process": "Zed.exe",
                "foreground_is_host": true,
            }));
        let v = e.to_json();
        for k in ["code", "exit_code", "message", "hint", "action_index"] {
            assert_eq!(v["error"][k], plain["error"][k], "{k}");
        }
        assert_eq!(v["error"]["exit_code"], 11);
        assert_eq!(v["error"]["reason"], "refused_host_front");
        assert_eq!(v["error"]["detail"]["set_foreground"], 0);
        assert_eq!(v["error"]["detail"]["foreground_is_host"], true);
        assert!(
            v["error"]["detail"].get("title").is_none(),
            "題名は入れない"
        );
        // 既定の hint は原因を断定しない (人が別のアプリを使っている、と言い切らない)
        assert!(
            !ErrorKind::FocusFailed
                .hint()
                .contains("人が別のアプリを使っていて"),
            "{}",
            ErrorKind::FocusFailed.hint()
        );
    }

    /// rule と fix は値があるときだけ出す。既存の項目はそのまま (D-079)
    #[test]
    fn to_json_has_rule_and_fix_only_when_set() {
        let plain = AppError::new(ErrorKind::PolicyDenied, "だめ").to_json();
        assert!(plain["error"].get("rule").is_none());
        assert!(plain["error"].get("fix").is_none());
        // rule が None なら fix だけを付けない
        let no_rule = AppError::new(ErrorKind::PolicyDenied, "だめ")
            .with_deny(None, Some("直す".into()))
            .to_json();
        assert_eq!(no_rule, plain);
        let rule_only =
            AppError::new(ErrorKind::PolicyDenied, "だめ").with_deny(Some("builtin"), None);
        assert_eq!(rule_only.fix(), None);
        let v = rule_only.to_json();
        assert_eq!(v["error"]["rule"], "builtin");
        assert!(v["error"].get("fix").is_none());
        let both = AppError::new(ErrorKind::ForegroundGuard, "だめ")
            .at(1)
            .with_deny(Some("allow.processes"), Some("足す".into()));
        assert_eq!(both.fix(), Some("足す"));
        let v = both.to_json();
        assert_eq!(v["error"]["rule"], "allow.processes");
        assert_eq!(v["error"]["fix"], "足す");
        for k in ["code", "exit_code", "message", "hint", "action_index"] {
            assert!(v["error"].get(k).is_some(), "{k}");
        }
        assert!(std::mem::size_of::<Result<(), AppError>>() < 128);
    }

    /// 進み具合は付けたときだけ最上位に出る。`ok` と `error` は上書きしない (D-065)
    #[test]
    fn error_json_has_progress_keys_at_top_only_when_set() {
        let plain = AppError::new(ErrorKind::ForegroundGuard, "だめ")
            .at(2)
            .to_json();
        let top: Vec<&String> = plain.as_object().unwrap().keys().collect();
        assert_eq!(top.len(), 2, "ok と error だけ: {top:?}");
        assert!(plain.get("ok").is_some() && plain.get("error").is_some());

        let v = AppError::new(ErrorKind::ForegroundGuard, "だめ")
            .at(2)
            .with_progress(serde_json::json!({
                "executed": 2,
                "partial": false,
                "screenshots": [],
                "focused": [],
                "elapsed_ms": 5,
                "ok": true,
                "error": 1,
            }))
            .to_json();
        assert_eq!(v["executed"], 2);
        assert_eq!(v["partial"], false);
        assert_eq!(v["elapsed_ms"], 5);
        assert_eq!(v["screenshots"], serde_json::json!([]));
        assert_eq!(v["ok"], false, "ok は上書きしない");
        assert_eq!(v["error"]["code"], plain["error"]["code"]);
        let keys: Vec<&String> = v["error"].as_object().unwrap().keys().collect();
        assert_eq!(
            keys.len(),
            5,
            "code、exit_code、message、hint、action_index: {keys:?}"
        );
        for k in ["code", "exit_code", "message", "hint", "action_index"] {
            assert_eq!(v["error"][k], plain["error"][k], "{k}");
        }
    }

    /// 止められた実行の判定 (D-093) は、AppError と、その `to_json()` の error オブジェクトの両方で同じ答えになる。
    /// 6 は理由を問わず真、予告の間の 10 (`lead_input`) は真、他の 10 と 7 は偽
    #[test]
    fn stopped_by_user_covers_stopped_and_lead_input() {
        let cases = [
            (AppError::new(ErrorKind::Stopped, "マウス"), true),
            (
                AppError::new(ErrorKind::Stopped, "上限").with_reason(reason::KEYS_HELD),
                true,
            ),
            (
                AppError::new(ErrorKind::UserActive, "予告").with_reason(reason::LEAD_INPUT),
                true,
            ),
            (
                AppError::new(ErrorKind::UserActive, "押されたまま").with_reason(reason::KEYS_HELD),
                false,
            ),
            (AppError::new(ErrorKind::UserActive, "無操作"), false),
            (AppError::new(ErrorKind::ForegroundGuard, "前面"), false),
        ];
        for (e, want) in cases {
            assert_eq!(e.stopped_by_user(), want, "{e:?}");
            assert_eq!(stopped_by_user_json(&e.to_json()["error"]), want, "{e:?}");
        }
    }
}
