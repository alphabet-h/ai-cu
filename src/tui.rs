//! 設定を編集する TUI (D-009、D-023、D-041)。「状態 (`App`)」「更新 (`App::update`、純関数)」「描画 (`render`)」に分ける。
//! 端末の初期化と復元、イベントの読み込み、ウィンドウの列挙は `run` だけが行う。テストは `update` と `TestBackend` で行う。
//!
//! 画面は 2 ペイン: 左は見出しでまとめた項目の一覧、右は選んだ項目の詳細と編集。編集は常に右ペインの中で行い、
//! 左の一覧は動かさない。許可を広げる操作 (総スイッチを有効にする、allow を空にする、など) は確認を挟む。
//!
//! `r` で操作列 JSON を選んで検証し、確認とカウントダウンを経て実行できる (D-043)。実行は `ScriptRunner` に頼み、
//! 判定は保存済みの設定ファイルで行う。自分の端末のウィンドウには送らない (D-045)。

use std::path::{Path, PathBuf};
use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect as TuiRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::{self, Config, normalize_class, normalize_process};
use crate::device::WindowInfo;
use crate::error::AppError;
use crate::geometry::Rect;
use crate::keys::KeyCombo;
use crate::run::RunReport;
use crate::runner::{FirstBatch, PreflightReport, ScriptRunner};
use crate::script::{Action, ActionKind, ClickModifier};

pub mod theme;
pub use theme::{Theme, ThemeEnv};

/// 1 ペインに切り替える幅
pub const NARROW_WIDTH: u16 = 80;
const UNDO_LIMIT: usize = 100;
/// 実行までのカウントダウン (利用者が対象のウィンドウを前面にして、マウスから手を離す時間。D-043)
pub const RUN_COUNTDOWN_MS: u64 = 5000;
/// カウントダウン中にキーを待つ間隔 (描画の更新の粒度)
const TICK_MS: u64 = 250;
/// 実行後に、端末に届いた打鍵を読み捨てるときの猶予 (推定。D-045)
const DRAIN_GRACE_MS: u64 = 150;
/// 結果の画面に出す問題と撮影の行数の上限
const MAX_ISSUE_LINES: usize = 8;
const MAX_SHOT_LINES: usize = 5;

// ---------------------------------------------------------------------------
// 項目
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Actions,
    Processes,
    Classes,
    Regions,
    Keys,
}

impl ListKind {
    fn key(self) -> &'static str {
        match self {
            ListKind::Actions => "actions",
            ListKind::Processes => "processes",
            ListKind::Classes => "classes",
            ListKind::Regions => "regions",
            ListKind::Keys => "keys",
        }
    }

    fn name(self) -> &'static str {
        match self {
            ListKind::Actions => "操作",
            ListKind::Processes => "アプリ (プロセス)",
            ListKind::Classes => "ウィンドウクラス",
            ListKind::Regions => "画面領域",
            ListKind::Keys => "キー",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            ListKind::Actions => "チェックで選ぶ (wait は常に許可されるので対象外)",
            ListKind::Processes => "実行ファイル名。例 notepad.exe (大文字小文字と .exe は無視)",
            ListKind::Classes => "ウィンドウクラス名 (大文字小文字は無視)",
            ListKind::Regions => "x,y,w,h (物理ピクセル)。例 0,0,1920,1080",
            ListKind::Keys => "キーの組み合わせ。例 ctrl+s、alt+tab、f5",
        }
    }

    /// 重複の判定に使う比較用の値 (照合と同じ正規化)。
    fn compare_key(self, s: &str) -> String {
        match self {
            ListKind::Processes => normalize_process(s),
            ListKind::Classes => normalize_class(s),
            ListKind::Keys => s
                .parse::<KeyCombo>()
                .map(|k| k.to_string())
                .unwrap_or_else(|_| s.to_lowercase()),
            ListKind::Regions => s
                .parse::<Rect>()
                .map(|r| r.to_string())
                .unwrap_or_else(|_| s.to_string()),
            ListKind::Actions => ActionKind::parse(s)
                .map(|k| k.name().to_string())
                .unwrap_or_else(|| s.to_string()),
        }
    }
}

const KINDS: [ListKind; 5] = [
    ListKind::Actions,
    ListKind::Processes,
    ListKind::Classes,
    ListKind::Regions,
    ListKind::Keys,
];

/// チェックリストに出す操作 (wait は常に許可されるので出さない)
fn checklist_kinds() -> Vec<ActionKind> {
    ActionKind::ALL
        .into_iter()
        .filter(|k| *k != ActionKind::Wait)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    InputEnabled,
    CaptureEnabled,
    /// パスワード欄への入力 (D-090)
    PasswordInput,
    List(Side, ListKind),
    MaxActions,
    MaxDurationMs,
    MaxTextLen,
    SentinelFile,
    MouseThreshold,
    /// 撮影の制限 (D-049)
    CaptureFullScreen,
    CaptureIdleMs,
    /// 撮影の保存先のフォルダ (D-075)
    CaptureSaveDirs,
    /// 毎回の承認を求める MCP の道具 (D-078)
    McpRequireApproval,
    /// 画面を変える操作の前の枠と予告の間 (D-093)
    IndicatorEnabled,
    IndicatorLeadMs,
    IndicatorSound,
    /// TUI の配色 (D-129)
    Theme,
}

pub fn fields() -> Vec<Field> {
    let mut v = vec![
        Field::InputEnabled,
        Field::CaptureEnabled,
        Field::PasswordInput,
    ];
    v.extend(KINDS.iter().map(|k| Field::List(Side::Allow, *k)));
    v.extend(KINDS.iter().map(|k| Field::List(Side::Deny, *k)));
    v.extend([
        Field::MaxActions,
        Field::MaxDurationMs,
        Field::MaxTextLen,
        Field::SentinelFile,
        Field::MouseThreshold,
        Field::CaptureFullScreen,
        Field::CaptureIdleMs,
        Field::CaptureSaveDirs,
        Field::McpRequireApproval,
        Field::IndicatorEnabled,
        Field::IndicatorLeadMs,
        Field::IndicatorSound,
        Field::Theme,
    ]);
    v
}

/// 真偽の項目の現在の値。
fn toggle_value(c: &Config, f: Field) -> bool {
    match f {
        Field::InputEnabled => c.general.input_enabled,
        Field::CaptureEnabled => c.general.capture_enabled,
        Field::PasswordInput => c.general.password_input,
        Field::CaptureFullScreen => c.capture.full_screen,
        Field::IndicatorEnabled => c.indicator.enabled,
        Field::IndicatorSound => c.indicator.sound,
        _ => false,
    }
}

fn set_toggle(c: &mut Config, f: Field, v: bool) {
    match f {
        Field::InputEnabled => c.general.input_enabled = v,
        Field::CaptureEnabled => c.general.capture_enabled = v,
        Field::PasswordInput => c.general.password_input = v,
        Field::CaptureFullScreen => c.capture.full_screen = v,
        Field::IndicatorEnabled => c.indicator.enabled = v,
        Field::IndicatorSound => c.indicator.sound = v,
        _ => {}
    }
}

impl Field {
    pub fn key(self) -> String {
        match self {
            Field::InputEnabled => "general.input_enabled".into(),
            Field::CaptureEnabled => "general.capture_enabled".into(),
            Field::PasswordInput => "general.password_input".into(),
            Field::List(Side::Allow, k) => format!("allow.{}", k.key()),
            Field::List(Side::Deny, k) => format!("deny.{}", k.key()),
            Field::MaxActions => "limits.max_actions".into(),
            Field::MaxDurationMs => "limits.max_duration_ms".into(),
            Field::MaxTextLen => "limits.max_text_len".into(),
            Field::SentinelFile => "stop.sentinel_file".into(),
            Field::MouseThreshold => "stop.mouse_threshold_px".into(),
            Field::CaptureFullScreen => "capture.full_screen".into(),
            Field::CaptureIdleMs => "capture.min_idle_ms".into(),
            Field::CaptureSaveDirs => "capture.save_dirs".into(),
            Field::McpRequireApproval => "mcp.require_approval".into(),
            Field::IndicatorEnabled => "indicator.enabled".into(),
            Field::IndicatorLeadMs => "indicator.lead_ms".into(),
            Field::IndicatorSound => "indicator.sound".into(),
            Field::Theme => "tui.theme".into(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Field::InputEnabled => "実入力 (マウスとキー)",
            Field::CaptureEnabled => "画面撮影",
            Field::PasswordInput => "パスワード欄への入力",
            Field::List(_, k) => k.name(),
            Field::MaxActions => "最大の操作数",
            Field::MaxDurationMs => "最大の実行時間 (ms)",
            Field::MaxTextLen => "最大の文字数 (type)",
            Field::SentinelFile => "停止の目印のファイル",
            Field::MouseThreshold => "停止するマウス移動 (px)",
            Field::CaptureFullScreen => "全画面の撮影",
            Field::CaptureIdleMs => "shot と focus の前の無操作 (ms)",
            Field::CaptureSaveDirs => "撮影の保存先のフォルダ",
            Field::McpRequireApproval => "毎回の承認を求める MCP の道具",
            Field::IndicatorEnabled => "操作の前の枠",
            Field::IndicatorLeadMs => "予告の間 (ミリ秒)",
            Field::IndicatorSound => "予告の音",
            Field::Theme => "テーマ",
        }
    }

    fn section(self) -> &'static str {
        match self {
            Field::InputEnabled | Field::CaptureEnabled | Field::PasswordInput => {
                "全般 (総スイッチ)"
            }
            Field::List(Side::Allow, _) => "許可リスト (allow)",
            Field::List(Side::Deny, _) => "拒否リスト (deny)",
            Field::MaxActions | Field::MaxDurationMs | Field::MaxTextLen => "上限",
            Field::SentinelFile | Field::MouseThreshold => "緊急停止",
            Field::CaptureFullScreen | Field::CaptureIdleMs | Field::CaptureSaveDirs => {
                "撮影の制限"
            }
            Field::McpRequireApproval => "MCP",
            Field::IndicatorEnabled
            | Field::IndicatorLeadMs
            | Field::IndicatorSound
            | Field::Theme => "表示",
        }
    }

    /// 左の一覧での場所「<節> > <項目>」(D-080)。狭い幅の右の枠の題と、床の fix の表示名 (`policy::tui_label`) の正
    fn location(self) -> String {
        format!("{} > {}", self.section(), self.name())
    }

    fn description(self) -> Vec<String> {
        let v: Vec<&str> = match self {
            Field::InputEnabled => vec![
                "有効にすると、ai-cu が本物のマウスとキーボードを動かせる。",
                "無効なら run はすべて拒否される (validate と --dry-run は使える)。",
            ],
            Field::CaptureEnabled => vec![
                "有効にすると、shot と screenshot で画面を撮れる。",
                "撮った画像には画面上の個人情報が写りうる。",
            ],
            Field::PasswordInput => vec![
                "有効にすると、パスワード欄 (UIA の IsPassword) にフォーカスがあるときも type と key を送れる。",
                "既定はオフで、パスワードは人が打つ (AI の type / key は終了コード 4 で断る)。",
            ],
            Field::List(Side::Allow, _) => vec![
                "空なら、この次元は制限しない。",
                "1 件でもあれば、一致したものだけを許可する (それ以外は拒否)。",
                "拒否リストに同じ値があれば、拒否が優先する。",
            ],
            Field::List(Side::Deny, _) => vec!["一致したものを常に拒否する (許可リストより強い)。"],
            Field::MaxActions => vec!["1 回の操作列に書ける操作の数の上限。"],
            Field::MaxDurationMs => vec![
                "1 回の実行にかけてよい時間の上限。",
                "実行の余裕 1000 ms より大きい値にする。",
            ],
            Field::MaxTextLen => vec!["1 回の type で入力できる文字数の上限。"],
            Field::SentinelFile => vec![
                "このファイルが存在したら、実行を止める (絶対パス)。",
                "空なら使わない。",
            ],
            Field::MouseThreshold => {
                vec!["ai-cu が動かした位置から、これより離れてマウスが動いたら止める。"]
            }
            Field::CaptureFullScreen => vec![
                "有効にすると、region を省略した撮影と、画面全体を覆う矩形の撮影を許す。",
                "無効なら、対象のウィンドウなど一部の領域しか撮れない (作業中の画面が丸ごと写るのを防ぐ)。",
            ],
            Field::CaptureIdleMs => vec![
                "shot と focus (操作列でウィンドウを前面にする) の直前に、人の最後の入力からこれ以上の無操作を求める (ミリ秒)。",
                "満たさなければ撮らず、前面にもせずに終了コード 10 で断る。0 なら求めない。ai-cu 自身の run の直後も数える。",
            ],
            Field::CaptureSaveDirs => vec![
                "撮影の保存先を、このフォルダのどれかの下に限る。",
                "絶対パスを ; で区切る。空なら制限なし。",
            ],
            Field::McpRequireApproval => vec![
                "書いた道具を呼ぶたびに、クライアント (Claude Code) が人に確認を出す (auto のモードや allow のルールでも省かない)。",
                "ai-cu の守りではなく客への依頼で、対応しない客は無視する。",
                "設定の変更は客が道具の一覧を取り直したとき (Claude Code では /mcp の再接続) に効く。",
                "道具の名前を , で区切る。",
            ],
            Field::IndicatorEnabled => vec![
                "有効にすると、画面を変える操作 (マウス、キー、文字入力、focus、close) の前に、画面の縁に青い枠と案内の札を出す。",
                "枠が出ていなければ予告の間を待ってから動かす。無効なら予告の間も音も無い。",
                "どれかの層で有効なら出す (追加の層 (--config など) に [indicator] が無ければ、その層は既定の有効で加わる)。",
            ],
            Field::IndicatorLeadMs => vec![
                "枠を出してから最初の操作までの予告の間 (ミリ秒)。0 なら待たない。",
                "枠が予告の間より前から出ていれば待たず、出て間もなければ残りだけ待つ。その間にマウスを動かすかキーに触れると、何も送らずに止まる。",
                "層の最大を使う (上の層は延ばせるが縮められない)。",
                "TUI からの実行は枠を出すが待たず、予告の音も鳴らない (実行前のカウントダウンがあるため)。",
            ],
            Field::IndicatorSound => vec![
                "有効にすると、予告の間に入るとき (待つときだけ) に Windows の通知音を 1 回鳴らす。",
                "操作の前の枠が無効なら鳴らない。どれかの層で有効なら鳴らす。",
            ],
            Field::Theme => vec![
                "TUI の配色。auto は環境で選ぶ (NO_COLOR なら mono、VT が使えなければ ansi、それ以外は calm)。明るい背景なら calm-light。",
                "calm と calm-light は VT が使えない端末では ansi で表示する。ansi は端末の 16 色、mono は色を使わない。",
                "切り替えるとその場で反映される。許可には関わらない。",
            ],
        };
        let mut out: Vec<String> = v.into_iter().map(String::from).collect();
        if let Field::List(_, k) = self {
            out.push(format!("書き方: {}", k.hint()));
        }
        if self == Field::IndicatorLeadMs {
            out.push(format!(
                "上限 {} (超えると保存しない)。",
                config::INDICATOR_LEAD_MAX_MS
            ));
        }
        out
    }

    fn is_list(self) -> bool {
        matches!(self, Field::List(..))
    }
}

fn list_mut(cfg: &mut Config, side: Side, kind: ListKind) -> &mut Vec<String> {
    let rules = match side {
        Side::Allow => &mut cfg.allow,
        Side::Deny => &mut cfg.deny,
    };
    match kind {
        ListKind::Actions => &mut rules.actions,
        ListKind::Processes => &mut rules.processes,
        ListKind::Classes => &mut rules.classes,
        ListKind::Regions => &mut rules.regions,
        ListKind::Keys => &mut rules.keys,
    }
}

pub fn list_ref(cfg: &Config, side: Side, kind: ListKind) -> &Vec<String> {
    let rules = match side {
        Side::Allow => &cfg.allow,
        Side::Deny => &cfg.deny,
    };
    match kind {
        ListKind::Actions => &rules.actions,
        ListKind::Processes => &rules.processes,
        ListKind::Classes => &rules.classes,
        ListKind::Regions => &rules.regions,
        ListKind::Keys => &rules.keys,
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "有効" } else { "無効" }
}

/// 左ペインに出す短い値。
fn short_value(cfg: &Config, f: Field) -> String {
    match f {
        Field::InputEnabled => on_off(cfg.general.input_enabled).into(),
        Field::CaptureEnabled => on_off(cfg.general.capture_enabled).into(),
        Field::PasswordInput => on_off(cfg.general.password_input).into(),
        Field::List(side, kind) => {
            let l = list_ref(cfg, side, kind);
            match (l.len(), side) {
                (0, Side::Allow) => "制限なし".into(),
                (0, Side::Deny) => "なし".into(),
                (n, _) => format!("{n} 件: {}", l.join(", ")),
            }
        }
        Field::MaxActions => cfg.limits.max_actions.to_string(),
        Field::MaxDurationMs => cfg.limits.max_duration_ms.to_string(),
        Field::MaxTextLen => cfg.limits.max_text_len.to_string(),
        Field::SentinelFile => {
            if cfg.stop.sentinel_file.is_empty() {
                "使わない".into()
            } else {
                cfg.stop.sentinel_file.clone()
            }
        }
        Field::MouseThreshold => cfg.stop.mouse_threshold_px.to_string(),
        Field::CaptureFullScreen => on_off(cfg.capture.full_screen).into(),
        Field::CaptureIdleMs => cfg.capture.min_idle_ms.to_string(),
        Field::CaptureSaveDirs => {
            if cfg.capture.save_dirs.is_empty() {
                "制限なし".into()
            } else {
                cfg.capture.save_dirs.join(";")
            }
        }
        Field::McpRequireApproval => {
            if cfg.mcp.require_approval.is_empty() {
                "なし".into()
            } else {
                cfg.mcp.require_approval.join(", ")
            }
        }
        Field::IndicatorEnabled => on_off(cfg.indicator.enabled).into(),
        Field::IndicatorLeadMs => cfg.indicator.lead_ms.to_string(),
        Field::IndicatorSound => on_off(cfg.indicator.sound).into(),
        Field::Theme => cfg.tui.theme.as_str().into(),
    }
}

/// 項目の値の表示。`short_value` に、環境で決まる情報を添える。
/// テーマは、`auto` なら選ばれたテーマを `(auto: calm)` の形で添え、VT が無くて RGB のテーマから落ちたときは落ち先を添える (D-129)
fn shown_value(app: &App, f: Field) -> String {
    match f {
        Field::Theme => {
            let chosen = app.config.tui.theme;
            let resolved = app.theme().name;
            if chosen == config::ThemeName::Auto {
                format!("(auto: {})", resolved.as_str())
            } else if chosen == resolved {
                chosen.as_str().into()
            } else {
                format!("{}→{}", chosen.as_str(), resolved.as_str())
            }
        }
        _ => short_value(&app.config, f),
    }
}

/// `,` で区切った MCP の道具の名前の入力を、trim して空を捨てた一覧にする (検査は `config::normalized_mcp_tools`)。
fn split_mcp_tools(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// `;` で区切った保存先のフォルダの入力を、trim して空を捨てた一覧にする。
fn split_save_dirs(s: &str) -> Vec<String> {
    s.split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(String::from)
        .collect()
}

fn scalar_text(cfg: &Config, f: Field) -> String {
    match f {
        Field::SentinelFile => cfg.stop.sentinel_file.clone(),
        Field::CaptureSaveDirs => cfg.capture.save_dirs.join(";"),
        Field::McpRequireApproval => cfg.mcp.require_approval.join(", "),
        _ => short_value(cfg, f),
    }
}

fn field_changed(a: &Config, b: &Config, f: Field) -> bool {
    match f {
        Field::List(side, kind) => list_ref(a, side, kind) != list_ref(b, side, kind),
        _ => scalar_text(a, f) != scalar_text(b, f),
    }
}

// ---------------------------------------------------------------------------
// 入力の検証 (その場で結果を出す)
// ---------------------------------------------------------------------------

/// 入力の検証の結果。`Ok` は保存する表記、`warning` は通すが注意を出すもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub value: Result<String, String>,
    pub warning: Option<String>,
}

pub fn check_list_input(side: Side, kind: ListKind, input: &str) -> Checked {
    let s = input.trim();
    let value = match kind {
        ListKind::Actions => ActionKind::parse(s)
            .filter(|k| *k != ActionKind::Wait)
            .map(|k| k.name().to_string())
            .ok_or_else(|| format!("未知の操作名: {s:?}")),
        ListKind::Processes | ListKind::Classes => {
            if s.is_empty() {
                Err("名前を入力する".into())
            } else {
                Ok(s.to_string())
            }
        }
        ListKind::Regions => s.parse::<Rect>().map(|r| r.to_string()),
        ListKind::Keys => s.parse::<KeyCombo>().map(|k| k.to_string()),
    };
    let warning = match (kind, &value) {
        (ListKind::Keys, Ok(v)) if side == Side::Allow => v
            .parse::<KeyCombo>()
            .ok()
            .and_then(|k| k.builtin_denied())
            .map(|_| "組み込みの拒否に当たるので、許可リストに入れても効かない".to_string()),
        (ListKind::Processes, Ok(v)) if v.contains(['\\', '/']) => {
            Some("パスの部分は無視され、ファイル名だけで比べる".into())
        }
        (ListKind::Processes, Ok(v)) if v.contains('.') && !v.to_lowercase().ends_with(".exe") => {
            Some(".exe 以外の拡張子は、そのまま名前の一部として比べる".into())
        }
        _ => None,
    };
    Checked { value, warning }
}

pub fn check_scalar_input(f: Field, input: &str) -> Checked {
    let s = input.trim();
    let positive = |name: &str| -> Result<String, String> {
        s.parse::<u32>()
            .ok()
            .filter(|v| *v > 0)
            .map(|v| v.to_string())
            .ok_or_else(|| format!("{name}は 1 以上の整数"))
    };
    let value = match f {
        Field::MaxActions => positive("最大の操作数"),
        Field::MaxTextLen => positive("最大の文字数"),
        Field::MouseThreshold => positive("移動の閾値"),
        Field::CaptureIdleMs => s
            .parse::<u64>()
            .ok()
            .filter(|v| *v <= i64::MAX as u64)
            .map(|v| v.to_string())
            .ok_or_else(|| "無操作の時間は 0 以上の整数 (ミリ秒)".to_string()),
        // 上限 (config::INDICATOR_LEAD_MAX_MS) は保存の経路 (request_save の Layer::from_config) が断る。判定を 2 か所にしない
        Field::IndicatorLeadMs => s
            .parse::<u64>()
            .ok()
            .filter(|v| *v <= i64::MAX as u64)
            .map(|v| v.to_string())
            .ok_or_else(|| "予告の間は 0 以上の整数 (ミリ秒)".to_string()),
        Field::MaxDurationMs => {
            let margin = crate::validate::DEADLINE_MARGIN_MS;
            s.parse::<u64>()
                .ok()
                .filter(|v| *v > margin && *v <= i64::MAX as u64)
                .map(|v| v.to_string())
                .ok_or_else(|| format!("{margin} より大きい整数"))
        }
        Field::SentinelFile => {
            if !s.is_empty() && config::check_setting_abs_path("stop.sentinel_file", s).is_err() {
                Err("絶対パスで書く (環境変数の展開はしない)。空にすると使わない".into())
            } else {
                Ok(s.to_string())
            }
        }
        Field::CaptureSaveDirs => {
            let dirs = split_save_dirs(s);
            config::normalized_save_dirs(&dirs).map(|_| dirs.join(";"))
        }
        Field::McpRequireApproval => {
            let tools = split_mcp_tools(s);
            config::normalized_mcp_tools(&tools).map(|_| tools.join(", "))
        }
        _ => Err("この項目は入力欄で編集しない".into()),
    };
    Checked {
        value,
        warning: None,
    }
}

// ---------------------------------------------------------------------------
// 保存前の要約: 許可を広げる変更と、知らせを弱める変更
// ---------------------------------------------------------------------------

/// 保存済みの設定 `old` から `new` への変更のうち、許可を広げるか知らせを弱める (どちらも安全を緩める) ものの説明。
pub fn widening_changes(old: &Config, new: &Config) -> Vec<String> {
    let mut out = Vec::new();
    if !old.general.input_enabled && new.general.input_enabled {
        out.push("実入力を有効にする".to_string());
    }
    if !old.general.capture_enabled && new.general.capture_enabled {
        out.push("画面撮影を有効にする".to_string());
    }
    if !old.general.password_input && new.general.password_input {
        out.push("パスワード欄への入力を許す".to_string());
    }
    if !old.capture.full_screen && new.capture.full_screen {
        out.push("全画面の撮影を有効にする".to_string());
    }
    if new.capture.min_idle_ms < old.capture.min_idle_ms {
        out.push(format!(
            "無操作の時間を短くする (人の操作中でも shot と focus を断りにくくなる。{} → {} ms)",
            old.capture.min_idle_ms, new.capture.min_idle_ms
        ));
    }
    // 知らせを弱める変更 (D-093)
    if old.indicator.enabled && !new.indicator.enabled {
        out.push("操作の前の枠をやめる (知らせを弱める。予告の間と音も無くなる)".to_string());
    }
    if new.indicator.lead_ms < old.indicator.lead_ms {
        out.push(format!(
            "予告の間を短くする (知らせを弱める。{} → {} ms)",
            old.indicator.lead_ms, new.indicator.lead_ms
        ));
    }
    if old.indicator.sound && !new.indicator.sound {
        out.push("予告の音をやめる (知らせを弱める)".to_string());
    }
    for kind in KINDS {
        let (o, n) = (
            list_ref(old, Side::Allow, kind),
            list_ref(new, Side::Allow, kind),
        );
        let key = |s: &String| kind.compare_key(s);
        if !o.is_empty() && n.is_empty() {
            out.push(format!(
                "許可リストの{}を空にする (制限なしになる)",
                kind.name()
            ));
        } else if !o.is_empty() {
            let added: Vec<&String> = n
                .iter()
                .filter(|x| !o.iter().any(|y| key(y) == key(x)))
                .collect();
            if !added.is_empty() {
                out.push(format!(
                    "許可リストの{}に追加: {}",
                    kind.name(),
                    added
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        let (o, n) = (
            list_ref(old, Side::Deny, kind),
            list_ref(new, Side::Deny, kind),
        );
        let removed: Vec<&String> = o
            .iter()
            .filter(|x| !n.iter().any(|y| key(y) == key(x)))
            .collect();
        if !removed.is_empty() {
            out.push(format!(
                "拒否リストの{}から削除: {}",
                kind.name(),
                removed
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if new.limits.max_actions > old.limits.max_actions {
        out.push(format!(
            "最大の操作数を上げる ({} → {})",
            old.limits.max_actions, new.limits.max_actions
        ));
    }
    if new.limits.max_duration_ms > old.limits.max_duration_ms {
        out.push(format!(
            "最大の実行時間を延ばす ({} → {})",
            old.limits.max_duration_ms, new.limits.max_duration_ms
        ));
    }
    if new.limits.max_text_len > old.limits.max_text_len {
        out.push(format!(
            "最大の文字数を上げる ({} → {})",
            old.limits.max_text_len, new.limits.max_text_len
        ));
    }
    if new.stop.mouse_threshold_px > old.stop.mouse_threshold_px {
        out.push(format!(
            "停止するマウス移動を大きくする (止まりにくくなる。{} → {})",
            old.stop.mouse_threshold_px, new.stop.mouse_threshold_px
        ));
    }
    if !old.stop.sentinel_file.is_empty() && new.stop.sentinel_file.is_empty() {
        out.push("停止の目印のファイルをやめる".to_string());
    }
    let (o, n) = (&old.capture.save_dirs, &new.capture.save_dirs);
    if !o.is_empty() && n.is_empty() {
        out.push("撮影の保存先の制限をやめる".to_string());
    } else if !o.is_empty() {
        // 旧のどのフォルダの下にも無い新のフォルダが「足す」。畳めなければ安全側で全部を並べる
        let added: Vec<&str> = match (
            config::normalized_save_dirs(o),
            config::normalized_save_dirs(n),
        ) {
            (Ok(od), Ok(nd)) => n
                .iter()
                .zip(&nd)
                .filter(|(_, p)| !od.iter().any(|d| crate::image::path_within(p, d)))
                .map(|(s, _)| s.as_str())
                .collect(),
            _ => n.iter().map(String::as_str).collect(),
        };
        if !added.is_empty() {
            out.push(format!(
                "撮影の保存先のフォルダを足す ({})",
                added.join(", ")
            ));
        }
    }
    // 正規化は config::normalized_mcp_tools の 1 か所。古い側が読めない値なら、古い設定では効いていなかったので挙げない
    if let (Ok(o), Ok(n)) = (
        config::normalized_mcp_tools(&old.mcp.require_approval),
        config::normalized_mcp_tools(&new.mcp.require_approval),
    ) {
        let removed: Vec<&str> = o
            .iter()
            .filter(|t| !n.contains(t))
            .map(String::as_str)
            .collect();
        if !removed.is_empty() {
            out.push(format!(
                "毎回の承認をやめる MCP の道具: {}",
                removed.join(", ")
            ));
        }
    }
    out
}

/// 実入力が有効で、対象のアプリと画面がまったく絞られていないか。
pub fn unrestricted(cfg: &Config) -> bool {
    cfg.general.input_enabled
        && cfg.allow.processes.is_empty()
        && cfg.allow.classes.is_empty()
        && cfg.allow.regions.is_empty()
}

// ---------------------------------------------------------------------------
// 1 行の入力欄
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineEditor {
    chars: Vec<char>,
    cursor: usize,
    /// 開いた直後で、最初の文字の入力で全体を置き換える
    fresh: bool,
}

impl LineEditor {
    pub fn new(initial: &str) -> Self {
        let chars: Vec<char> = initial.chars().collect();
        LineEditor {
            cursor: chars.len(),
            fresh: !chars.is_empty(),
            chars,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// キーを処理する。処理したら true。
    pub fn handle(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            // AltGr は CONTROL と ALT の組で届くので、文字として受け付ける
            KeyCode::Char(c) if !c.is_control() && (!ctrl || alt) => {
                if self.fresh {
                    self.chars.clear();
                    self.cursor = 0;
                }
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Char('u') if ctrl => {
                self.chars.clear();
                self.cursor = 0;
            }
            KeyCode::Backspace => {
                if self.fresh {
                    self.chars.clear();
                    self.cursor = 0;
                } else if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.fresh {
                    self.chars.clear();
                    self.cursor = 0;
                } else if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => return false,
        }
        self.fresh = false;
        true
    }

    /// 幅 `width` の欄に出す文字列と、欄の左端からのカーソルの表示位置。長い値は横にずらす。
    pub fn view(&self, width: usize) -> (String, usize) {
        let width = width.max(1);
        let w = |c: &char| c.width().unwrap_or(0);
        let before: usize = self.chars[..self.cursor].iter().map(w).sum();
        let mut start = 0;
        let mut offset = 0;
        while before - offset >= width && start < self.cursor {
            offset += w(&self.chars[start]);
            start += 1;
        }
        let mut shown = String::new();
        let mut used = 0;
        for c in &self.chars[start..] {
            if used + w(c) > width {
                break;
            }
            used += w(c);
            shown.push(*c);
        }
        (shown, before - offset)
    }
}

// ---------------------------------------------------------------------------
// 状態
// ---------------------------------------------------------------------------

/// 候補の一覧に出すウィンドウ (TUI の外で列挙して渡す)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCandidate {
    pub process: String,
    pub class: String,
    pub title: String,
}

/// 編集するファイルの種類 (D-029)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// 追加の層 (`--config` か `AI_CU_CONFIG`)。allow は床より広げられない
    pub overlay: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Ok,
    Warn,
    Error,
}

/// 確認のあとに行う操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Enable(Field),
    SetList(Side, ListKind, Vec<String>),
    Save { then_quit: bool },
}

/// 候補の一覧の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub detail: String,
}

/// 操作列 JSON の候補 (TUI の外で列挙して渡す)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptCandidate {
    pub name: String,
    pub path: PathBuf,
}

/// 実行の要求 (`Effect::RunScript`)。事前検査で見た操作列をそのまま走らせる。
#[derive(Debug, Clone, PartialEq)]
pub struct RunRequest {
    pub report: PreflightReport,
    /// 武装の瞬間の前面 (自分の端末)。届く先がこれなら送らない (D-045)。取れなければ実行しない
    pub self_window: WindowInfo,
}

/// 右ペインの上に重ねて出すもの。
#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    /// 数値と文字列、リストの値の編集 (`item` が Some なら既存の値の書き換え)
    Edit {
        field: Field,
        item: Option<usize>,
        editor: LineEditor,
    },
    /// 操作のチェックリスト
    Checklist {
        side: Side,
        checked: Vec<bool>,
        cursor: usize,
    },
    /// 候補から選ぶか、入力欄で絞り込んで手入力する
    Picker {
        side: Side,
        kind: ListKind,
        filter: LineEditor,
        cursor: usize,
    },
    Confirm {
        title: String,
        lines: Vec<String>,
        pending: Pending,
    },
    QuitDialog,
    Help,
    /// 操作列 JSON を一覧から選ぶか、パスを手入力する (D-043)
    ScriptPicker {
        filter: LineEditor,
        cursor: usize,
    },
    /// 事前検査の結果を見て、`y` で武装する
    RunConfirm {
        report: PreflightReport,
    },
    /// 実行までのカウントダウン。`self_window` は武装の瞬間の前面 (`set_self_window` で入る)
    Countdown {
        report: PreflightReport,
        remaining_ms: u64,
        self_window: Option<WindowInfo>,
    },
    /// 同期で実行している間に 1 回だけ描く
    Running {
        report: PreflightReport,
    },
    /// 検査と実行の結果。Enter と Esc 以外のキーは無視する
    RunResult {
        title: String,
        lines: Vec<String>,
        ok: bool,
    },
}

/// 更新の結果、呼び出し側 (`run`) にしてほしいこと。
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    Save,
    /// 保存し、成功したら終了する (失敗したら開いたまま)
    SaveAndQuit,
    Quit,
    /// 開いているウィンドウを列挙して `set_windows` で渡す
    LoadWindows,
    /// 作業ディレクトリの操作列 JSON を列挙して `set_scripts` で渡す
    LoadScripts,
    /// この操作列を事前検査して `on_preflight` で渡す (装置に触れない)
    Preflight(PathBuf),
    /// 前面のウィンドウ (自分の端末) を取って `set_self_window` で渡す
    ArmRun,
    /// 実行して `on_run_finished` で渡す。その前に 1 回描く
    RunScript(Box<RunRequest>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub target: Target,
    pub config: Config,
    saved: Config,
    /// 左ペインで選んでいる項目 (`fields()` の添字)
    pub selected: usize,
    pub focus: Focus,
    /// 右ペインのリストの行 (値の行の後ろに「＋ 追加」の行がある)
    pub row: usize,
    pub overlay: Overlay,
    pub status: String,
    pub status_kind: StatusKind,
    undo: Vec<Config>,
    pub windows: Option<Vec<WindowCandidate>>,
    /// 領域の雛形 (名前, 矩形)
    pub screens: Vec<(String, Rect)>,
    /// 開いたファイルが読めなかった (保存すると雛形で置き換え、元は .broken-*.bak に残す)
    pub replace_broken: bool,
    /// 作業ディレクトリの操作列 JSON (None は未読み込み)
    pub scripts: Option<Vec<ScriptCandidate>>,
    /// 最後に検査した操作列 (次に開いたとき入力欄に入れておく)
    last_script: Option<PathBuf>,
    /// 配色の選び方が頼る環境 (`NO_COLOR`、VT の可否)。`run` が 1 回だけ読んで入れる (D-129)
    pub env: ThemeEnv,
}

/// RGB の配色を選んだのに VT が使えず、ansi で描いているときの知らせ (D-129)
const FELL_BACK_MSG: &str = "VT が使えないので ansi の配色で表示している";

impl App {
    pub fn new(target: Target, config: Config) -> App {
        App {
            target,
            saved: config.clone(),
            config,
            selected: 0,
            focus: Focus::Left,
            row: 0,
            overlay: Overlay::None,
            status: String::new(),
            status_kind: StatusKind::Info,
            undo: Vec::new(),
            windows: None,
            screens: Vec::new(),
            replace_broken: false,
            scripts: None,
            last_script: None,
            env: ThemeEnv::default(),
        }
    }

    /// 描画に使う配色。編集中の設定の `[tui] theme` と環境から選ぶ (その場で反映。D-129)
    pub fn theme(&self) -> &'static Theme {
        theme::resolve(self.config.tui.theme, self.env)
    }

    /// ファイルから読む。無ければ既定値。読めなければ既定値で開き、復旧の導線を状態行に出す (D-018)。
    pub fn load(target: Target) -> App {
        let path = target.path.clone();
        if !path.exists() {
            let mut app = App::new(target, Config::default());
            app.set_status(
                StatusKind::Info,
                "新しい設定ファイル (まだ無い)。s で作成する",
            );
            return app;
        }
        match Config::load(&path) {
            Ok(cfg) => App::new(target, cfg),
            Err(e) => {
                let mut app = App::new(target, Config::default());
                app.replace_broken = true;
                app.set_status(
                    StatusKind::Warn,
                    format!("読めない: {} / 既定値で表示中。s で保存すると置き換え、元は .broken-<時刻>.bak に残る", e.message),
                );
                app
            }
        }
    }

    fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status_kind = kind;
        self.status = text.into();
    }

    pub fn dirty(&self) -> bool {
        self.config != self.saved || self.replace_broken
    }

    /// 保存済みと違う項目の数
    pub fn changed_count(&self) -> usize {
        fields()
            .into_iter()
            .filter(|f| field_changed(&self.config, &self.saved, *f))
            .count()
    }

    pub fn field(&self) -> Field {
        fields()[self.selected]
    }

    pub fn set_windows(&mut self, list: Vec<WindowCandidate>) {
        self.windows = Some(list);
    }

    /// 操作列 JSON の一覧を受け取る。読めなければ空の一覧にして、状態行に理由を出す。
    pub fn set_scripts(&mut self, list: Result<Vec<ScriptCandidate>, String>) {
        match list {
            Ok(l) => self.scripts = Some(l),
            Err(e) => {
                self.scripts = Some(Vec::new());
                self.set_status(StatusKind::Error, format!("一覧を読めない: {e}"));
            }
        }
    }

    /// 一覧のうち、名前に絞り込みの文字列を含むもの (大文字小文字は無視)。
    /// 開いた直後 (`fresh`、前回のパスが入っているだけ) は絞り込まない。
    pub fn script_rows(&self, filter: &LineEditor) -> Vec<&ScriptCandidate> {
        let f = if filter.fresh {
            String::new()
        } else {
            filter.text().trim().to_lowercase()
        };
        self.scripts
            .iter()
            .flatten()
            .filter(|s| f.is_empty() || s.name.to_lowercase().contains(&f))
            .collect()
    }

    /// 事前検査の結果を受け取る。通れば確認の画面、問題があれば結果の画面 (武装できない)。
    pub fn on_preflight(&mut self, result: Result<PreflightReport, AppError>) {
        match result {
            Err(e) => {
                self.overlay = Overlay::RunResult {
                    title: "検査できない".into(),
                    lines: error_lines(&e, None),
                    ok: false,
                };
                self.set_status(StatusKind::Error, format!("検査できない: {e}"));
            }
            Ok(report) => {
                self.last_script = Some(report.path.clone());
                if report.runnable() {
                    self.set_status(
                        StatusKind::Info,
                        "検証 OK。内容を確かめて y で実行 (5 秒のカウントダウン)",
                    );
                    self.overlay = Overlay::RunConfirm { report };
                } else {
                    let n = report.validation.issues.len();
                    self.overlay = Overlay::RunResult {
                        title: "検証で問題 (実行しない)".into(),
                        lines: preflight_issue_lines(&report),
                        ok: false,
                    };
                    self.set_status(StatusKind::Error, format!("検証で {n} 件の問題"));
                }
            }
        }
    }

    /// 武装の瞬間の前面のウィンドウ (自分の端末) を受け取る。
    pub fn set_self_window(&mut self, w: Option<WindowInfo>) {
        if let Overlay::Countdown { self_window, .. } = &mut self.overlay {
            *self_window = w;
        }
    }

    /// カウントダウンの時間を進める (純関数)。0 になった瞬間に 1 回だけ実行を頼む。
    pub fn tick(&mut self, elapsed_ms: u64) -> Effect {
        let Overlay::Countdown {
            report,
            remaining_ms,
            self_window,
        } = &mut self.overlay
        else {
            return Effect::None;
        };
        *remaining_ms = remaining_ms.saturating_sub(elapsed_ms);
        if *remaining_ms > 0 {
            return Effect::None;
        }
        // 前面のウィンドウ (自分の端末) が取れていなければ、除外なしでは走らせない (D-045、fail-closed)
        let Some(self_window) = self_window.take() else {
            self.overlay = Overlay::RunResult {
                title: "実行しない".into(),
                lines: vec![
                    "自分の端末のウィンドウを特定できないので、除外なしでは実行しない (何も送っていない)".into(),
                    "この端末を前面にして、もう一度 r から試す".into(),
                ],
                ok: false,
            };
            self.set_status(
                StatusKind::Error,
                "実行しない: 自分の端末のウィンドウを特定できない",
            );
            return Effect::None;
        };
        let req = RunRequest {
            report: report.clone(),
            self_window,
        };
        self.overlay = Overlay::Running {
            report: req.report.clone(),
        };
        self.set_status(StatusKind::Warn, "実行中… 止めるにはマウスを大きく動かす");
        Effect::RunScript(Box::new(req))
    }

    /// 1 周回分の更新: イベントがあれば処理し、その後に経過時間を進める (純関数)。
    /// `run` が毎周回これを呼ぶので、キーが届いた周回でもカウントダウンは実時間で進む。
    pub fn step(&mut self, ev: Option<&Event>, elapsed_ms: u64) -> Vec<Effect> {
        let mut out = Vec::new();
        if let Some(ev) = ev {
            out.push(self.update(ev));
        }
        out.push(self.tick(elapsed_ms));
        out.retain(|e| *e != Effect::None);
        if out.is_empty() {
            out.push(Effect::None);
        }
        out
    }

    /// 実行の結果を受け取る。
    pub fn on_run_finished(&mut self, result: Result<RunReport, AppError>) {
        let script = match &self.overlay {
            Overlay::Running { report } => Some(report.script.clone()),
            _ => None,
        };
        match result {
            Ok(r) => {
                let mut lines = vec![format!(
                    "✓ 成功 (終了コード 0): {} 操作、{} ms",
                    r.executed, r.elapsed_ms
                )];
                for s in r.screenshots.iter().take(MAX_SHOT_LINES) {
                    lines.push(format!(
                        "撮影 actions[{}]: {} ({}, {} {}x{})",
                        s.action_index, s.path, s.x, s.y, s.width, s.height
                    ));
                }
                if r.screenshots.len() > MAX_SHOT_LINES {
                    lines.push(format!("…他 {} 枚", r.screenshots.len() - MAX_SHOT_LINES));
                }
                for c in &r.closed {
                    lines.push(if c.gone && c.hidden {
                        format!(
                            "隠れた actions[{}]: {} \"{}\" (トレイなど。プロセスは動いている)",
                            c.action_index, c.process, c.title
                        )
                    } else if c.gone {
                        format!(
                            "閉じた actions[{}]: {} \"{}\"",
                            c.action_index, c.process, c.title
                        )
                    } else {
                        format!(
                            "⚠ actions[{}]: {} \"{}\" はまだ開いている (保存の確認などを確かめる)",
                            c.action_index, c.process, c.title
                        )
                    });
                }
                self.overlay = Overlay::RunResult {
                    title: "実行した".into(),
                    lines,
                    ok: true,
                };
                self.set_status(StatusKind::Ok, format!("実行: 成功 ({} 操作)", r.executed));
            }
            Err(e) => {
                self.overlay = Overlay::RunResult {
                    title: "実行に失敗".into(),
                    lines: error_lines(&e, script.as_ref()),
                    ok: false,
                };
                self.set_status(StatusKind::Error, format!("実行: {e}"));
            }
        }
    }

    /// 保存の結果を反映する。
    pub fn on_saved(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.saved = self.config.clone();
                self.replace_broken = false;
                self.set_status(
                    StatusKind::Ok,
                    format!("保存した: {}", self.target.path.display()),
                );
            }
            Err(e) => self.set_status(StatusKind::Error, format!("保存できない: {e}")),
        }
    }

    /// 設定を変える前に、取り消し用に今の値を積む。
    fn mutate(&mut self, f: impl FnOnce(&mut Config)) {
        let before = self.config.clone();
        f(&mut self.config);
        if self.config != before {
            self.undo.push(before);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
    }

    /// 候補の一覧の行 (先頭は「入力した値を追加」の行)
    pub fn picker_rows(&self, side: Side, kind: ListKind, filter: &str) -> Vec<Candidate> {
        let existing: Vec<String> = list_ref(&self.config, side, kind)
            .iter()
            .map(|s| kind.compare_key(s))
            .collect();
        let f = filter.trim().to_lowercase();
        let mut rows: Vec<Candidate> = Vec::new();
        let push = |rows: &mut Vec<Candidate>, value: String, detail: String| {
            let key = kind.compare_key(&value);
            if existing.contains(&key) || rows.iter().any(|r| kind.compare_key(&r.value) == key) {
                return;
            }
            if f.is_empty()
                || value.to_lowercase().contains(&f)
                || detail.to_lowercase().contains(&f)
            {
                rows.push(Candidate { value, detail });
            }
        };
        match kind {
            ListKind::Processes | ListKind::Classes => {
                for w in self.windows.iter().flatten() {
                    let value = if kind == ListKind::Processes {
                        w.process.clone()
                    } else {
                        w.class.clone()
                    };
                    if value.is_empty() {
                        continue;
                    }
                    let detail = if kind == ListKind::Processes {
                        format!("{} — {}", w.class, w.title)
                    } else {
                        format!("{} — {}", w.process, w.title)
                    };
                    push(&mut rows, value, detail);
                }
            }
            ListKind::Regions => {
                for (name, r) in &self.screens {
                    push(&mut rows, r.to_string(), name.clone());
                }
            }
            _ => {}
        }
        rows
    }

    // -----------------------------------------------------------------------
    // 更新
    // -----------------------------------------------------------------------

    /// イベントを 1 つ処理する (純関数。端末にも装置にも触れない)。
    pub fn update(&mut self, ev: &Event) -> Effect {
        match ev {
            // Windows は 1 回の打鍵で Press と Release の両方を流すので、Release は無視する (D-023)
            Event::Key(k) if k.kind == KeyEventKind::Release => Effect::None,
            Event::Key(k) => self.on_key(*k),
            _ => Effect::None,
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Effect {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let is_ctrl_c = ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('C'));
        // メッセージの行の一時的なメッセージは、次のキーで消す (キーの案内は別の行に常に出る。D-130)
        if self.status_kind != StatusKind::Info || !self.status.is_empty() {
            self.set_status(StatusKind::Info, "");
        }
        // 実行中と結果の画面では、届いた打鍵 (読み捨てをすり抜けた操作列の ctrl+c など) で終了しない (D-045)
        if matches!(
            self.overlay,
            Overlay::Running { .. } | Overlay::RunResult { .. }
        ) && (is_ctrl_c || ctrl)
        {
            return self.on_key_in_run_screen(k);
        }
        // raw モードでは Ctrl+C はキーとして届く。未保存の変更があれば確認に回し、確認の画面でもう一度押すと強制終了する
        if is_ctrl_c {
            if self.overlay == Overlay::QuitDialog || !self.dirty() {
                return Effect::Quit;
            }
            self.overlay = Overlay::QuitDialog;
            return Effect::None;
        }
        // Ctrl+S: 入力や選択の途中では保存せず、内容を残す。終了の確認では「保存して終了」と同じ
        let typing = matches!(self.overlay, Overlay::Edit { .. } | Overlay::Picker { .. });
        if ctrl && matches!(k.code, KeyCode::Char('s') | KeyCode::Char('S')) {
            match self.overlay {
                Overlay::None | Overlay::Help => {
                    self.overlay = Overlay::None;
                    return self.request_save(false);
                }
                Overlay::QuitDialog => {
                    self.overlay = Overlay::None;
                    return self.request_save(true);
                }
                Overlay::ScriptPicker { .. }
                | Overlay::RunConfirm { .. }
                | Overlay::Countdown { .. }
                | Overlay::Running { .. }
                | Overlay::RunResult { .. } => {
                    self.set_status(StatusKind::Warn, "実行の画面を閉じてから保存する");
                    return Effect::None;
                }
                _ => {
                    self.set_status(
                        StatusKind::Warn,
                        "入力を確定 (Enter) するか取り消して (Esc) から保存する",
                    );
                    return Effect::None;
                }
            }
        }
        match self.overlay.clone() {
            Overlay::Edit {
                field,
                item,
                editor,
            } => return self.on_edit(k, field, item, editor),
            Overlay::Checklist {
                side,
                checked,
                cursor,
            } => return self.on_checklist(k, side, checked, cursor),
            Overlay::Picker {
                side,
                kind,
                filter,
                cursor,
            } => return self.on_picker(k, side, kind, filter, cursor),
            Overlay::Confirm { pending, .. } => return self.on_confirm(k, pending),
            Overlay::QuitDialog => return self.on_quit_dialog(k),
            Overlay::Help => {
                self.overlay = Overlay::None;
                return Effect::None;
            }
            Overlay::ScriptPicker { filter, cursor } => {
                return self.on_script_picker(k, filter, cursor);
            }
            Overlay::RunConfirm { report } => return self.on_run_confirm(k, report),
            Overlay::Countdown { .. } => return self.on_countdown(k),
            // 実行中はキーを受け付けない (同期実行なので届かないが、届いても何もしない)
            Overlay::Running { .. } => return Effect::None,
            Overlay::RunResult { .. } => return self.on_run_result(k),
            Overlay::None => {}
        }
        debug_assert!(!typing);
        // 慣れたキー (D-130): Ctrl+Z は u と同じ、F1 は ? と同じ。入力欄などの最中は上で各場面に渡っている
        if ctrl && matches!(k.code, KeyCode::Char('z') | KeyCode::Char('Z')) {
            self.undo_last();
            return Effect::None;
        }
        match k.code {
            KeyCode::Char('q') => self.quit_or_ask(),
            KeyCode::Char('s') => self.request_save(false),
            KeyCode::Char('r') => self.start_run(),
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.overlay = Overlay::Help;
                Effect::None
            }
            KeyCode::Char('u') => {
                self.undo_last();
                Effect::None
            }
            _ => match self.focus {
                Focus::Left => self.on_left(k),
                Focus::Right => self.on_right(k),
            },
        }
    }

    /// `q` と主画面 (左ペイン) の `Esc`: 未保存があれば終了の確認、無ければ終了 (D-130)
    fn quit_or_ask(&mut self) -> Effect {
        if self.dirty() {
            self.overlay = Overlay::QuitDialog;
            Effect::None
        } else {
            Effect::Quit
        }
    }

    /// `u` と `Ctrl+Z`: 直前の変更を元に戻す
    fn undo_last(&mut self) {
        match self.undo.pop() {
            Some(prev) => {
                self.config = prev;
                self.clamp_row();
                self.set_status(StatusKind::Ok, "直前の変更を元に戻した");
            }
            None => self.set_status(StatusKind::Warn, "元に戻す変更が無い"),
        }
    }

    fn on_left(&mut self, k: KeyEvent) -> Effect {
        let n = fields().len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = (self.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1) % n,
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = n - 1,
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Tab => {
                return self.open_field();
            }
            // 主画面の Esc で終了する (D-130 が D-041 の Esc の部分を置き換える)
            KeyCode::Esc => return self.quit_or_ask(),
            _ => {}
        }
        self.row = 0;
        Effect::None
    }

    /// 左ペインで選んだ項目を開く: 真偽は切り替え、リストは右ペインへ、数値と文字列は入力欄。
    fn open_field(&mut self) -> Effect {
        let f = self.field();
        match f {
            Field::InputEnabled
            | Field::CaptureEnabled
            | Field::PasswordInput
            | Field::CaptureFullScreen => {
                if toggle_value(&self.config, f) {
                    self.mutate(|c| set_toggle(c, f, false));
                    self.set_status(StatusKind::Ok, format!("{}を無効にした", f.name()));
                } else {
                    let mut lines = vec![format!("{}を有効にする。", f.name())];
                    match f {
                        Field::InputEnabled => {
                            lines.push(
                                "ai-cu が本物のマウスとキーボードを動かせるようになる。".into(),
                            );
                            if unrestricted(&Config {
                                general: config::General {
                                    input_enabled: true,
                                    ..self.config.general.clone()
                                },
                                ..self.config.clone()
                            }) {
                                lines.push("⚠ 許可リスト (アプリ、クラス、領域) が空なので、どのアプリにも操作できる。".into());
                            }
                        }
                        Field::CaptureFullScreen => lines.push(
                            "region を省略した撮影で、作業中の画面が丸ごと写るようになる。".into(),
                        ),
                        Field::PasswordInput => lines.push(
                            "パスワード欄にフォーカスがあっても type / key を送るようになる。パスワードを AI に打たせてよいときだけ有効にする。".into(),
                        ),
                        _ => lines.push("画面上の個人情報が写りうる。".into()),
                    }
                    self.overlay = Overlay::Confirm {
                        title: "有効にする".into(),
                        lines,
                        pending: Pending::Enable(f),
                    };
                }
            }
            // 有効にする向きは知らせを強める側なので確認しない。無効にする向きは保存の要約に ⚠ で並ぶ (D-093)
            Field::IndicatorEnabled | Field::IndicatorSound => {
                let v = !toggle_value(&self.config, f);
                self.mutate(|c| set_toggle(c, f, v));
                self.set_status(StatusKind::Ok, format!("{}を{}にした", f.name(), on_off(v)));
            }
            // 配色は許可に関わらないので確認しない。その場で描画に反映する (D-129)
            Field::Theme => {
                self.mutate(|c| c.tui.theme = c.tui.theme.next());
                if theme::fell_back(self.config.tui.theme, self.env) {
                    // 起動時と同じ知らせ。選んだ RGB の配色では描けないことを Warn で伝える
                    self.set_status(StatusKind::Warn, FELL_BACK_MSG);
                } else {
                    self.set_status(
                        StatusKind::Ok,
                        format!("{}を {} にした", f.name(), self.config.tui.theme.as_str()),
                    );
                }
            }
            Field::List(..) => {
                self.focus = Focus::Right;
                self.row = 0;
            }
            _ => {
                self.focus = Focus::Right;
                self.overlay = Overlay::Edit {
                    field: f,
                    item: None,
                    editor: LineEditor::new(&scalar_text(&self.config, f)),
                };
            }
        }
        Effect::None
    }

    fn list_len(&self) -> usize {
        match self.field() {
            Field::List(side, kind) => list_ref(&self.config, side, kind).len(),
            _ => 0,
        }
    }

    fn clamp_row(&mut self) {
        self.row = self.row.min(self.list_len());
    }

    fn on_right(&mut self, k: KeyEvent) -> Effect {
        let f = self.field();
        let Field::List(side, kind) = f else {
            // 数値と文字列の項目は、入力欄を閉じたら左に戻る。キーは捨てずに左で処理する。
            // ただし Esc は右から左へ戻るだけにして、終了には回さない (D-130)
            self.focus = Focus::Left;
            if k.code == KeyCode::Esc {
                return Effect::None;
            }
            return self.on_left(k);
        };
        let len = self.list_len();
        // 行は値の行 (0..len) と「＋ 追加」の行 (len)
        match k.code {
            KeyCode::Left | KeyCode::Esc => self.focus = Focus::Left,
            KeyCode::Up | KeyCode::Char('k') => self.row = (self.row + len) % (len + 1),
            KeyCode::Down | KeyCode::Char('j') => self.row = (self.row + 1) % (len + 1),
            KeyCode::Char('a') | KeyCode::Insert => return self.start_add(side, kind),
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('e') => {
                if self.row >= len || kind == ListKind::Actions {
                    return self.start_add(side, kind);
                }
                let current = list_ref(&self.config, side, kind)[self.row].clone();
                self.overlay = Overlay::Edit {
                    field: f,
                    item: Some(self.row),
                    editor: LineEditor::new(&current),
                };
            }
            KeyCode::Char('d') | KeyCode::Delete if self.row < len => {
                let mut next = list_ref(&self.config, side, kind).clone();
                let removed = next.remove(self.row);
                if side == Side::Allow && next.is_empty() {
                    self.overlay = Overlay::Confirm {
                        title: "許可リストの最後の 1 件を消す".into(),
                        lines: vec![
                            format!(
                                "{removed} を消すと、許可リストの{}が空になる。",
                                kind.name()
                            ),
                            "空の許可リストは「制限なし」を意味する (すべて許可)。".into(),
                        ],
                        pending: Pending::SetList(side, kind, next),
                    };
                } else {
                    self.mutate(|c| *list_mut(c, side, kind) = next);
                    self.clamp_row();
                    self.set_status(
                        StatusKind::Ok,
                        format!("削除した: {removed} (u で元に戻す)"),
                    );
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn start_add(&mut self, side: Side, kind: ListKind) -> Effect {
        match kind {
            ListKind::Actions => {
                let current = list_ref(&self.config, side, kind);
                // 設定ファイルには別名や大文字でも書けるので、解析してから比べる
                let checked = checklist_kinds()
                    .iter()
                    .map(|k| current.iter().any(|c| ActionKind::parse(c) == Some(*k)))
                    .collect();
                self.overlay = Overlay::Checklist {
                    side,
                    checked,
                    cursor: 0,
                };
                Effect::None
            }
            ListKind::Processes | ListKind::Classes | ListKind::Regions => {
                self.overlay = Overlay::Picker {
                    side,
                    kind,
                    filter: LineEditor::default(),
                    cursor: 0,
                };
                if kind != ListKind::Regions && self.windows.is_none() {
                    Effect::LoadWindows
                } else {
                    Effect::None
                }
            }
            ListKind::Keys => {
                self.overlay = Overlay::Edit {
                    field: Field::List(side, kind),
                    item: None,
                    editor: LineEditor::default(),
                };
                Effect::None
            }
        }
    }

    /// リストを新しい値にする。許可リストが空から 1 件以上に変わるときは確認する。
    fn propose_list(&mut self, side: Side, kind: ListKind, next: Vec<String>, done: String) {
        let before = list_ref(&self.config, side, kind);
        if side == Side::Allow && before.is_empty() && !next.is_empty() {
            self.overlay = Overlay::Confirm {
                title: "許可リストを使い始める".into(),
                lines: vec![
                    format!(
                        "許可リストの{}が「制限なし」から「一致したものだけ」に変わる。",
                        kind.name()
                    ),
                    format!("許可するのは: {}", next.join(", ")),
                    "それ以外はすべて拒否される。".into(),
                ],
                pending: Pending::SetList(side, kind, next),
            };
            return;
        }
        if side == Side::Allow && !before.is_empty() && next.is_empty() {
            self.overlay = Overlay::Confirm {
                title: "許可リストを空にする".into(),
                lines: vec![format!(
                    "許可リストの{}が空になり、「制限なし」(すべて許可) になる。",
                    kind.name()
                )],
                pending: Pending::SetList(side, kind, next),
            };
            return;
        }
        self.overlay = Overlay::None;
        self.mutate(|c| *list_mut(c, side, kind) = next);
        self.clamp_row();
        self.set_status(StatusKind::Ok, done);
    }

    fn on_edit(
        &mut self,
        k: KeyEvent,
        field: Field,
        item: Option<usize>,
        mut editor: LineEditor,
    ) -> Effect {
        match k.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                if !field.is_list() {
                    self.focus = Focus::Left;
                }
                self.set_status(StatusKind::Info, "取り消した");
            }
            KeyCode::Enter => {
                let text = editor.text();
                match field {
                    Field::List(side, kind) => {
                        let checked = check_list_input(side, kind, &text);
                        let value = match checked.value {
                            Ok(v) => v,
                            Err(e) => {
                                self.set_status(StatusKind::Error, format!("確定できない: {e}"));
                                return Effect::None;
                            }
                        };
                        let mut next = list_ref(&self.config, side, kind).clone();
                        let dup = next.iter().enumerate().any(|(i, x)| {
                            Some(i) != item && kind.compare_key(x) == kind.compare_key(&value)
                        });
                        if dup {
                            self.set_status(StatusKind::Error, format!("既にある: {value}"));
                            return Effect::None;
                        }
                        let msg = match item {
                            Some(i) => {
                                next[i] = value.clone();
                                format!("変更した: {value}")
                            }
                            None => {
                                next.push(value.clone());
                                self.row = next.len() - 1;
                                format!("追加した: {value}")
                            }
                        };
                        self.propose_list(side, kind, next, msg);
                    }
                    _ => {
                        let checked = check_scalar_input(field, &text);
                        match checked.value {
                            Ok(v) => {
                                self.mutate(|c| apply_scalar(c, field, &v));
                                self.overlay = Overlay::None;
                                self.focus = Focus::Left;
                                self.set_status(
                                    StatusKind::Ok,
                                    format!(
                                        "{} = {}",
                                        field.name(),
                                        short_value(&self.config, field)
                                    ),
                                );
                            }
                            Err(e) => {
                                self.set_status(StatusKind::Error, format!("確定できない: {e}"))
                            }
                        }
                    }
                }
            }
            _ => {
                editor.handle(k);
                self.overlay = Overlay::Edit {
                    field,
                    item,
                    editor,
                };
            }
        }
        Effect::None
    }

    fn on_checklist(
        &mut self,
        k: KeyEvent,
        side: Side,
        mut checked: Vec<bool>,
        mut cursor: usize,
    ) -> Effect {
        let n = checked.len();
        match k.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.set_status(StatusKind::Info, "取り消した");
                return Effect::None;
            }
            KeyCode::Up | KeyCode::Char('k') => cursor = (cursor + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => cursor = (cursor + 1) % n,
            KeyCode::Char(' ') => checked[cursor] = !checked[cursor],
            KeyCode::Char('a') => {
                let all = checked.iter().all(|c| *c);
                checked.iter_mut().for_each(|c| *c = !all);
            }
            KeyCode::Enter => {
                let current = list_ref(&self.config, side, ListKind::Actions).clone();
                // チェックリストに出ない値 (wait など) は、そのまま残す
                let mut next: Vec<String> = current
                    .iter()
                    .filter(|c| {
                        !ActionKind::parse(c).is_some_and(|k| checklist_kinds().contains(&k))
                    })
                    .cloned()
                    .collect();
                next.extend(
                    checklist_kinds()
                        .iter()
                        .zip(&checked)
                        .filter(|(_, c)| **c)
                        .map(|(k, _)| k.name().to_string()),
                );
                let set = |v: &[String]| {
                    let mut s: Vec<String> =
                        v.iter().map(|x| ListKind::Actions.compare_key(x)).collect();
                    s.sort();
                    s.dedup();
                    s
                };
                if set(&next) == set(&current) {
                    self.overlay = Overlay::None;
                    self.set_status(StatusKind::Info, "変更なし");
                    return Effect::None;
                }
                let msg = format!(
                    "操作の{}を更新した",
                    if side == Side::Allow {
                        "許可リスト"
                    } else {
                        "拒否リスト"
                    }
                );
                self.propose_list(side, ListKind::Actions, next, msg);
                return Effect::None;
            }
            _ => {}
        }
        self.overlay = Overlay::Checklist {
            side,
            checked,
            cursor,
        };
        Effect::None
    }

    fn on_picker(
        &mut self,
        k: KeyEvent,
        side: Side,
        kind: ListKind,
        mut filter: LineEditor,
        mut cursor: usize,
    ) -> Effect {
        let rows = self.picker_rows(side, kind, &filter.text());
        // 行 0 は「入力した値を追加」、行 1.. は候補
        let n = rows.len() + 1;
        match k.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.set_status(StatusKind::Info, "取り消した");
                return Effect::None;
            }
            KeyCode::Up => cursor = (cursor + n - 1) % n,
            KeyCode::Down => cursor = (cursor + 1) % n,
            KeyCode::F(5) if kind != ListKind::Regions => {
                self.windows = None;
                self.overlay = Overlay::Picker {
                    side,
                    kind,
                    filter,
                    cursor: 0,
                };
                return Effect::LoadWindows;
            }
            KeyCode::Enter => {
                let value = if cursor == 0 {
                    filter.text()
                } else {
                    rows[cursor - 1].value.clone()
                };
                let checked = check_list_input(side, kind, &value);
                let value = match checked.value {
                    Ok(v) => v,
                    Err(e) => {
                        self.set_status(StatusKind::Error, format!("追加できない: {e}"));
                        return Effect::None;
                    }
                };
                let mut next = list_ref(&self.config, side, kind).clone();
                if next
                    .iter()
                    .any(|x| kind.compare_key(x) == kind.compare_key(&value))
                {
                    self.set_status(StatusKind::Error, format!("既にある: {value}"));
                    return Effect::None;
                }
                next.push(value.clone());
                self.row = next.len() - 1;
                self.propose_list(side, kind, next, format!("追加した: {value}"));
                return Effect::None;
            }
            _ => {
                if filter.handle(k) {
                    cursor = 0;
                }
            }
        }
        self.overlay = Overlay::Picker {
            side,
            kind,
            filter,
            cursor,
        };
        Effect::None
    }

    fn on_confirm(&mut self, k: KeyEvent, pending: Pending) -> Effect {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.overlay = Overlay::None;
                match pending {
                    Pending::Enable(f) => {
                        self.mutate(|c| set_toggle(c, f, true));
                        self.set_status(
                            StatusKind::Warn,
                            format!("{}を有効にした (s で保存)", f.name()),
                        );
                    }
                    Pending::SetList(side, kind, next) => {
                        self.mutate(|c| *list_mut(c, side, kind) = next);
                        self.clamp_row();
                        self.set_status(StatusKind::Ok, "変更した (s で保存、u で元に戻す)");
                    }
                    Pending::Save { then_quit } => {
                        return if then_quit {
                            Effect::SaveAndQuit
                        } else {
                            Effect::Save
                        };
                    }
                }
            }
            _ => {
                self.overlay = Overlay::None;
                self.set_status(StatusKind::Info, "取りやめた");
            }
        }
        Effect::None
    }

    fn on_quit_dialog(&mut self, k: KeyEvent) -> Effect {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Effect::Quit,
            // 保存して終了も、通常の保存と同じく、許可を広げる変更の確認を通す
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.overlay = Overlay::None;
                self.request_save(true)
            }
            _ => {
                self.overlay = Overlay::None;
                self.set_status(StatusKind::Info, "終了を取りやめた");
                Effect::None
            }
        }
    }

    // -----------------------------------------------------------------------
    // 操作列の実行 (D-043)
    // -----------------------------------------------------------------------

    /// `r`: 未保存の変更があれば断る (判定は保存済みの設定で行うので、画面の値と食い違わせない)。
    fn start_run(&mut self) -> Effect {
        if self.dirty() {
            self.set_status(
                StatusKind::Warn,
                "未保存の変更があるので実行しない。実行は保存済みの設定で判定する (s で保存、u で元に戻す)",
            );
            return Effect::None;
        }
        let initial = self
            .last_script
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.overlay = Overlay::ScriptPicker {
            filter: LineEditor::new(&initial),
            cursor: 0,
        };
        if self.scripts.is_none() {
            Effect::LoadScripts
        } else {
            Effect::None
        }
    }

    fn on_script_picker(
        &mut self,
        k: KeyEvent,
        mut filter: LineEditor,
        mut cursor: usize,
    ) -> Effect {
        // 行 0 は「入力したパスを使う」、行 1.. は一覧
        let n = self.script_rows(&filter).len() + 1;
        match k.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.set_status(StatusKind::Info, "取り消した");
                return Effect::None;
            }
            KeyCode::Up => cursor = (cursor + n - 1) % n,
            KeyCode::Down => cursor = (cursor + 1) % n,
            KeyCode::F(5) => {
                self.scripts = None;
                self.overlay = Overlay::ScriptPicker { filter, cursor: 0 };
                return Effect::LoadScripts;
            }
            KeyCode::Enter => {
                let path = if cursor == 0 {
                    let text = filter.text().trim().to_string();
                    if text.is_empty() {
                        self.set_status(StatusKind::Error, "パスを入力するか、一覧から選ぶ");
                        self.overlay = Overlay::ScriptPicker { filter, cursor };
                        return Effect::None;
                    }
                    PathBuf::from(text)
                } else {
                    self.script_rows(&filter)[cursor - 1].path.clone()
                };
                self.overlay = Overlay::ScriptPicker { filter, cursor };
                self.set_status(StatusKind::Info, format!("検査中: {}", path.display()));
                return Effect::Preflight(path);
            }
            _ => {
                if filter.handle(k) {
                    cursor = 0;
                }
            }
        }
        self.overlay = Overlay::ScriptPicker { filter, cursor };
        Effect::None
    }

    /// 確認の画面: `y` で武装してカウントダウンへ。前面のウィンドウ (自分の端末) を取ってもらう。
    fn on_run_confirm(&mut self, k: KeyEvent, report: PreflightReport) -> Effect {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.overlay = Overlay::Countdown {
                    report,
                    remaining_ms: RUN_COUNTDOWN_MS,
                    self_window: None,
                };
                self.set_status(
                    StatusKind::Warn,
                    "対象のウィンドウを前面にして、マウスから手を離す (Esc で取りやめ)",
                );
                Effect::ArmRun
            }
            _ => {
                self.overlay = Overlay::None;
                self.set_status(StatusKind::Info, "取りやめた");
                Effect::None
            }
        }
    }

    /// カウントダウン中: Esc と q だけが取りやめ。他のキーは無視する (ウィンドウの切り替えで届く修飾キーで取り消さない)。
    fn on_countdown(&mut self, k: KeyEvent) -> Effect {
        if matches!(
            k.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q')
        ) {
            self.overlay = Overlay::None;
            self.set_status(StatusKind::Info, "取りやめた (何も送っていない)");
        }
        Effect::None
    }

    /// 結果の画面: Enter と Esc で閉じる。他のキーは無視する (実行後に届いた打鍵への保険。D-045)。
    fn on_run_result(&mut self, k: KeyEvent) -> Effect {
        if matches!(k.code, KeyCode::Enter | KeyCode::Esc) {
            self.overlay = Overlay::None;
            self.set_status(StatusKind::Info, "");
        }
        Effect::None
    }

    /// 実行中と結果の画面に届いた修飾キーつきの打鍵: 何もしない (Ctrl+C でも終了しない)。
    fn on_key_in_run_screen(&mut self, k: KeyEvent) -> Effect {
        match self.overlay {
            Overlay::RunResult { .. } => self.on_run_result(k),
            _ => Effect::None,
        }
    }

    /// 保存の要求。許可を広げる変更か知らせを弱める変更があれば、要約を出して確認する。
    fn request_save(&mut self, then_quit: bool) -> Effect {
        if let Err(e) = config::Layer::from_config(&self.config, "") {
            self.set_status(StatusKind::Error, format!("保存しない: {e}"));
            return Effect::None;
        }
        let save = if then_quit {
            Effect::SaveAndQuit
        } else {
            Effect::Save
        };
        if !self.dirty() {
            if then_quit {
                return Effect::Quit;
            }
            self.set_status(StatusKind::Info, "変更は無い");
            return Effect::None;
        }
        let widening = widening_changes(&self.saved, &self.config);
        if widening.is_empty() {
            return save;
        }
        let mut lines = vec!["次の変更は、許可を広げる (安全を緩める):".to_string()];
        lines.extend(widening.iter().map(|w| format!("⚠ {w}")));
        self.overlay = Overlay::Confirm {
            title: "保存の確認".into(),
            lines,
            pending: Pending::Save { then_quit },
        };
        Effect::None
    }
}

// ---------------------------------------------------------------------------
// 実行の画面に出す文 (純関数)
// ---------------------------------------------------------------------------

/// 操作の 1 行要約 (結果の画面で、失敗した操作の中身を見せる)。
pub fn action_summary(a: &Action) -> String {
    let kind = a.kind().name();
    match a {
        Action::Click {
            x,
            y,
            button,
            modifiers,
        }
        | Action::DoubleClick {
            x,
            y,
            button,
            modifiers,
        } => {
            let mut line = format!("{kind} ({x}, {y}) {button:?}").to_lowercase();
            if !modifiers.is_empty() {
                // 失敗した操作の表示から修飾キーを落とさない。正規の順 (ctrl、alt、shift) で + でつなぐ (D-137)
                let names: Vec<&str> = ClickModifier::canonical(modifiers)
                    .iter()
                    .map(|m| m.modifier().name())
                    .collect();
                line.push(' ');
                line.push_str(&names.join("+"));
            }
            line
        }
        Action::Drag { from, to, .. } => {
            format!("{kind} ({}, {}) → ({}, {})", from.x, from.y, to.x, to.y)
        }
        Action::Move { x, y } => format!("{kind} ({x}, {y})"),
        Action::Scroll {
            clicks, horizontal, ..
        } => format!(
            "{kind} {clicks} ノッチ{}",
            if *horizontal { " (横)" } else { "" }
        ),
        Action::Type { text, .. } => format!("{kind} {} 文字", text.chars().count()),
        Action::Key { keys } => format!("{kind} {keys}"),
        Action::Wait { ms } => format!("{kind} {ms} ms"),
        Action::Screenshot { path, .. } => format!("{kind} → {path}"),
        Action::Focus { process, title } => match title {
            Some(t) => format!("{kind} {process} \"{t}\""),
            None => format!("{kind} {process}"),
        },
        Action::Close { process, title } => format!("{kind} {process} \"{title}\""),
        Action::WaitFor {
            process,
            title,
            element,
            text,
            absent,
            ..
        } => {
            let mut s = format!("{kind} {process}");
            if let Some(t) = title {
                s.push_str(&format!(" \"{t}\""));
            }
            if let Some(f) = element {
                let parts: Vec<&str> = [f.name.as_deref(), f.control_type.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect();
                s.push_str(&format!(" element={}", parts.join("/")));
            }
            if let Some(t) = text {
                s.push_str(&format!(" text=\"{t}\""));
            }
            if *absent {
                s.push_str(" absent");
            }
            s
        }
        // 画面の待ち (D-081): 撮る対象と主な値。既定の threshold も書く (どれだけの差で判定するかが要約で分かる)
        Action::WaitStable {
            region,
            window,
            stable_ms,
            threshold,
            ..
        } => format!(
            "{kind} {} {stable_ms} ms threshold={threshold}",
            capture_target_summary(region.as_ref(), window.as_ref())
        ),
        Action::WaitChange {
            region,
            window,
            threshold,
            ..
        } => format!(
            "{kind} {} threshold={threshold}",
            capture_target_summary(region.as_ref(), window.as_ref())
        ),
    }
}

/// 撮る対象の 1 語の要約 (画面の待ち。D-081): window はプロセス (と題名)、region は矩形、両方無ければ全画面
fn capture_target_summary(
    region: Option<&crate::script::Region>,
    window: Option<&crate::script::WindowTarget>,
) -> String {
    match (region, window) {
        (Some(r), _) => crate::geometry::Rect::from(*r).to_string(),
        (None, Some(w)) => match &w.title {
            Some(t) => format!("{} \"{t}\"", w.process),
            None => w.process.clone(),
        },
        (None, None) => "全画面".to_string(),
    }
}

/// エラーを結果の画面の行にする。`script` があれば、失敗した操作の要約を添える。
fn error_lines(e: &AppError, script: Option<&crate::script::Script>) -> Vec<String> {
    let mut lines = vec![format!(
        "✗ [{}] (終了コード {})",
        e.kind.code(),
        e.kind.exit_code()
    )];
    if let Some(i) = e.action_index {
        match script.and_then(|s| s.actions.get(i)) {
            Some(a) => lines.push(format!("actions[{i}]: {}", action_summary(a))),
            None => lines.push(format!("actions[{i}]")),
        }
    }
    lines.push(e.message.clone());
    lines.push(format!("次の手: {}", e.hint()));
    // 既定の次の手が「error.fix を人間に伝える」と言うので、人間が見る TUI にも直し方を出す (D-079)
    if let Some(fix) = e.fix() {
        lines.push(format!("直し方: {fix}"));
    }
    lines
}

/// 検証の問題を結果の画面の行にする (上限あり。全件は `ai-cu validate`)。
fn preflight_issue_lines(r: &PreflightReport) -> Vec<String> {
    let issues = &r.validation.issues;
    let mut lines = vec![format!(
        "ファイル: {} ({} 操作、{} 件の問題)",
        r.path.display(),
        r.actions(),
        issues.len()
    )];
    for i in issues.iter().take(MAX_ISSUE_LINES) {
        lines.push(match i.action_index {
            Some(n) => format!("[{}] actions[{n}]: {}", i.code, i.message),
            None => format!("[{}] {}", i.code, i.message),
        });
    }
    if issues.len() > MAX_ISSUE_LINES {
        lines.push(format!(
            "…他 {} 件 (`ai-cu validate` で全件)",
            issues.len() - MAX_ISSUE_LINES
        ));
    }
    if let Some(kind) = r.validation.error_kind() {
        let hint = issues
            .iter()
            .find_map(|i| i.hint.clone())
            .unwrap_or_else(|| kind.hint().to_string());
        lines.push(format!("次の手: {hint}"));
        // 直し方は、終了コードと添字を決めた最初の問題のもの (`Validation::first_issue`。D-079)
        if let Some(fix) = r.validation.first_issue().and_then(|i| i.fix.as_deref()) {
            lines.push(format!("直し方: {fix}"));
        }
    }
    lines
}

/// 確認の画面の行 (要約、判定に使う層、停止の手段、注意)。
fn confirm_lines(r: &PreflightReport) -> Vec<String> {
    let mut lines = vec![
        format!("ファイル: {}", r.path.display()),
        format!(
            "操作 {} 件 / バッチ {} / 待ち {} ms / 撮影 {} 枚",
            r.actions(),
            r.batches,
            r.wait_total_ms,
            r.screenshots
        ),
        format!("判定に使う設定 (保存済み): {}", r.layer_sources.join(", ")),
    ];
    let mut stop = format!(
        "停止: マウスを {} px 以上動かす / 最大 {} ms",
        r.mouse_threshold_px, r.max_duration_ms
    );
    if !r.sentinel_files.is_empty() {
        stop.push_str(&format!(" / 目印ファイル {}", r.sentinel_files.join(", ")));
    }
    lines.push(stop);
    lines.push(match r.first_batch {
        Some(FirstBatch::Keyboard) => {
            "⚠ 最初の操作はキー入力: カウントダウンの間に対象のウィンドウを前面にしないと、何も送らずに 7 で止まる".into()
        }
        Some(FirstBatch::Mouse) => {
            "最初の操作はマウス: その点の下のウィンドウに届く (端末が覆っていれば 7 で止まる)".into()
        }
        Some(FirstBatch::Focus) => {
            "最初の操作は focus: 対象のウィンドウを前面にしてから送る (端末が前面のままでよい。カウントダウンの間は触らない。人が操作中なら 10、前面にできなければ 11 で、何も送らずに止まる)".into()
        }
        Some(FirstBatch::Close) => {
            "最初の操作は close: 前面は変えず、対象のウィンドウに WM_CLOSE を置く (前面で人が使っていれば 10、見つからなければ 12 で止まる)".into()
        }
        None => "入力のバッチは無い (待ちと撮影だけ)".into(),
    });
    if r.closes > 0 {
        lines.push(format!(
            "⚠ close が {} 件: 対象のウィンドウを閉じる (タブのあるアプリはタブをすべて閉じる。保存の確認が出ることがある)",
            r.closes
        ));
    }
    if r.unrestricted {
        lines.push(
            "⚠ 許可リスト (アプリ、クラス、領域) が空: どのアプリにも送れる設定。自分の端末には送らない"
                .into(),
        );
    }
    lines.push(format!(
        "y を押すと {} 秒のカウントダウン。その間に対象のウィンドウを前面にして、マウスから手を離す",
        RUN_COUNTDOWN_MS / 1000
    ));
    lines.push("実行中はキーでは止められない。止めるにはマウスを大きく動かす".into());
    lines
}

fn countdown_lines(r: &PreflightReport, remaining_ms: u64) -> Vec<String> {
    vec![
        format!("ファイル: {}", r.path.display()),
        "対象のウィンドウを前面にして、マウスから手を離す".into(),
        format!(
            "取りやめは Esc か q (この端末が前面の間だけ届く)。残り {} 秒",
            remaining_ms.div_ceil(1000)
        ),
    ]
}

fn running_lines(r: &PreflightReport) -> Vec<String> {
    let mut lines = vec![
        format!("ファイル: {}", r.path.display()),
        format!(
            "止めるには: マウスを {} px 以上動かす / 最大 {} ms で止まる",
            r.mouse_threshold_px, r.max_duration_ms
        ),
    ];
    if !r.sentinel_files.is_empty() {
        lines.push(format!(
            "目印ファイルを作る: {}",
            r.sentinel_files.join(", ")
        ));
    }
    lines.push("キーでは止められない (画面は実行が終わるまで更新されない)".into());
    lines
}

/// ディレクトリ直下の `*.json` (再帰しない、名前順)。読み取りだけ。
pub fn list_scripts(dir: &Path) -> Result<Vec<ScriptCandidate>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out: Vec<ScriptCandidate> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .map(|e| ScriptCandidate {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

fn apply_scalar(c: &mut Config, f: Field, v: &str) {
    match f {
        Field::MaxActions => c.limits.max_actions = v.parse().unwrap_or(c.limits.max_actions),
        Field::MaxDurationMs => {
            c.limits.max_duration_ms = v.parse().unwrap_or(c.limits.max_duration_ms)
        }
        Field::MaxTextLen => c.limits.max_text_len = v.parse().unwrap_or(c.limits.max_text_len),
        Field::MouseThreshold => {
            c.stop.mouse_threshold_px = v.parse().unwrap_or(c.stop.mouse_threshold_px)
        }
        Field::SentinelFile => c.stop.sentinel_file = v.to_string(),
        Field::CaptureIdleMs => c.capture.min_idle_ms = v.parse().unwrap_or(c.capture.min_idle_ms),
        Field::CaptureSaveDirs => c.capture.save_dirs = split_save_dirs(v),
        Field::McpRequireApproval => c.mcp.require_approval = split_mcp_tools(v),
        Field::IndicatorLeadMs => c.indicator.lead_ms = v.parse().unwrap_or(c.indicator.lead_ms),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 描画
// ---------------------------------------------------------------------------

/// allow は強調の青 (`accent`)、deny は `danger`。緑は「安全」と読まれるので、実入力を許す側の allow に使わない (D-129)
fn side_color(th: &Theme, side: Side) -> Color {
    match side {
        Side::Allow => th.accent,
        Side::Deny => th.danger,
    }
}

/// 節の見出しの色。allow と deny の節はその色、他は本文の色
fn section_color(th: &Theme, f: Field) -> Color {
    match f {
        Field::List(side, _) => side_color(th, side),
        _ => th.text,
    }
}

/// 本文の基本の書式。色を持たない文字 (項目の名前、説明、帯、ダイアログの本文、入力欄の文字) は
/// 端末の既定の前景ではなく、テーマの `text` の役割で塗る (D-129)
fn body_style(th: &Theme) -> Style {
    Style::new().fg(th.text)
}

/// 実入力とパスワード入力の「有効」。重さを色だけに任せず ⚠ を添える (D-129)
const ON_DANGER: &str = "⚠ 有効";

fn status_style(th: &Theme, kind: StatusKind) -> Style {
    match kind {
        StatusKind::Info => Style::new().fg(th.muted),
        StatusKind::Ok => Style::new().fg(th.ok),
        StatusKind::Warn => Style::new().fg(th.warn),
        StatusKind::Error => Style::new().fg(th.danger).add_modifier(Modifier::BOLD),
    }
}

/// ダイアログの重さ。枠とキーの案内の色を役割で決める (`render_dialog`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Info,
    Ok,
    Warn,
    Danger,
}

impl Tone {
    fn color(self, th: &Theme) -> Color {
        match self {
            Tone::Info => th.accent,
            Tone::Ok => th.ok,
            Tone::Warn => th.warn,
            Tone::Danger => th.danger,
        }
    }
}

/// 有効な状態の要約 (上部の帯)。
fn summary_lines(cfg: &Config, th: &Theme) -> Vec<Line<'static>> {
    let target = |kind: ListKind, all: &str| {
        let l = list_ref(cfg, Side::Allow, kind);
        if l.is_empty() {
            all.to_string()
        } else {
            format!("{} 件", l.len())
        }
    };
    let deny_total: usize = KINDS
        .iter()
        .map(|k| list_ref(cfg, Side::Deny, *k).len())
        .sum();
    let mut first = vec![
        Span::raw("実入力: "),
        if cfg.general.input_enabled {
            Span::styled(
                ON_DANGER,
                Style::new().fg(th.danger).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(on_off(false), Style::new().fg(th.ok))
        },
        Span::raw("  撮影: "),
        Span::styled(
            on_off(cfg.general.capture_enabled),
            if cfg.general.capture_enabled {
                Style::new().fg(th.warn)
            } else {
                Style::new().fg(th.ok)
            },
        ),
        Span::raw(format!(
            "  アプリ: {}  領域: {}  操作: {}  拒否: {} 件",
            target(ListKind::Processes, "全て"),
            target(ListKind::Regions, "全画面"),
            target(ListKind::Actions, "全て"),
            deny_total
        )),
    ];
    if !cfg.general.input_enabled {
        first.push(Span::styled(
            "  (実入力は無効: run はすべて拒否)",
            Style::new().fg(th.muted),
        ));
    }
    let mut out = vec![Line::from(first)];
    if unrestricted(cfg) {
        out.push(Line::styled(
            "⚠ 許可リスト (アプリ、クラス、領域) が空: どのアプリ、どの画面にも実入力できる",
            Style::new().fg(th.danger).add_modifier(Modifier::BOLD),
        ));
    }
    out
}

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let th = app.theme();
    let summary = summary_lines(&app.config, th);
    // 下の 2 行: メッセージの行 (無ければ空行。長ければ 2 行に折る) と、常に出すキーの案内の行 (D-130)
    let status_h = if UnicodeWidthStr::width(app.status.as_str()) > area.width as usize {
        2
    } else {
        1
    };
    let [title, banner, body, status, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(summary.len() as u16),
        Constraint::Fill(1),
        Constraint::Length(status_h),
        Constraint::Length(1),
    ])
    .areas(area);
    // ダイアログはキーの案内の行に重ねない
    let dialog_area = TuiRect {
        height: area.height.saturating_sub(1),
        ..area
    };

    // 題名: ファイルと層の種類と未保存の件数
    let layer = if app.target.overlay {
        Span::styled(
            " [追加の層: allow は基本の層より広げられない]",
            Style::new().fg(th.warn),
        )
    } else {
        Span::styled(" [基本の層: 既定の場所]", Style::new().fg(th.muted))
    };
    let mut t = vec![
        Span::styled(
            "ai-cu 設定",
            Style::new().fg(th.text).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" — {}", app.target.path.display())),
        layer,
    ];
    if app.dirty() {
        let n = app.changed_count();
        t.push(Span::styled(
            format!(" [未保存 {n} 件]"),
            Style::new().fg(th.warn).add_modifier(Modifier::BOLD),
        ));
        t.push(Span::styled(" — ", Style::new().fg(th.muted)));
        t.push(Span::styled(
            "s",
            Style::new().fg(th.warn).add_modifier(Modifier::BOLD),
        ));
        t.push(Span::styled(" で保存", Style::new().fg(th.muted)));
    }
    frame.render_widget(Paragraph::new(Line::from(t)).style(body_style(th)), title);
    frame.render_widget(Paragraph::new(summary).style(body_style(th)), banner);

    if body.width < NARROW_WIDTH {
        // 1 ペイン: 右に入っているとき (操作列の一覧を含む) は右ペインを全画面で出す
        if app.focus == Focus::Right || matches!(app.overlay, Overlay::ScriptPicker { .. }) {
            render_right(app, frame, body, true);
        } else {
            render_left(app, frame, body);
        }
    } else {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .areas(body);
        render_left(app, frame, left);
        render_right(app, frame, right, false);
    }

    frame.render_widget(
        Paragraph::new(app.status.as_str())
            .style(status_style(th, app.status_kind))
            .wrap(Wrap { trim: false }),
        status,
    );
    frame.render_widget(Paragraph::new(key_bar(app, keys.width)), keys);

    match &app.overlay {
        Overlay::Confirm { title, lines, .. } => render_dialog(
            frame,
            dialog_area,
            title,
            lines,
            "y 実行する   それ以外のキー 取りやめる",
            Tone::Warn,
            th,
        ),
        Overlay::QuitDialog => render_dialog(
            frame,
            dialog_area,
            "未保存の変更がある",
            &[format!(
                "{} 件の項目が保存されていない。",
                app.changed_count()
            )],
            "s 保存して終了   y 破棄して終了   Esc 戻る   Ctrl+C 強制終了",
            Tone::Warn,
            th,
        ),
        Overlay::Help => render_dialog(
            frame,
            dialog_area,
            "ヘルプ",
            &help_lines(),
            "何かキーを押すと閉じる",
            Tone::Info,
            th,
        ),
        Overlay::RunConfirm { report } => render_dialog(
            frame,
            dialog_area,
            "実行の確認",
            &confirm_lines(report),
            &format!(
                "y 実行 ({} 秒後)   Esc など 取りやめる",
                RUN_COUNTDOWN_MS / 1000
            ),
            if report.unrestricted {
                Tone::Danger
            } else {
                Tone::Warn
            },
            th,
        ),
        Overlay::Countdown {
            report,
            remaining_ms,
            ..
        } => render_dialog(
            frame,
            dialog_area,
            &format!("実行まで {} 秒", remaining_ms.div_ceil(1000)),
            &countdown_lines(report, *remaining_ms),
            "Esc / q 取りやめ",
            Tone::Danger,
            th,
        ),
        Overlay::Running { report } => render_dialog(
            frame,
            dialog_area,
            "実行中…",
            &running_lines(report),
            "止めるにはマウスを大きく動かす",
            Tone::Danger,
            th,
        ),
        Overlay::RunResult { title, lines, ok } => render_dialog(
            frame,
            dialog_area,
            title,
            lines,
            "Enter / Esc 閉じる",
            if *ok { Tone::Ok } else { Tone::Danger },
            th,
        ),
        _ => {}
    }
}

/// キーの案内の札の見え方 (D-130)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagLook {
    /// キーは accent の太字、意味は muted
    Normal,
    /// 変更が無いときの s / u。キーも意味も muted (控えめ)
    Quiet,
    /// 未保存のときの s / u。キーは warn の太字、意味は text
    Urgent,
}

/// キーの案内の札 (キー, 意味)。`keep` は幅が足りなくても落とさない
struct KeyTag {
    key: &'static str,
    what: String,
    keep: bool,
    look: TagLook,
}

fn tag(key: &'static str, what: impl Into<String>) -> KeyTag {
    KeyTag {
        key,
        what: what.into(),
        keep: false,
        look: TagLook::Normal,
    }
}

impl KeyTag {
    fn keep(mut self) -> Self {
        self.keep = true;
        self
    }

    fn look(mut self, look: TagLook) -> Self {
        self.look = look;
        self
    }

    fn width(&self) -> usize {
        UnicodeWidthStr::width(self.key) + 1 + UnicodeWidthStr::width(self.what.as_str())
    }
}

/// 札と札の区切り
const TAG_SEP: &str = "   ";

/// 場面ごとの札の並び (spec の「キーの案内」)。場面は `overlay`、`focus`、`dirty()` で決める
fn key_tags(app: &App) -> Vec<KeyTag> {
    let save_undo = |look| [tag("s", "保存").look(look), tag("u", "元に戻す").look(look)];
    match &app.overlay {
        Overlay::None => {
            let dirty = app.dirty();
            let mut v = Vec::new();
            if dirty {
                v.extend(save_undo(TagLook::Urgent));
            }
            if app.focus == Focus::Right && app.field().is_list() {
                // 右ペインの一覧: 一覧のキー。未保存のときだけ先頭に s / u
                v.extend([
                    tag("↑↓", "選ぶ"),
                    tag("Enter", "編集/追加"),
                    tag("a", "追加"),
                    tag("d", "削除"),
                ]);
                if !dirty {
                    v.push(tag("u", "元に戻す").look(TagLook::Quiet));
                }
                v.push(tag("←", "戻る").keep());
            } else {
                v.extend([tag("↑↓", "選ぶ"), tag("Enter", "開く・切替")]);
                if !dirty {
                    v.extend(save_undo(TagLook::Quiet));
                }
                v.extend([
                    tag("r", "実行"),
                    tag("F1", "ヘルプ").keep(),
                    tag("Esc", "終了").keep(),
                ]);
            }
            v
        }
        Overlay::Edit { .. } => vec![
            tag("Enter", "確定").keep(),
            tag("Esc", "取消").keep(),
            tag("←→ Home End", "移動"),
            tag("Ctrl+U", "全消去"),
        ],
        Overlay::Checklist { .. } => vec![
            tag("Space", "切替"),
            tag("a", "全部"),
            tag("Enter", "確定").keep(),
            tag("Esc", "取消").keep(),
        ],
        Overlay::Picker { kind, .. } => {
            let mut v = vec![tag("↑↓", "選ぶ"), tag("Enter", "追加").keep()];
            if *kind != ListKind::Regions {
                v.push(tag("F5", "再読み込み"));
            }
            v.push(tag("Esc", "取消").keep());
            v
        }
        Overlay::ScriptPicker { .. } => vec![
            tag("↑↓", "選ぶ"),
            tag("Enter", "検証").keep(),
            tag("F5", "再読み込み"),
            tag("Esc", "取消").keep(),
        ],
        Overlay::Confirm { .. } => vec![
            tag("y", "実行する").keep(),
            tag("それ以外のキー", "取りやめる").keep(),
        ],
        Overlay::QuitDialog => vec![
            tag("s", "保存して終了").keep(),
            tag("y", "破棄して終了").keep(),
            tag("Esc", "戻る").keep(),
            tag("Ctrl+C", "強制終了"),
        ],
        Overlay::Help => vec![tag("何かキー", "閉じる").keep()],
        Overlay::RunConfirm { .. } => vec![
            tag("y", format!("実行 ({} 秒後)", RUN_COUNTDOWN_MS / 1000)).keep(),
            tag("Esc など", "取りやめる").keep(),
        ],
        Overlay::Countdown { .. } => vec![tag("Esc / q", "取りやめ").keep()],
        Overlay::Running { .. } => vec![tag("マウスを大きく動かす", "止める").keep()],
        Overlay::RunResult { .. } => vec![tag("Enter / Esc", "閉じる").keep()],
    }
}

/// 最下行のキーの案内 (D-130)。幅が足りなければ、`keep` でない札を右から落とす
fn key_bar(app: &App, width: u16) -> Line<'static> {
    let th = app.theme();
    let mut tags = key_tags(app);
    let total = |tags: &[KeyTag]| {
        tags.iter().map(KeyTag::width).sum::<usize>() + TAG_SEP.len() * tags.len().saturating_sub(1)
    };
    while total(&tags) > usize::from(width) {
        match tags.iter().rposition(|t| !t.keep) {
            Some(i) => {
                tags.remove(i);
            }
            None => break,
        }
    }
    let mut spans = Vec::new();
    for (i, t) in tags.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(TAG_SEP));
        }
        let (key_style, what_style) = match t.look {
            TagLook::Normal => (
                Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
                Style::new().fg(th.muted),
            ),
            TagLook::Quiet => (Style::new().fg(th.muted), Style::new().fg(th.muted)),
            TagLook::Urgent => (
                Style::new().fg(th.warn).add_modifier(Modifier::BOLD),
                Style::new().fg(th.text),
            ),
        };
        spans.push(Span::styled(t.key, key_style));
        spans.push(Span::styled(format!(" {}", t.what), what_style));
    }
    Line::from(spans)
}

fn pane_block(th: &Theme, title: String, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::new().fg(th.focus)
    } else {
        Style::new().fg(th.border)
    };
    Block::bordered()
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(style)
        .title(title)
}

fn render_left(app: &App, frame: &mut Frame, area: TuiRect) {
    let th = app.theme();
    let fs = fields();
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_row = 0;
    let mut last_section = "";
    let name_w = fs
        .iter()
        .map(|f| UnicodeWidthStr::width(f.name()))
        .max()
        .unwrap_or(0);
    for (i, f) in fs.iter().enumerate() {
        if f.section() != last_section {
            last_section = f.section();
            let color = section_color(th, *f);
            items.push(ListItem::new(Line::styled(
                format!("── {} ", last_section),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            )));
        }
        if i == app.selected {
            selected_row = items.len();
        }
        let changed = field_changed(&app.config, &app.saved, *f);
        let pad = name_w.saturating_sub(UnicodeWidthStr::width(f.name()));
        let mut value = shown_value(app, *f);
        let value_style = match f {
            // 実入力とパスワード入力は安全を緩める側 (D-090)。有効は danger、無効 (安全側) は上部の帯と同じ ok
            Field::InputEnabled | Field::PasswordInput => {
                if toggle_value(&app.config, *f) {
                    value = ON_DANGER.into();
                    Style::new().fg(th.danger).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(th.ok)
                }
            }
            Field::CaptureEnabled if app.config.general.capture_enabled => Style::new().fg(th.warn),
            // 撮影と全画面の撮影の「無効」も安全側なので、上部の帯と同じ ok (D-129)
            Field::CaptureEnabled | Field::CaptureFullScreen if !toggle_value(&app.config, *f) => {
                Style::new().fg(th.ok)
            }
            _ => Style::new().fg(th.accent),
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                if changed { "● " } else { "  " },
                Style::new().fg(th.changed),
            ),
            Span::styled(format!("{}{}  ", f.name(), " ".repeat(pad)), body_style(th)),
            Span::styled(value, value_style),
        ])));
    }
    let focused = app.focus == Focus::Left && app.overlay == Overlay::None;
    let list = List::new(items)
        .block(pane_block(th, " 項目 ".into(), focused))
        .highlight_symbol("▶")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_right(app: &App, frame: &mut Frame, area: TuiRect, narrow: bool) {
    let th = app.theme();
    let f = app.field();
    let picking_script = matches!(app.overlay, Overlay::ScriptPicker { .. });
    let crumb = if picking_script {
        " 操作列を実行 ".to_string()
    } else if narrow {
        format!(" {} (← 戻る) ", f.location())
    } else {
        format!(" {} ", f.name())
    };
    let focused = app.focus == Focus::Right && app.overlay == Overlay::None;
    let block = pane_block(
        th,
        crumb,
        focused
            || !matches!(
                app.overlay,
                Overlay::None | Overlay::Help | Overlay::QuitDialog | Overlay::Confirm { .. }
            ),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut header: Vec<Line> = if picking_script {
        vec![
            Line::raw("操作列 JSON を選んで検証し、確認してから実行する。"),
            Line::raw(format!(
                "判定は保存済みの設定で行う。実行の前に {} 秒のカウントダウンがある。",
                RUN_COUNTDOWN_MS / 1000
            )),
        ]
    } else {
        let mut h = vec![Line::styled(f.key(), Style::new().fg(th.muted))];
        h.extend(f.description().into_iter().map(Line::raw));
        h
    };
    header.push(Line::raw(""));
    // 折り返した後の行数で高さを決める。下の一覧や入力欄のために、少なくとも 6 行は残す
    let w = usize::from(inner.width).max(1);
    let wrapped: usize = header.iter().map(|l| l.width().div_ceil(w).max(1)).sum();
    let header_h = u16::try_from(wrapped)
        .unwrap_or(u16::MAX)
        .min(inner.height.saturating_sub(6).max(2));
    let [head, rest] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Fill(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(header)
            .style(body_style(th))
            .wrap(Wrap { trim: false }),
        head,
    );

    // 追加や編集の最中も、今の値と効果が見えるように、上に 1 行の要約を出す
    let rest = match (&app.overlay, f) {
        (Overlay::Edit { .. } | Overlay::Picker { .. }, Field::List(side, _)) => {
            let [summary, below] =
                Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(rest);
            let color = side_color(th, side);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!("今の値: {}", short_value(&app.config, f)),
                    Style::new().fg(color),
                )),
                summary,
            );
            below
        }
        _ => rest,
    };

    match &app.overlay {
        Overlay::Edit {
            field,
            item,
            editor,
        } if *field == f => render_editor(app, frame, rest, *field, *item, editor),
        Overlay::Checklist {
            side,
            checked,
            cursor,
        } => render_checklist(th, frame, rest, *side, checked, *cursor),
        Overlay::Picker {
            side,
            kind,
            filter,
            cursor,
        } => render_picker(app, frame, rest, *side, *kind, filter, *cursor),
        Overlay::ScriptPicker { filter, cursor } => {
            render_script_picker(app, frame, rest, filter, *cursor)
        }
        _ => match f {
            Field::List(side, kind) => render_list_values(app, frame, rest, side, kind),
            _ => {
                let mut lines = vec![Line::from(vec![
                    Span::raw("現在の値: "),
                    Span::styled(shown_value(app, f), Style::new().fg(th.accent)),
                ])];
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    match f {
                        Field::InputEnabled
                        | Field::CaptureEnabled
                        | Field::PasswordInput
                        | Field::CaptureFullScreen => {
                            "左の一覧で Enter: 切り替え (有効にするときは確認する)"
                        }
                        Field::IndicatorEnabled | Field::IndicatorSound => {
                            "左の一覧で Enter: 切り替え"
                        }
                        Field::Theme => {
                            "左の一覧で Enter: 切り替え (auto → calm → calm-light → ansi → mono)"
                        }
                        _ => "左の一覧で Enter: 編集する",
                    },
                    Style::new().fg(th.muted),
                ));
                frame.render_widget(
                    Paragraph::new(lines)
                        .style(body_style(th))
                        .wrap(Wrap { trim: false }),
                    rest,
                );
            }
        },
    }
}

fn render_list_values(app: &App, frame: &mut Frame, area: TuiRect, side: Side, kind: ListKind) {
    let th = app.theme();
    let values = list_ref(&app.config, side, kind);
    let other = match side {
        Side::Allow => list_ref(&app.config, Side::Deny, kind),
        Side::Deny => list_ref(&app.config, Side::Allow, kind),
    };
    let color = side_color(th, side);
    let effect = match (side, values.len()) {
        (Side::Allow, 0) => "効果: 制限なし (この次元はすべて許可)".to_string(),
        (Side::Allow, n) => format!("効果: この {n} 件だけを許可。それ以外は拒否"),
        (Side::Deny, 0) => "効果: なし".to_string(),
        (Side::Deny, n) => format!("効果: この {n} 件を常に拒否"),
    };
    let mut items: Vec<ListItem> = values
        .iter()
        .map(|v| {
            let overlap = other
                .iter()
                .any(|o| kind.compare_key(o) == kind.compare_key(v));
            let mut spans = vec![Span::styled(v.clone(), Style::new().fg(color))];
            if overlap && side == Side::Allow {
                spans.push(Span::styled(
                    "  (拒否リストにもある: 拒否が優先)",
                    Style::new().fg(th.warn),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let add_label = if kind == ListKind::Actions {
        "✎ チェックで選び直す…"
    } else {
        "＋ 追加…"
    };
    items.push(ListItem::new(Line::styled(
        add_label,
        Style::new().fg(th.accent),
    )));
    let [eff, list_area, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::styled(effect, Style::new().fg(color))),
        eff,
    );
    let focused = app.focus == Focus::Right;
    let list = List::new(items)
        .highlight_symbol(if focused { "▶" } else { " " })
        .highlight_style(if focused {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        });
    let mut state = ListState::default().with_selected(Some(app.row.min(values.len())));
    frame.render_stateful_widget(list, list_area, &mut state);
    // 一覧に入っているときのキーは、最下行のキーの案内に出す (D-130)
    if !focused {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Enter か → で、この一覧を編集する",
                Style::new().fg(th.muted),
            )),
            help,
        );
    }
}

fn render_editor(
    app: &App,
    frame: &mut Frame,
    area: TuiRect,
    field: Field,
    item: Option<usize>,
    editor: &LineEditor,
) {
    let th = app.theme();
    let title = match (field, item) {
        (Field::List(..), Some(i)) => format!(" 値を編集 ({}) ", i + 1),
        (Field::List(..), None) => " 新しい値を追加 ".to_string(),
        _ => " 新しい値 ".to_string(),
    };
    let checked = match field {
        Field::List(side, kind) => check_list_input(side, kind, &editor.text()),
        _ => check_scalar_input(field, &editor.text()),
    };
    let rows = [Constraint::Length(3), Constraint::Length(4)];
    let [input, feedback] = Layout::vertical(rows).areas(area);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(th.focus))
        .title(title);
    let inner = block.inner(input);
    let (shown, cursor) = editor.view(inner.width as usize);
    let text_style = if editor.fresh {
        body_style(th).add_modifier(Modifier::REVERSED)
    } else {
        body_style(th)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(shown, text_style)).block(block),
        input,
    );
    frame.set_cursor_position((inner.x + cursor as u16, inner.y));
    let mut lines = vec![match &checked.value {
        Ok(_) if editor.text().trim().is_empty() && matches!(field, Field::SentinelFile) => {
            Line::styled("✓ 空 (使わない)", Style::new().fg(th.ok))
        }
        Ok(v) if v.is_empty() && matches!(field, Field::CaptureSaveDirs) => {
            Line::styled("✓ 空 (制限なし)", Style::new().fg(th.ok))
        }
        Ok(v) if v.is_empty() && matches!(field, Field::McpRequireApproval) => {
            Line::styled("✓ 空 (なし)", Style::new().fg(th.ok))
        }
        Ok(v) => Line::styled(format!("✓ {v}"), Style::new().fg(th.ok)),
        Err(e) => Line::styled(format!("✗ {e}"), Style::new().fg(th.danger)),
    }];
    if let Some(w) = &checked.warning {
        lines.push(Line::styled(format!("⚠ {w}"), Style::new().fg(th.warn)));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), feedback);
}

fn render_checklist(
    th: &Theme,
    frame: &mut Frame,
    area: TuiRect,
    side: Side,
    checked: &[bool],
    cursor: usize,
) {
    let kinds = checklist_kinds();
    let items: Vec<ListItem> = kinds
        .iter()
        .zip(checked)
        .map(|(k, c)| ListItem::new(format!("{} {}", if *c { "[x]" } else { "[ ]" }, k.name())))
        .collect();
    let n = checked.iter().filter(|c| **c).count();
    let note = match (side, n) {
        (Side::Allow, 0) => "選ばないと「制限なし」(すべての操作を許可)".to_string(),
        (Side::Allow, n) => format!("選んだ {n} 種だけを許可し、他は拒否"),
        (Side::Deny, 0) => "拒否する操作なし".to_string(),
        (Side::Deny, n) => format!("選んだ {n} 種を常に拒否"),
    };
    let [list_area, note_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(th.focus))
        .title(" 操作を選ぶ ");
    let list = List::new(items)
        .block(block)
        .style(body_style(th))
        .highlight_symbol("▶")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
    frame.render_widget(
        Paragraph::new(Line::styled(note, Style::new().fg(th.warn))),
        note_area,
    );
}

fn render_picker(
    app: &App,
    frame: &mut Frame,
    area: TuiRect,
    side: Side,
    kind: ListKind,
    filter: &LineEditor,
    cursor: usize,
) {
    let th = app.theme();
    let rows = app.picker_rows(side, kind, &filter.text());
    let [input, verdict, list_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let title = match kind {
        ListKind::Regions => " 領域を入力するか、下から選ぶ ".to_string(),
        _ => format!(
            " {}を入力するか、開いているウィンドウから選ぶ ",
            kind.name()
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(th.focus))
        .title(title);
    let inner = block.inner(input);
    let (shown, c) = filter.view(inner.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(shown, body_style(th))).block(block),
        input,
    );
    if cursor == 0 {
        frame.set_cursor_position((inner.x + c as u16, inner.y));
    }
    let checked = check_list_input(side, kind, &filter.text());
    let v = if filter.text().trim().is_empty() {
        Line::styled(
            "文字を打つと絞り込む。そのまま Enter で、打った値を追加",
            Style::new().fg(th.muted),
        )
    } else {
        match (&checked.value, &checked.warning) {
            (Ok(v), Some(w)) => Line::styled(format!("✓ {v}  ⚠ {w}"), Style::new().fg(th.warn)),
            (Ok(v), None) => Line::styled(format!("✓ {v}"), Style::new().fg(th.ok)),
            (Err(e), _) => Line::styled(format!("✗ {e}"), Style::new().fg(th.danger)),
        }
    };
    frame.render_widget(Paragraph::new(v), verdict);
    let mut items = vec![ListItem::new(Line::styled(
        format!("✎ 入力した値を追加: {}", filter.text()),
        Style::new().fg(th.accent),
    ))];
    if kind != ListKind::Regions && app.windows.is_none() {
        items.push(ListItem::new(Line::styled(
            "(開いているウィンドウを読み込み中…)",
            Style::new().fg(th.muted),
        )));
    }
    for r in &rows {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(r.value.clone(), body_style(th)),
            Span::styled(format!("  {}", r.detail), Style::new().fg(th.muted)),
        ])));
    }
    let list = List::new(items)
        .highlight_symbol("▶")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn render_script_picker(
    app: &App,
    frame: &mut Frame,
    area: TuiRect,
    filter: &LineEditor,
    cursor: usize,
) {
    let th = app.theme();
    let rows = app.script_rows(filter);
    let [input, note, list_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(th.focus))
        .title(" 操作列 JSON のパスを入力するか、下から選ぶ ");
    let inner = block.inner(input);
    let (shown, c) = filter.view(inner.width as usize);
    let text_style = if filter.fresh {
        body_style(th).add_modifier(Modifier::REVERSED)
    } else {
        body_style(th)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(shown, text_style)).block(block),
        input,
    );
    if cursor == 0 {
        frame.set_cursor_position((inner.x + c as u16, inner.y));
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            "文字を打つと絞り込む。Enter で検証 (装置には触れない)",
            Style::new().fg(th.muted),
        )),
        note,
    );
    let mut items = vec![ListItem::new(Line::styled(
        format!("✎ 入力したパスを使う: {}", filter.text()),
        Style::new().fg(th.accent),
    ))];
    match &app.scripts {
        None => items.push(ListItem::new(Line::styled(
            "(作業ディレクトリの *.json を読み込み中…)",
            Style::new().fg(th.muted),
        ))),
        Some(all) if all.is_empty() => items.push(ListItem::new(Line::styled(
            "(作業ディレクトリに *.json が無い)",
            Style::new().fg(th.muted),
        ))),
        Some(_) => {}
    }
    for r in &rows {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(r.name.clone(), body_style(th)),
            Span::styled(format!("  {}", r.path.display()), Style::new().fg(th.muted)),
        ])));
    }
    let list = List::new(items)
        .highlight_symbol("▶")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn render_dialog(
    frame: &mut Frame,
    area: TuiRect,
    title: &str,
    lines: &[String],
    keys: &str,
    tone: Tone,
    th: &Theme,
) {
    let color = tone.color(th);
    let content_w = lines
        .iter()
        .map(|l| UnicodeWidthStr::width(l.as_str()))
        .chain([UnicodeWidthStr::width(keys)])
        .max()
        .unwrap_or(20);
    let w = u16::try_from(content_w)
        .unwrap_or(u16::MAX)
        .saturating_add(4)
        .min(area.width)
        .max(20.min(area.width));
    // 長い行は自分で文字単位に折る (ratatui の単語単位の折り返しと高さの見積もりがずれないように)。
    // 収まらなければ本文を省略し、キーの案内は必ず出す
    let inner_w = usize::from(w.saturating_sub(2)).max(1);
    let mut wrapped: Vec<String> = lines.iter().flat_map(|l| wrap_chars(l, inner_w)).collect();
    let keys_rows = wrap_chars(keys, inner_w);
    let budget = usize::from(area.height.saturating_sub(2))
        .saturating_sub(keys_rows.len() + 1)
        .max(1);
    if wrapped.len() > budget {
        wrapped.truncate(budget.saturating_sub(1));
        wrapped.push("…(省略)".into());
    }
    let rows = wrapped.len() + 1 + keys_rows.len();
    let h = u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height);
    let r = TuiRect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    clear_wide_left_of(frame.buffer_mut(), r);
    frame.render_widget(Clear, r);
    let mut text: Vec<Line> = wrapped
        .into_iter()
        .map(|l| Line::styled(l, body_style(th)))
        .collect();
    text.push(Line::raw(""));
    text.extend(
        keys_rows
            .into_iter()
            .map(|k| Line::styled(k, Style::new().fg(color).add_modifier(Modifier::BOLD))),
    );
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(color))
        .title(format!(" {title} "));
    frame.render_widget(Paragraph::new(text).block(block), r);
}

/// ダイアログの左隣のセルが全角文字の左半分なら消す (D-046)。ratatui の差分描画は、幅 2 の文字のセルの次を
/// 「隠れている」として飛ばすので、そこに枠の左端 (`╔` `║` `╚`) を置いても端末に送られない (docs/design-notes.md)。
fn clear_wide_left_of(buf: &mut Buffer, r: TuiRect) {
    let Some(x) = r.x.checked_sub(1) else {
        return;
    };
    for y in r.top()..r.bottom() {
        if let Some(cell) = buf.cell_mut((x, y))
            && cell.cell_width() > 1
        {
            cell.reset();
        }
    }
}

/// 表示幅 `width` に収まるよう、文字単位で折る (全角は 2 桁)。空文字は 1 行。
fn wrap_chars(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(c);
        used += w;
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

fn help_lines() -> Vec<String> {
    [
        "項目の一覧: ↑↓ 選ぶ / Enter・→ 開く (総スイッチは切り替え)",
        "一覧の編集: ↑↓ 選ぶ / Enter 編集・追加 / a 追加 / d 削除 / ← 戻る",
        "どこでも: s・Ctrl+S 保存 / u・Ctrl+Z 元に戻す / ?・F1 ヘルプ / q 終了 (主画面では Esc も) / Ctrl+C 終了",
        "操作列の実行: r で JSON を選ぶ → 検証 → y → 5 秒後に実行 (保存済みの設定で判定。未保存なら先に s)",
        "実行中はマウスを大きく動かすと止まる。自分の端末のウィンドウには送らない",
        "",
        "判定の規則: 組み込みの拒否 > 拒否リスト > 許可リスト",
        "許可リストが空 = その次元は制限なし。1 件でもあれば一致したものだけ",
        "組み込みの拒否: Win を含む組み合わせ、Alt+F4、Ctrl+Alt+、Ctrl+Shift+Esc、Ctrl+Esc",
        "--config と AI_CU_CONFIG の層は、基本の層 (既定の場所) より許可を広げられない",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// 端末での実行 (テストしない薄い層)
// ---------------------------------------------------------------------------

/// 端末で TUI を動かす。呼び出し側は、標準入力と標準出力の両方がコンソールであることを確かめておく (D-023)。
/// 操作列の実行は `runner` に頼む (CLI の `run` と同じ経路。D-044)。
pub fn run(target: &Target, runner: &mut dyn ScriptRunner) -> i32 {
    let mut app = App::load(target.clone());
    // 配色の環境は 1 回だけ読む。RGB のテーマを選んだのに VT が無ければ、ansi で描くことを知らせる (D-129)
    app.env = ThemeEnv::detect();
    if theme::fell_back(app.config.tui.theme, app.env) {
        // 読み込みの知らせ (読めないファイルの復旧の導線など) は消さずに、後ろへ足す
        let text = if app.status.is_empty() {
            FELL_BACK_MSG.to_string()
        } else {
            format!("{} / {FELL_BACK_MSG}", app.status)
        };
        app.set_status(StatusKind::Warn, text);
    }
    // NO_COLOR は resolve が mono を選ぶことで扱う。crossterm にも効かせると、明示したテーマの色まで消える
    ratatui::crossterm::style::force_color_output(true);
    // 領域の雛形 (読み取りだけ)
    if let Ok(r) = crate::win32::screen::primary_screen() {
        app.screens
            .push((format!("主画面の全体 ({}x{})", r.w, r.h), r));
    }
    if let Ok(r) = crate::win32::screen::virtual_screen() {
        app.screens
            .push((format!("仮想デスクトップの全体 ({}x{})", r.w, r.h), r));
    }
    let mut terminal = match ratatui::try_init() {
        Ok(t) => t,
        Err(e) => {
            ratatui::restore();
            eprintln!("端末を TUI に切り替えられない: {e}");
            eprintln!("Windows Terminal か PowerShell のウィンドウで、引数なしで実行する。");
            eprintln!("Usage: ai-cu [--config=PATH] [COMMAND]  (ai-cu --help)");
            return 2;
        }
    };
    let code = 'outer: loop {
        if let Err(e) = terminal.draw(|f| render(&app, f)) {
            break report_io(e);
        }
        let started = std::time::Instant::now();
        // カウントダウンの間だけ、キーを待ちながら時間を進める
        let ev = if matches!(app.overlay, Overlay::Countdown { .. }) {
            match event::poll(Duration::from_millis(TICK_MS)) {
                Ok(true) => match event::read() {
                    Ok(ev) => Some(ev),
                    Err(e) => break report_io(e),
                },
                Ok(false) => None,
                Err(e) => break report_io(e),
            }
        } else {
            match event::read() {
                Ok(ev) => Some(ev),
                Err(e) => break report_io(e),
            }
        };
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        for effect in app.step(ev.as_ref(), elapsed) {
            if let Effect::RunScript(req) = &effect {
                // 「実行中」を 1 回描いてから、代替画面を保ったまま同期で実行する (D-043)
                if let Err(e) = terminal.draw(|f| render(&app, f)) {
                    break 'outer report_io(e);
                }
                apply_run(&mut app, req, runner);
                // 実行中に端末へ届いた打鍵を捨てる (自分の端末への注入を含む。D-045)
                if let Err(e) = drain_input() {
                    break 'outer report_io(e);
                }
            } else if let Some(code) = apply_effect(&mut app, &effect, runner) {
                break 'outer code;
            }
        }
    };
    ratatui::restore();
    code
}

/// 実行の要求を runner に渡し、結果を App に戻す。
fn apply_run(app: &mut App, req: &RunRequest, runner: &mut dyn ScriptRunner) {
    let r = runner.run(&req.report.script, Some(&req.self_window));
    app.on_run_finished(r);
}

/// 端末に触れない Effect を実行し、結果を App に戻す。終了するなら終了コードを返す。
/// `RunScript` は呼び出し側が描画と読み捨てを挟むので、ここでは `apply_run` に委ねる。
fn apply_effect(app: &mut App, effect: &Effect, runner: &mut dyn ScriptRunner) -> Option<i32> {
    match effect {
        Effect::None => {}
        Effect::Quit => return Some(0),
        Effect::Save | Effect::SaveAndQuit => {
            let r = config::save_with(&app.target.path, &app.config, app.replace_broken)
                .map_err(|e| e.message);
            let ok = r.is_ok();
            app.on_saved(r);
            // 保存して終了: 失敗したら開いたままにして、エラーを見せる
            if ok && *effect == Effect::SaveAndQuit {
                return Some(0);
            }
        }
        Effect::LoadWindows => {
            // 読み取りだけの列挙 (入力も撮影もしない)
            let list = crate::win32::window::top_level_windows()
                .into_iter()
                .map(|w| WindowCandidate {
                    process: w.process,
                    class: w.class,
                    title: w.title,
                })
                .collect();
            app.set_windows(list);
        }
        Effect::LoadScripts => {
            // 読み取りだけの列挙 (作業ディレクトリ直下)
            let list = std::env::current_dir()
                .map_err(|e| e.to_string())
                .and_then(|d| list_scripts(&d));
            app.set_scripts(list);
        }
        Effect::Preflight(path) => {
            let r = runner.preflight(path);
            app.on_preflight(r);
        }
        Effect::ArmRun => {
            // 武装の瞬間の前面 (この端末)。読み取りだけ
            let w = runner.foreground();
            app.set_self_window(w);
        }
        Effect::RunScript(req) => apply_run(app, req, runner),
    }
    None
}

/// 短い猶予の間に何も届かなくなるまで、端末の入力を読み捨てる。
fn drain_input() -> std::io::Result<()> {
    while event::poll(Duration::from_millis(DRAIN_GRACE_MS))? {
        let _ = event::read()?;
    }
    Ok(())
}

fn report_io(e: std::io::Error) -> i32 {
    ratatui::restore();
    eprintln!("端末の入出力に失敗: {e}");
    1
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEventState;

    use super::*;
    use crate::device::WindowInfo;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn release(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new_with_kind_and_state(
            code,
            KeyModifiers::NONE,
            KeyEventKind::Release,
            KeyEventState::NONE,
        ))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn target(overlay: bool) -> Target {
        Target {
            path: PathBuf::from("target/test-tmp/tui.toml"),
            overlay,
        }
    }

    fn app() -> App {
        App::new(target(false), Config::default())
    }

    fn select(a: &mut App, f: Field) {
        a.selected = fields().iter().position(|x| *x == f).unwrap();
    }

    fn type_str(a: &mut App, s: &str) {
        for c in s.chars() {
            a.update(&press(KeyCode::Char(c)));
        }
    }

    fn buffer_text(t: &Terminal<TestBackend>) -> String {
        let buf = t.backend().buffer();
        let w = buf.area.width as usize;
        buf.content
            .chunks(w)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 全角文字の後ろのセルは空白として読めるので、空白を除いて照合する
    fn has(text: &str, needle: &str) -> bool {
        let strip = |s: &str| s.chars().filter(|c| *c != ' ').collect::<String>();
        strip(text).contains(&strip(needle))
    }

    fn draw(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(app, f)).unwrap();
        buffer_text(&t)
    }

    fn draw_term(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(app, f)).unwrap();
        t
    }

    /// セル (x, y) の文字の色
    fn cell_fg(t: &Terminal<TestBackend>, x: u16, y: u16) -> Color {
        t.backend().buffer()[(x, y)].fg
    }

    /// `needle` (空白は無視して照合) が始まるセルの座標を、上の行から順にすべて返す
    fn find_cells(t: &Terminal<TestBackend>, needle: &str) -> Vec<(u16, u16)> {
        let strip = |s: &str| s.chars().filter(|c| *c != ' ').collect::<String>();
        let want = strip(needle);
        let buf = t.backend().buffer();
        let mut out = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf[(x, y)].symbol().trim().is_empty() {
                    continue;
                }
                let rest: String = (x..buf.area.width).map(|i| buf[(i, y)].symbol()).collect();
                if strip(&rest).starts_with(&want) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// `needle` が始まる最初のセルの座標
    fn find_cell(t: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
        *find_cells(t, needle)
            .first()
            .unwrap_or_else(|| panic!("{needle} が画面に無い:\n{}", buffer_text(t)))
    }

    /// 左ペイン (幅 120 の画面で 54 桁まで) の中で `needle` が始まる最初のセル。右ペインの枠の題と区別する
    fn find_left_cell(t: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
        *find_cells(t, needle)
            .iter()
            .find(|(x, _)| *x < 54)
            .unwrap_or_else(|| panic!("{needle} が左ペインに無い:\n{}", buffer_text(t)))
    }

    #[test]
    fn allow_heading_uses_accent_not_green() {
        let t = draw_term(&app(), 120, 40);
        let (x, y) = find_cell(&t, "許可リスト (allow)");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.accent);
        let (x, y) = find_cell(&t, "拒否リスト (deny)");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.danger);
    }

    #[test]
    fn unsaved_count_uses_warn_and_mark_uses_changed() {
        let mut a = app();
        a.config.general.capture_enabled = !a.config.general.capture_enabled;
        let t = draw_term(&a, 120, 40);
        let (x, y) = find_cell(&t, "未保存");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.warn);
        let (x, y) = find_cell(&t, "●");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.changed);
    }

    /// 本文 (項目の名前、説明、帯、ダイアログ、入力欄の文字) は、端末の既定の前景ではなく `text` の役割で塗る
    #[test]
    fn body_text_uses_text_role() {
        let mut a = app();
        let t = draw_term(&a, 120, 40);
        // 左ペインの項目の名前 (帯の「実入力:」とは別。空白を除くと「実入力(マウスとキー)」)
        let (x, y) = find_left_cell(&t, "実入力 (マウスとキー)");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.text);
        // 上部の帯の本文
        let (x, y) = find_cell(&t, "撮影:");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.text);
        // 右ペインの説明
        let (x, y) = find_cell(&t, "有効にすると、ai-cu が本物の");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.text);
        // ダイアログの本文
        a.overlay = Overlay::Help;
        let t = draw_term(&a, 120, 40);
        let (x, y) = find_cell(&t, "項目の一覧:");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.text);
        // 入力欄に打った文字
        a.overlay = Overlay::None;
        select(&mut a, Field::MaxActions);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "7777");
        let t = draw_term(&a, 120, 40);
        let (x, y) = find_cell(&t, "7777");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.text);
    }

    /// 実入力とパスワード入力の「無効」(安全側) は、上部の帯と同じ `ok` で塗る
    #[test]
    fn disabled_master_switches_use_ok_in_left_pane() {
        let t = draw_term(&app(), 120, 40);
        for name in [
            "実入力 (マウスとキー)",
            "パスワード欄への入力",
            "画面撮影",
            "全画面の撮影",
        ] {
            let (_, y) = find_left_cell(&t, name);
            // 左ペイン (幅 54) の同じ行にある「無効」
            let cells: Vec<_> = find_cells(&t, "無効")
                .into_iter()
                .filter(|&(x, cy)| cy == y && x < 54)
                .collect();
            assert_eq!(cells.len(), 1, "{name}\n{}", buffer_text(&t));
            assert_eq!(cell_fg(&t, cells[0].0, y), theme::CALM.ok, "{name}");
        }
    }

    #[test]
    fn input_enabled_shows_warning_mark() {
        let mut a = app();
        a.config.general.input_enabled = true;
        a.config.general.capture_enabled = true;
        let t = draw_term(&a, 120, 40);
        let text = buffer_text(&t);
        assert!(has(&text, "⚠有効"), "{text}");
        // 上部の帯と左ペインの実入力の 2 か所。撮影の有効には ⚠ を付けない
        let cells = find_cells(&t, "⚠ 有効");
        assert_eq!(cells.len(), 2, "{text}");
        for (x, y) in cells {
            assert_eq!(cell_fg(&t, x, y), theme::CALM.danger);
        }
        assert!(has(&text, "撮影:有効"), "{text}");
        assert!(!has(&text, "撮影:⚠"), "{text}");
    }

    #[test]
    fn mono_theme_draws_without_color() {
        let mut a = app();
        a.config.tui.theme = config::ThemeName::Mono;
        a.config.general.input_enabled = true;
        for overlay in [Overlay::None, Overlay::Help] {
            a.overlay = overlay;
            let t = draw_term(&a, 120, 40);
            let buf = t.backend().buffer();
            for c in &buf.content {
                assert_eq!(c.fg, Color::Reset, "{:?}", c.symbol());
            }
        }
        // 選んでいる行の REVERSED は残る
        a.overlay = Overlay::None;
        let t = draw_term(&a, 120, 40);
        assert!(
            t.backend()
                .buffer()
                .content
                .iter()
                .any(|c| c.modifier.contains(Modifier::REVERSED))
        );
    }

    #[test]
    fn ansi_theme_used_without_vt() {
        let mut a = app();
        a.env = ThemeEnv {
            no_color: false,
            vt: false,
        };
        assert_eq!(a.theme().name, config::ThemeName::Ansi);
        a.env = ThemeEnv::default();
        assert_eq!(a.theme().name, config::ThemeName::Calm);
    }

    /// 項目「テーマ」(D-129): 左の一覧の最後に並び、Enter / Space で巡回し、確認なしでその場で配色に反映する
    #[test]
    fn theme_field_cycles_and_applies_immediately() {
        assert_eq!(Field::Theme.key(), "tui.theme");
        assert_eq!(Field::Theme.name(), "テーマ");
        assert_eq!(Field::Theme.location(), "表示 > テーマ");
        assert_eq!(fields().last(), Some(&Field::Theme));
        let mut a = app();
        select(&mut a, Field::Theme);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None, "確認は挟まない");
        assert_eq!(a.config.tui.theme, config::ThemeName::Calm);
        assert_eq!(a.theme().name, config::ThemeName::Calm);
        assert_eq!(a.changed_count(), 1);
        let want = [
            config::ThemeName::CalmLight,
            config::ThemeName::Ansi,
            config::ThemeName::Mono,
            config::ThemeName::Auto,
        ];
        for (i, name) in want.into_iter().enumerate() {
            // Enter と Space のどちらでも次へ進む
            let key = if i % 2 == 0 {
                KeyCode::Char(' ')
            } else {
                KeyCode::Enter
            };
            a.update(&press(key));
            assert_eq!(a.config.tui.theme, name);
            assert_eq!(a.theme().name, theme::resolve(name, a.env).name);
        }
        assert_eq!(a.changed_count(), 0, "auto に戻れば保存済みと同じ");
        assert!(!a.dirty());
    }

    /// `auto` の行には、いま選ばれているテーマを添える。他は名前だけ。VT が無くて落ちたときは落ち先を添える
    #[test]
    fn theme_field_shows_resolved_theme_for_auto() {
        let mut a = app();
        let text = draw(&a, 120, 40);
        assert!(has(&text, "テーマ"), "{text}");
        assert!(has(&text, "(auto:calm)"), "{text}");
        a.env = ThemeEnv {
            no_color: true,
            vt: true,
        };
        assert!(has(&draw(&a, 120, 40), "(auto:mono)"));
        a.env = ThemeEnv::default();
        a.config.tui.theme = config::ThemeName::CalmLight;
        let text = draw(&a, 120, 40);
        assert!(has(&text, "テーマ calm-light"), "{text}");
        assert!(!has(&text, "(auto:"), "{text}");
        a.env = ThemeEnv {
            no_color: false,
            vt: false,
        };
        let text = draw(&a, 120, 40);
        assert!(has(&text, "calm-light→ansi"), "{text}");
        // 右ペインには説明とキーが出る
        select(&mut a, Field::Theme);
        let text = draw(&a, 120, 40);
        assert!(has(&text, "tui.theme"), "{text}");
        assert!(has(&text, "NO_COLOR"), "{text}");
        assert!(has(&text, "左の一覧で Enter: 切り替え"), "{text}");
    }

    #[test]
    fn theme_change_is_undoable_and_not_widening() {
        let mut a = app();
        select(&mut a, Field::Theme);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.tui.theme, config::ThemeName::Calm);
        assert!(
            widening_changes(&a.saved, &a.config).is_empty(),
            "配色は許可を広げない"
        );
        a.update(&press(KeyCode::Char('u')));
        assert_eq!(a.config.tui.theme, config::ThemeName::Auto);
        assert_eq!(a.changed_count(), 0);
        // 配色だけを変えて保存しても、確認を挟まずにそのまま保存する
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.update(&press(KeyCode::Char('s'))), Effect::Save);
        assert_eq!(a.overlay, Overlay::None, "確認を挟まない");
    }

    /// VT の無い端末で calm / calm-light に切り替えたら、起動時と同じく ansi で描くことを Warn で知らせる (D-129)
    #[test]
    fn theme_cycle_warns_when_falling_back_without_vt() {
        let mut a = app();
        a.env = ThemeEnv {
            no_color: false,
            vt: false,
        };
        select(&mut a, Field::Theme);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.tui.theme, config::ThemeName::Calm);
        assert_eq!(a.status_kind, StatusKind::Warn);
        assert!(
            a.status
                .contains("VT が使えないので ansi の配色で表示している")
        );
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.tui.theme, config::ThemeName::CalmLight);
        assert_eq!(a.status_kind, StatusKind::Warn);
        // ansi は落ちないので Ok
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.tui.theme, config::ThemeName::Ansi);
        assert_eq!(a.status_kind, StatusKind::Ok);
    }

    // -----------------------------------------------------------------------
    // キーの案内の行と慣れたキー (D-130)
    // -----------------------------------------------------------------------

    /// 画面の行 `y` の文字列
    fn row(t: &Terminal<TestBackend>, y: u16) -> String {
        buffer_text(t)
            .lines()
            .nth(usize::from(y))
            .unwrap_or("")
            .to_string()
    }

    /// 行 `y` で `needle` が始まるセル
    fn find_in_row(t: &Terminal<TestBackend>, y: u16, needle: &str) -> (u16, u16) {
        *find_cells(t, needle)
            .iter()
            .find(|(_, cy)| *cy == y)
            .unwrap_or_else(|| panic!("{needle} が行 {y} に無い:\n{}", buffer_text(t)))
    }

    fn strip(s: &str) -> String {
        s.chars().filter(|c| *c != ' ').collect()
    }

    fn dirty_app() -> App {
        let mut a = app();
        select(&mut a, Field::CaptureEnabled);
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char('y')));
        assert!(a.dirty());
        a
    }

    #[test]
    fn key_bar_clean_main() {
        let t = draw_term(&app(), 120, 40);
        let bar = row(&t, 39);
        for needle in [
            "s 保存",
            "u 元に戻す",
            "Esc 終了",
            "F1 ヘルプ",
            "↑↓ 選ぶ",
            "r 実行",
        ] {
            assert!(has(&bar, needle), "{needle}\n{bar}");
        }
        assert!(strip(&bar).starts_with("↑↓"), "{bar}");
        let (x, y) = find_in_row(&t, 39, "s 保存");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.muted);
        let (x, y) = find_in_row(&t, 39, "F1 ヘルプ");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.accent);
        assert!(
            t.backend().buffer()[(x, y)]
                .modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(cell_fg(&t, x + 3, y), theme::CALM.muted, "意味は muted");
        // 変更が無ければ題名に「s で保存」は出ない
        assert!(!has(&row(&t, 0), "s で保存"));
    }

    #[test]
    fn key_bar_dirty_puts_save_first_in_warn() {
        let t = draw_term(&dirty_app(), 120, 40);
        let bar = row(&t, 39);
        assert!(strip(&bar).starts_with("s保存u元に戻す"), "{bar}");
        let (x, y) = find_in_row(&t, 39, "s 保存");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.warn);
        assert_eq!(cell_fg(&t, x + 2, y), theme::CALM.text, "意味は text");
        let title = row(&t, 0);
        assert!(has(&title, "s で保存"), "{title}");
        let (x, y) = find_in_row(&t, 0, "s で保存");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.warn);
    }

    #[test]
    fn status_message_does_not_hide_key_bar() {
        let mut a = dirty_app();
        a.update(&press(KeyCode::Char('u')));
        assert_eq!(a.status, "直前の変更を元に戻した");
        let t = draw_term(&a, 120, 40);
        assert!(has(&row(&t, 39), "F1 ヘルプ"), "{}", buffer_text(&t));
        assert!(
            has(&row(&t, 38), "直前の変更を元に戻した"),
            "{}",
            buffer_text(&t)
        );
        // 次のキーでメッセージは消え、案内は残る
        a.update(&press(KeyCode::Down));
        assert_eq!(a.status, "");
        let t = draw_term(&a, 120, 40);
        assert!(row(&t, 38).trim().is_empty(), "{}", buffer_text(&t));
        assert!(has(&row(&t, 39), "F1 ヘルプ"));
    }

    #[test]
    fn key_bar_in_dialog_shows_only_dialog_keys() {
        let mut a = dirty_app();
        a.update(&press(KeyCode::Char('q')));
        assert_eq!(a.overlay, Overlay::QuitDialog);
        let t = draw_term(&a, 120, 40);
        let bar = row(&t, 39);
        assert!(!bar.contains("F1"), "{bar}");
        assert!(has(&bar, "Esc 戻る"), "{bar}");
        assert!(has(&bar, "y 破棄して終了"), "{bar}");
    }

    #[test]
    fn key_bar_in_right_list_keeps_save_when_dirty() {
        let mut a = app();
        select(&mut a, Field::List(Side::Deny, ListKind::Keys));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "ctrl+w");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.focus, Focus::Right);
        assert!(a.dirty());
        let t = draw_term(&a, 120, 40);
        let bar = strip(&row(&t, 39));
        assert!(bar.starts_with("s保存u元に戻す"), "{bar}");
        for needle in ["↑↓選ぶ", "Enter編集/追加", "a追加", "d削除", "←戻る"] {
            assert!(bar.contains(needle), "{needle}\n{bar}");
        }
        let (x, y) = find_in_row(&t, 39, "s 保存");
        assert_eq!(cell_fg(&t, x, y), theme::CALM.warn);
        // 変更が無ければ一覧のキーだけ
        let mut a = app();
        select(&mut a, Field::List(Side::Deny, ListKind::Keys));
        a.update(&press(KeyCode::Enter));
        let bar = strip(&row(&draw_term(&a, 120, 40), 39));
        assert!(bar.starts_with("↑↓選ぶ"), "{bar}");
        assert!(!bar.contains("s保存"), "{bar}");
    }

    #[test]
    fn key_bar_keeps_help_and_quit_when_narrow() {
        for a in [app(), dirty_app()] {
            let t = draw_term(&a, 60, 30);
            let bar = row(&t, 29);
            assert!(has(&bar, "F1 ヘルプ"), "{bar}");
            assert!(has(&bar, "Esc 終了"), "{bar}");
        }
        // 純関数として: 幅 30 でも 1 行で、ヘルプと終了は残る
        let line = key_bar(&app(), 30);
        let s: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.contains("F1") && s.contains("Esc"), "{s}");
    }

    #[test]
    fn ctrl_z_undoes() {
        let mut a = dirty_app();
        a.update(&ctrl('z'));
        assert!(!a.dirty());
        assert_eq!(a.status, "直前の変更を元に戻した");
    }

    #[test]
    fn ctrl_z_in_editor_does_not_undo_config() {
        let mut a = dirty_app();
        let before = a.config.clone();
        select(&mut a, Field::MaxActions);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "12");
        a.update(&ctrl('z'));
        assert!(matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text() == "12"));
        assert_eq!(a.config, before);
    }

    #[test]
    fn f1_opens_help() {
        let mut a = app();
        a.update(&press(KeyCode::F(1)));
        assert_eq!(a.overlay, Overlay::Help);
        let help = help_lines().join("\n");
        assert!(help.contains("u・Ctrl+Z 元に戻す"), "{help}");
        assert!(help.contains("?・F1 ヘルプ"), "{help}");
        assert!(help.contains("q 終了 (主画面では Esc も)"), "{help}");
    }

    #[test]
    fn esc_quits_main_when_clean() {
        let mut a = app();
        assert_eq!(a.focus, Focus::Left);
        assert_eq!(a.update(&press(KeyCode::Esc)), Effect::Quit);
    }

    #[test]
    fn esc_twice_with_unsaved_changes_keeps_changes() {
        let mut a = dirty_app();
        assert_eq!(a.update(&press(KeyCode::Esc)), Effect::None);
        assert_eq!(a.overlay, Overlay::QuitDialog);
        assert_eq!(a.update(&press(KeyCode::Esc)), Effect::None);
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.dirty());
    }

    #[test]
    fn esc_in_right_pane_returns_left_not_quit() {
        let mut a = app();
        select(&mut a, Field::List(Side::Deny, ListKind::Keys));
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.focus, Focus::Right);
        assert_eq!(a.update(&press(KeyCode::Esc)), Effect::None);
        assert_eq!(a.focus, Focus::Left);
    }

    fn windows() -> Vec<WindowCandidate> {
        let w = |p: &str, c: &str, t: &str| WindowCandidate {
            process: p.into(),
            class: c.into(),
            title: t.into(),
        };
        vec![
            w("Notepad.exe", "Notepad", "無題 - メモ帳"),
            w("notepad.exe", "Notepad", "メモ.txt - メモ帳"),
            w("CalculatorApp.exe", "ApplicationFrameWindow", "電卓"),
        ]
    }

    #[test]
    fn tui_render_testbackend_two_panes_with_sections_and_summary() {
        let text = draw(&app(), 120, 32);
        // 評価者と利用者が会話で確認できるよう、描画結果を標準出力にも出す
        println!("{text}");
        for needle in [
            "ai-cu 設定",
            "[基本の層: 既定の場所]",
            "全般 (総スイッチ)",
            "許可リスト (allow)",
            "拒否リスト (deny)",
            "実入力 (マウスとキー)",
            "general.input_enabled",
            "実入力: 無効",
            "F1 ヘルプ",
        ] {
            assert!(has(&text, needle), "{needle}\n{text}");
        }
    }

    #[test]
    fn tui_render_testbackend_add_keeps_both_panes_visible() {
        // 報告の回帰: 追加を押しても、左の一覧と右の一覧が消えない
        let mut a = app();
        select(&mut a, Field::List(Side::Allow, ListKind::Processes));
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.focus, Focus::Right);
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::LoadWindows,
            "追加の行で Enter: 候補を読み込む"
        );
        a.set_windows(windows());
        let text = draw(&a, 120, 32);
        println!("{text}");
        assert!(has(&text, "許可リスト (allow)"), "左の一覧が残る\n{text}");
        assert!(
            has(&text, "開いているウィンドウから選ぶ"),
            "右ペインで選ぶ\n{text}"
        );
        assert!(has(&text, "CalculatorApp.exe"), "{text}");
        // 絞り込んで選び、確認して追加する。追加の後も右ペインに留まる
        type_str(&mut a, "note");
        a.update(&press(KeyCode::Down));
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(a.overlay, Overlay::Confirm { .. }),
            "空の許可リストに 1 件目を足すときは確認"
        );
        a.update(&press(KeyCode::Char('y')));
        assert_eq!(a.config.allow.processes, vec!["Notepad.exe".to_string()]);
        assert_eq!(a.focus, Focus::Right);
        assert_eq!(a.overlay, Overlay::None);
        let text = draw(&a, 120, 32);
        assert!(
            has(&text, "Notepad.exe") && has(&text, "＋ 追加…"),
            "{text}"
        );
    }

    #[test]
    fn tui_actions_checklist_toggles_and_cancels() {
        let mut a = app();
        select(&mut a, Field::List(Side::Deny, ListKind::Actions));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Enter));
        assert!(matches!(a.overlay, Overlay::Checklist { .. }));
        let text = draw(&a, 120, 32);
        // wait は対象外、wait_for (D-068) は対象 (行末が "[ ] wait" の行は無く、"[ ] wait_for" の行はある)
        assert!(has(&text, "[ ] click"), "{text}");
        assert!(
            text.lines().any(|l| l.contains("[ ] wait_for")),
            "wait_for は出す\n{text}"
        );
        // 画面の待ち (D-081) も対象
        for kind in ["wait_stable", "wait_change"] {
            assert!(
                text.lines().any(|l| l.contains(&format!("[ ] {kind}"))),
                "{kind} は出す\n{text}"
            );
        }
        assert!(
            !text.lines().any(|l| l.trim_end().ends_with("[ ] wait")),
            "wait は出さない\n{text}"
        );
        a.update(&press(KeyCode::Char(' ')));
        a.update(&press(KeyCode::Down));
        a.update(&press(KeyCode::Char(' ')));
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.config.deny.actions,
            vec!["click".to_string(), "double_click".to_string()]
        );
        // Esc で取り消すと変わらない
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char(' ')));
        a.update(&press(KeyCode::Esc));
        assert_eq!(a.config.deny.actions.len(), 2);
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn tui_picker_filters_and_rejects_case_insensitive_duplicates() {
        let mut a = app();
        a.set_windows(windows());
        // 大文字小文字だけ違うプロセスは 1 つにまとめる
        assert_eq!(a.picker_rows(Side::Deny, ListKind::Processes, "").len(), 2);
        assert_eq!(
            a.picker_rows(Side::Deny, ListKind::Processes, "電卓").len(),
            1
        );
        a.config.deny.processes = vec!["notepad.exe".into()];
        assert_eq!(
            a.picker_rows(Side::Deny, ListKind::Processes, "").len(),
            1,
            "既にある値は候補に出さない"
        );
        select(&mut a, Field::List(Side::Deny, ListKind::Processes));
        a.update(&press(KeyCode::Enter));
        a.row = 1;
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "NOTEPAD");
        a.update(&press(KeyCode::Enter));
        assert!(a.status.contains("既にある"), "{}", a.status);
        assert_eq!(a.config.deny.processes.len(), 1);
    }

    #[test]
    fn tui_keys_editor_validates_live_and_warns_builtin() {
        let mut a = app();
        select(&mut a, Field::List(Side::Allow, ListKind::Keys));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "Win+R");
        let text = draw(&a, 120, 32);
        assert!(
            has(&text, "✓ win+r") && has(&text, "許可リストに入れても効かない"),
            "{text}"
        );
        a.update(&ctrl('u'));
        type_str(&mut a, "ctrl+nope");
        let text = draw(&a, 120, 32);
        assert!(has(&text, "✗"), "{text}");
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(a.overlay, Overlay::Edit { .. }),
            "不正なら入力欄に留まる"
        );
        assert_eq!(a.status_kind, StatusKind::Error);
    }

    #[test]
    fn tui_regions_picker_offers_screen_presets() {
        let mut a = app();
        a.screens = vec![(
            "主画面の全体 (3840x2160)".into(),
            Rect::new(0, 0, 3840, 2160),
        )];
        select(&mut a, Field::List(Side::Deny, ListKind::Regions));
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::None,
            "領域はウィンドウを読み込まない"
        );
        a.update(&press(KeyCode::Down));
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.deny.regions, vec!["0,0,3840,2160".to_string()]);
    }

    #[test]
    fn tui_line_editor_cursor_keys_and_wide_chars() {
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut e = LineEditor::new("abc");
        e.handle(key(KeyCode::Char('x')));
        assert_eq!(e.text(), "x", "開いた直後の入力は全体を置き換える");
        e.handle(key(KeyCode::Home));
        e.handle(key(KeyCode::Char('日')));
        assert_eq!(e.text(), "日x");
        assert_eq!(
            e.view(10),
            ("日x".to_string(), 2),
            "全角はカーソルを 2 桁進める"
        );
        e.handle(key(KeyCode::End));
        e.handle(key(KeyCode::Left));
        e.handle(key(KeyCode::Delete));
        assert_eq!(e.text(), "日");
        e.handle(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "");
        let long = LineEditor::new(&"あ".repeat(30));
        let (shown, cursor) = long.view(10);
        assert!(
            UnicodeWidthStr::width(shown.as_str()) <= 10 && cursor <= 10,
            "長い値は横にずらす: {shown} {cursor}"
        );
    }

    #[test]
    fn tui_enabling_input_requires_confirmation() {
        let mut a = app();
        a.update(&press(KeyCode::Enter));
        assert!(matches!(a.overlay, Overlay::Confirm { .. }));
        let text = draw(&a, 120, 32);
        assert!(has(&text, "どのアプリにも操作できる"), "{text}");
        a.update(&press(KeyCode::Char('n')));
        assert!(!a.config.general.input_enabled);
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char('y')));
        assert!(a.config.general.input_enabled);
        // 無効にするのは確認なし
        a.update(&press(KeyCode::Enter));
        assert!(!a.config.general.input_enabled);
    }

    #[test]
    fn tui_removing_last_allow_item_confirms_and_undo_restores() {
        let mut cfg = Config::default();
        cfg.allow.processes = vec!["notepad.exe".into()];
        let mut a = App::new(target(false), cfg);
        select(&mut a, Field::List(Side::Allow, ListKind::Processes));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char('d')));
        assert!(
            matches!(a.overlay, Overlay::Confirm { .. }),
            "最後の 1 件は確認"
        );
        a.update(&press(KeyCode::Char('y')));
        assert!(a.config.allow.processes.is_empty());
        a.update(&press(KeyCode::Char('u')));
        assert_eq!(a.config.allow.processes, vec!["notepad.exe".to_string()]);
    }

    #[test]
    fn tui_save_summarizes_widening_changes() {
        let old = Config::default();
        let mut new = old.clone();
        new.general.input_enabled = true;
        new.deny.keys = vec!["ctrl+w".into()];
        let w = widening_changes(&old, &new);
        assert_eq!(
            w,
            vec!["実入力を有効にする".to_string()],
            "deny の追加は広げる変更ではない"
        );
        let mut base = Config::default();
        base.allow.processes = vec!["notepad.exe".into()];
        base.deny.processes = vec!["cmd.exe".into()];
        let mut next = base.clone();
        next.allow.processes.push("calc.exe".into());
        next.deny.processes.clear();
        next.limits.max_actions += 1;
        assert_eq!(widening_changes(&base, &next).len(), 3);
        let mut empty = base.clone();
        empty.allow.processes.clear();
        assert!(widening_changes(&base, &empty)[0].contains("制限なし"));

        // 広げる変更があれば確認、無ければそのまま保存
        let mut a = app();
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char('y')));
        assert_eq!(a.update(&press(KeyCode::Char('s'))), Effect::None);
        let text = draw(&a, 120, 32);
        assert!(has(&text, "⚠ 実入力を有効にする"), "{text}");
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::Save);
        a.on_saved(Ok(()));
        assert!(!a.dirty());
        select(&mut a, Field::MaxActions);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "25");
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.config.limits.max_actions, 25,
            "開いた直後の入力で置き換わる"
        );
        assert_eq!(a.update(&ctrl('s')), Effect::Save, "下げる変更は確認なし");
    }

    /// パスワード欄への入力 (D-090): 全般の項目で、有効にする向きは確認 (パスワードの文で、撮影の文ではない)、
    /// 無効にする向きは即時。描画では左の一覧に名前と値が出る
    #[test]
    fn password_input_field_toggles_and_renders() {
        let mut a = app();
        assert!(fields().contains(&Field::PasswordInput));
        assert_eq!(
            Field::PasswordInput.location(),
            "全般 (総スイッチ) > パスワード欄への入力"
        );
        select(&mut a, Field::PasswordInput);
        let text = draw(&a, 120, 32);
        assert!(has(&text, "パスワード欄への入力"), "{text}");
        assert!(has(&text, "general.password_input"), "{text}");
        a.update(&press(KeyCode::Enter));
        match &a.overlay {
            Overlay::Confirm { lines, .. } => {
                // 1 行目は f.name() から作るので「パスワード」を必ず含む。専用の文だけが持つ句で見る
                assert!(
                    lines
                        .iter()
                        .any(|l| l.contains("パスワードを AI に打たせてよいときだけ")),
                    "{lines:?}"
                );
                assert!(
                    !lines.iter().any(|l| l.contains("個人情報が写りうる")),
                    "撮影の文に落ちない: {lines:?}"
                );
            }
            o => panic!("有効にする向きは確認: {o:?}"),
        }
        let text = draw(&a, 120, 32);
        assert!(has(&text, "パスワード欄への入力を有効にする。"), "{text}");
        a.update(&press(KeyCode::Char('n')));
        assert!(!a.config.general.password_input);
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char('y')));
        assert!(a.config.general.password_input);
        assert!(!a.config.general.input_enabled, "他の総スイッチは動かない");
        let text = draw(&a, 120, 32);
        assert!(has(&text, "パスワード欄への入力  ⚠ 有効"), "{text}");
        a.update(&press(KeyCode::Enter));
        assert!(!a.config.general.password_input, "無効にする向きは即時");
    }

    /// パスワード欄への入力をオンにする変更は許可を広げる変更として並び、オフにする変更は並ばない (D-090)
    #[test]
    fn widening_changes_lists_password_input() {
        let off = Config::default();
        let mut on = off.clone();
        on.general.password_input = true;
        assert_eq!(
            widening_changes(&off, &on),
            vec!["パスワード欄への入力を許す".to_string()]
        );
        assert!(widening_changes(&on, &off).is_empty());
    }

    /// 撮影の制限 (D-049): 全画面は切り替え (有効にする向きは確認で、保存のときも ⚠ が出る)、
    /// 無操作の時間は数値の入力 (0 以上)
    #[test]
    fn tui_capture_fields_toggle_and_edit() {
        let mut a = app();
        let fs = fields();
        assert!(fs.contains(&Field::CaptureFullScreen));
        assert!(fs.contains(&Field::CaptureIdleMs));
        select(&mut a, Field::CaptureFullScreen);
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(a.overlay, Overlay::Confirm { .. }),
            "許可を広げるので確認"
        );
        a.update(&press(KeyCode::Char('y')));
        assert!(a.config.capture.full_screen);
        assert_eq!(a.update(&press(KeyCode::Char('s'))), Effect::None);
        assert!(
            matches!(&a.overlay, Overlay::Confirm { lines, .. } if lines.iter().any(|l| l.contains("全画面"))),
            "保存のときも広げる変更として並ぶ: {:?}",
            a.overlay
        );
        a.update(&press(KeyCode::Esc));
        a.update(&press(KeyCode::Enter));
        assert!(!a.config.capture.full_screen, "無効にする向きは即時");
        select(&mut a, Field::CaptureIdleMs);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "0");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.capture.min_idle_ms, 0);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "abc");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.capture.min_idle_ms, 0, "不正な値は変えない");
        a.update(&press(KeyCode::Esc));
        // 無操作の時間を短くするのは緩める向きなので、保存のときに ⚠ で並ぶ
        assert_eq!(a.update(&ctrl('s')), Effect::None);
        assert!(
            matches!(&a.overlay, Overlay::Confirm { lines, .. } if lines.iter().any(|l| l.contains("無操作"))),
            "{:?}",
            a.overlay
        );
        a.update(&press(KeyCode::Esc));
        let text = draw(&a, 120, 32);
        assert!(has(&text, "撮影の制限"), "{text}");
        assert!(has(&text, "全画面の撮影"), "{text}");
    }

    /// 操作の前の枠 (D-093): 3 項目がグループ「表示」に並び、左の一覧に名前と値、右に設定のキーが出る
    #[test]
    fn tui_indicator_fields_render_in_display_group() {
        let mut a = app();
        let cases = [
            (
                Field::IndicatorEnabled,
                "indicator.enabled",
                "表示 > 操作の前の枠",
                "操作の前の枠  有効",
            ),
            (
                Field::IndicatorLeadMs,
                "indicator.lead_ms",
                "表示 > 予告の間 (ミリ秒)",
                "予告の間 (ミリ秒)  3000",
            ),
            (
                Field::IndicatorSound,
                "indicator.sound",
                "表示 > 予告の音",
                "予告の音  無効",
            ),
        ];
        for (f, key, location, row) in cases {
            assert!(fields().contains(&f), "{f:?}");
            assert_eq!(f.key(), key);
            assert_eq!(f.location(), location);
            select(&mut a, f);
            let text = draw(&a, 120, 40);
            assert!(has(&text, "── 表示"), "{text}");
            assert!(has(&text, row), "{text}");
            assert!(has(&text, key), "{text}");
        }
        // 3 項目は続けて並ぶ (グループの見出しが 1 回だけ出る)
        let fs = fields();
        let i = fs
            .iter()
            .position(|f| *f == Field::IndicatorEnabled)
            .unwrap();
        assert_eq!(
            fs[i..i + 3],
            [
                Field::IndicatorEnabled,
                Field::IndicatorLeadMs,
                Field::IndicatorSound
            ]
        );
        select(&mut a, Field::IndicatorLeadMs);
        let text = draw(&a, 120, 40);
        assert!(has(&text, "上限 30000"), "{text}");
    }

    /// 真偽の 2 項目は両向きとも確認なしで切り替わり、予告の間は数値の入力で変わる (D-093)
    #[test]
    fn tui_indicator_toggle_and_lead_edit() {
        let mut a = app();
        select(&mut a, Field::IndicatorEnabled);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None, "無効にする向きは即時");
        assert!(!a.config.indicator.enabled);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None, "有効にする向きも確認しない");
        assert!(a.config.indicator.enabled);
        select(&mut a, Field::IndicatorSound);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.config.indicator.sound);
        assert!(a.config.indicator.enabled, "他の項目は動かない");
        let text = draw(&a, 120, 40);
        assert!(has(&text, "予告の音  有効"), "{text}");

        select(&mut a, Field::IndicatorLeadMs);
        a.update(&press(KeyCode::Enter));
        assert!(matches!(a.overlay, Overlay::Edit { .. }));
        type_str(&mut a, "500");
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.config.indicator.lead_ms, 500,
            "開いた直後の入力で置き換わる"
        );
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "-1");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.indicator.lead_ms, 500, "不正な値は変えない");
        a.update(&press(KeyCode::Esc));
        a.update(&press(KeyCode::Esc));
        let text = draw(&a, 120, 40);
        assert!(has(&text, "予告の間 (ミリ秒)  500"), "{text}");
        assert!(a.dirty());
    }

    /// 上限 (30000) を超える予告の間は、入力欄では通るが保存の経路 (Layer::from_config) が断る。
    /// 保存の効果を返さないのでファイルは変わらない。30000 なら保存する (D-093)
    #[test]
    fn tui_indicator_lead_over_limit_is_refused_on_save() {
        let mut a = app();
        select(&mut a, Field::IndicatorLeadMs);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "30001");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.config.indicator.lead_ms, 30001);
        assert_eq!(a.update(&ctrl('s')), Effect::None, "保存しない");
        assert_eq!(a.overlay, Overlay::None, "確認も出さない");
        assert!(a.dirty(), "保存済みの設定は元のまま");
        assert_eq!(a.saved.indicator.lead_ms, 3000);
        // 右ペインの説明にも「保存しない」と key が出るので、ステータスの文にしか無い文で見る
        assert_eq!(a.status_kind, StatusKind::Error);
        assert!(
            a.status.starts_with("保存しない: ")
                && a.status.contains("indicator.lead_ms は 30000 以下"),
            "{}",
            a.status
        );
        let text = draw(&a, 120, 40);
        assert!(has(&text, "indicator.lead_ms は 30000 以下"), "{text}");

        select(&mut a, Field::IndicatorLeadMs);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "30000");
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.update(&ctrl('s')),
            Effect::Save,
            "上限ちょうどは通り、延ばす変更は確認なし"
        );
    }

    /// 知らせを弱める 3 つの変更 (枠をやめる、予告の間を縮める、音をやめる) が要約に並び、強める変更は並ばない (D-093)
    #[test]
    fn widening_changes_reports_indicator_weakening() {
        let mut strong = Config::default();
        strong.indicator.sound = true;
        let mut weak = strong.clone();
        weak.indicator.enabled = false;
        weak.indicator.lead_ms = 1000;
        weak.indicator.sound = false;
        assert_eq!(
            widening_changes(&strong, &weak),
            vec![
                "操作の前の枠をやめる (知らせを弱める。予告の間と音も無くなる)".to_string(),
                "予告の間を短くする (知らせを弱める。3000 → 1000 ms)".to_string(),
                "予告の音をやめる (知らせを弱める)".to_string(),
            ]
        );
        assert!(widening_changes(&weak, &strong).is_empty(), "強める変更");
        let mut longer = strong.clone();
        longer.indicator.lead_ms = 5000;
        assert!(widening_changes(&strong, &longer).is_empty(), "延ばす変更");
        assert!(widening_changes(&strong, &strong).is_empty());

        // 保存のときも ⚠ で並ぶ
        let mut a = app();
        select(&mut a, Field::IndicatorEnabled);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.update(&ctrl('s')), Effect::None);
        assert!(
            matches!(&a.overlay, Overlay::Confirm { lines, .. } if lines.iter().any(|l| l.contains("⚠ 操作の前の枠をやめる"))),
            "{:?}",
            a.overlay
        );
    }

    /// 撮影の保存先のフォルダ (D-075): ; 区切りの 1 行で編集し、設定の規則 (絶対パス) を通す
    #[test]
    fn tui_edits_capture_save_dirs_as_semicolon_list() {
        let mut a = app();
        assert!(fields().contains(&Field::CaptureSaveDirs));
        select(&mut a, Field::CaptureSaveDirs);
        let text = draw(&a, 120, 32);
        assert!(has(&text, "撮影の保存先のフォルダ"), "{text}");
        assert!(has(&text, "現在の値: 制限なし"), "{text}");
        assert!(has(&text, "絶対パスを ; で区切る"), "{text}");
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text().is_empty()),
            "空の一覧の入力欄は空: {:?}",
            a.overlay
        );
        let text = draw(&a, 120, 32);
        assert!(has(&text, "✓ 空 (制限なし)"), "{text}");
        type_str(&mut a, "C:\\a; C:\\b ;");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.config.capture.save_dirs, vec!["C:\\a", "C:\\b"]);
        assert_eq!(
            short_value(&a.config, Field::CaptureSaveDirs),
            "C:\\a;C:\\b"
        );
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text() == "C:\\a;C:\\b"),
            "{:?}",
            a.overlay
        );
        type_str(&mut a, "rel");
        a.update(&press(KeyCode::Enter));
        assert!(matches!(a.overlay, Overlay::Edit { .. }), "確定できない");
        assert_eq!(
            a.config.capture.save_dirs,
            vec!["C:\\a", "C:\\b"],
            "相対パスは拒否され値が変わらない"
        );
        a.update(&ctrl('u'));
        type_str(&mut a, " ; ");
        let text = draw(&a, 120, 32);
        assert!(
            has(&text, "✓ 空 (制限なし)"),
            "区切りだけは空の一覧: {text}"
        );
        assert!(
            check_scalar_input(Field::CaptureSaveDirs, "C:\\a;%USERPROFILE%\\s")
                .value
                .is_err()
        );
        assert_eq!(
            check_scalar_input(Field::CaptureSaveDirs, " ; ").value,
            Ok(String::new())
        );
    }

    #[test]
    fn widening_changes_reports_save_dirs_loosening() {
        let cfg = |d: &[&str]| {
            let mut c = Config::default();
            c.capture.save_dirs = d.iter().map(|s| s.to_string()).collect();
            c
        };
        let w = |o: &[&str], n: &[&str]| widening_changes(&cfg(o), &cfg(n));
        assert_eq!(w(&["C:\\a"], &[]), vec!["撮影の保存先の制限をやめる"]);
        assert_eq!(
            w(&["C:\\a"], &["C:\\a", "D:\\x"]),
            vec!["撮影の保存先のフォルダを足す (D:\\x)"]
        );
        assert_eq!(
            w(&["C:\\a"], &["C:\\ab"]),
            vec!["撮影の保存先のフォルダを足す (C:\\ab)"],
            "部品の単位で比べる"
        );
        assert!(w(&["C:\\a"], &["C:\\a", "C:\\a\\b"]).is_empty(), "下を足す");
        assert!(w(&["C:\\a"], &["c:\\A\\b"]).is_empty(), "下に絞る");
        assert!(w(&["C:\\a", "C:\\b"], &["C:\\a"]).is_empty(), "減らす");
        assert!(w(&[], &["C:\\a"]).is_empty(), "制限なしから絞る");
        assert_eq!(
            w(&["C:\\a"], &["C:\\a\\b", "rel"]),
            vec!["撮影の保存先のフォルダを足す (C:\\a\\b, rel)"],
            "畳めなければ安全側に全部を並べる"
        );
    }

    /// 毎回の承認を求める MCP の道具 (D-078): , 区切りの 1 行で編集し、名前は config::normalized_mcp_tools で検査する
    #[test]
    fn tui_edits_mcp_require_approval_as_comma_list() {
        let mut a = app();
        assert!(fields().contains(&Field::McpRequireApproval));
        select(&mut a, Field::McpRequireApproval);
        let text = draw(&a, 120, 32);
        assert!(has(&text, "毎回の承認を求める MCP の道具"), "{text}");
        assert!(has(&text, "現在の値: なし"), "{text}");
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text().is_empty()),
            "空の一覧の入力欄は空: {:?}",
            a.overlay
        );
        let text = draw(&a, 120, 32);
        assert!(has(&text, "✓ 空 (なし)"), "{text}");
        type_str(&mut a, "click, Type");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.config.mcp.require_approval, vec!["click", "Type"]);
        assert_eq!(
            short_value(&a.config, Field::McpRequireApproval),
            "click, Type"
        );
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text() == "click, Type"),
            "{:?}",
            a.overlay
        );
        a.update(&ctrl('u'));
        type_str(&mut a, "click,clik");
        a.update(&press(KeyCode::Enter));
        assert!(matches!(a.overlay, Overlay::Edit { .. }), "確定できない");
        assert_eq!(
            a.config.mcp.require_approval,
            vec!["click", "Type"],
            "未知の名前は拒否され値が変わらない"
        );
        let err = check_scalar_input(Field::McpRequireApproval, "click,clik")
            .value
            .unwrap_err();
        assert!(err.contains("clik"), "{err}");
        assert_eq!(
            check_scalar_input(Field::McpRequireApproval, "").value,
            Ok(String::new())
        );
        assert_eq!(
            check_scalar_input(Field::McpRequireApproval, " , ").value,
            Ok(String::new())
        );
    }

    #[test]
    fn widening_changes_reports_removed_approval_tools() {
        let cfg = |t: &[&str]| {
            let mut c = Config::default();
            c.mcp.require_approval = t.iter().map(|s| s.to_string()).collect();
            c
        };
        let w = |o: &[&str], n: &[&str]| widening_changes(&cfg(o), &cfg(n));
        assert_eq!(
            w(&["click", "type"], &["click"]),
            vec!["毎回の承認をやめる MCP の道具: type"]
        );
        assert!(w(&["click"], &["click", "type"]).is_empty(), "足すだけ");
        assert!(w(&["click"], &["click"]).is_empty(), "同じ");
        assert!(
            w(&["Click"], &["click"]).is_empty(),
            "大文字小文字の違いだけ"
        );
        assert!(w(&[" type "], &["type"]).is_empty(), "前後の空白の違いだけ");
        assert!(
            w(&["clik"], &[]).is_empty(),
            "古い側が読めない値なら挙げない"
        );
    }

    #[test]
    fn tui_quit_flows_esc_q_and_ctrl_c() {
        let mut a = app();
        assert_eq!(
            a.update(&press(KeyCode::Esc)),
            Effect::Quit,
            "未保存が無ければ主画面の Esc で終了 (D-130)"
        );
        let mut a = app();
        assert_eq!(
            a.update(&ctrl('c')),
            Effect::Quit,
            "未保存が無ければ Ctrl+C で終了"
        );
        let mut a = app();
        select(&mut a, Field::MaxActions);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "7");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.update(&press(KeyCode::Char('q'))), Effect::None);
        assert_eq!(a.overlay, Overlay::QuitDialog);
        assert_eq!(
            a.update(&press(KeyCode::Char('s'))),
            Effect::SaveAndQuit,
            "保存して終了 (下げる変更なので確認なし)"
        );
        a.update(&ctrl('c'));
        assert_eq!(
            a.overlay,
            Overlay::QuitDialog,
            "未保存があれば Ctrl+C は確認へ"
        );
        assert_eq!(
            a.update(&ctrl('c')),
            Effect::Quit,
            "確認の画面でもう一度 Ctrl+C で強制終了"
        );
    }

    #[test]
    fn tui_quit_dialog_save_still_confirms_widening_and_then_quits() {
        let mut cfg = Config::default();
        cfg.deny.processes = vec!["cmd.exe".into()];
        let mut a = App::new(target(false), cfg);
        select(&mut a, Field::List(Side::Deny, ListKind::Processes));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Char('d')));
        assert!(
            a.config.deny.processes.is_empty(),
            "拒否リストの削除は確認なし"
        );
        a.update(&press(KeyCode::Char('q')));
        assert_eq!(a.update(&press(KeyCode::Char('s'))), Effect::None);
        assert!(
            matches!(&a.overlay, Overlay::Confirm { lines, .. } if lines.iter().any(|l| l.contains("cmd.exe")))
        );
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::SaveAndQuit);
        // 変更が無ければ、保存して終了はそのまま終了
        let mut a = app();
        a.overlay = Overlay::QuitDialog;
        assert_eq!(a.update(&press(KeyCode::Char('s'))), Effect::Quit);
    }

    #[test]
    fn tui_ctrl_s_while_typing_keeps_the_input() {
        let mut a = app();
        select(&mut a, Field::List(Side::Deny, ListKind::Keys));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "ctrl+w");
        assert_eq!(a.update(&ctrl('s')), Effect::None);
        assert!(matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text() == "ctrl+w"));
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.update(&ctrl('s')), Effect::Save, "確定した後なら保存");
    }

    #[test]
    fn tui_checklist_reads_aliases_and_keeps_unlisted_entries() {
        let mut cfg = Config::default();
        cfg.allow.actions = vec!["Click".into(), "wait".into()];
        let mut a = App::new(target(false), cfg);
        select(&mut a, Field::List(Side::Allow, ListKind::Actions));
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Enter));
        assert!(
            matches!(&a.overlay, Overlay::Checklist { checked, .. } if checked[0]),
            "Click を click として選択済みにする"
        );
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.config.allow.actions,
            vec!["Click".to_string(), "wait".to_string()],
            "変更なしなら書き換えない"
        );
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Down));
        a.update(&press(KeyCode::Char(' ')));
        a.update(&press(KeyCode::Enter));
        assert_eq!(
            a.config.allow.actions,
            vec![
                "wait".to_string(),
                "click".to_string(),
                "double_click".to_string()
            ]
        );
    }

    #[test]
    fn tui_scalar_edit_then_next_key_is_not_swallowed_and_delete_clears() {
        let mut a = app();
        select(&mut a, Field::MaxActions);
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Delete));
        assert!(
            matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text().is_empty()),
            "開いた直後の Delete で全消去"
        );
        type_str(&mut a, "30");
        a.update(&press(KeyCode::Enter));
        let before = a.selected;
        a.update(&press(KeyCode::Down));
        assert_eq!(a.selected, before + 1, "確定の後の最初のキーが効く");
    }

    #[test]
    fn tui_right_pane_header_wraps_at_80_columns() {
        let mut a = app();
        select(&mut a, Field::List(Side::Allow, ListKind::Processes));
        let text = draw(&a, 80, 24);
        assert!(has(&text, "書き方:"), "{text}");
        a.update(&press(KeyCode::Enter));
        a.update(&press(KeyCode::Enter));
        a.set_windows(windows());
        let text = draw(&a, 80, 24);
        assert!(
            has(&text, "今の値: 制限なし"),
            "追加の最中も今の値が見える\n{text}"
        );
    }

    #[test]
    fn tui_banner_warns_when_unrestricted_and_overlay_title() {
        let mut cfg = Config::default();
        cfg.general.input_enabled = true;
        let text = draw(&App::new(target(true), cfg.clone()), 120, 32);
        assert!(has(&text, "どのアプリ、どの画面にも実入力できる"), "{text}");
        assert!(
            has(&text, "追加の層: allow は基本の層より広げられない"),
            "{text}"
        );
        cfg.allow.processes = vec!["notepad.exe".into()];
        let text = draw(&App::new(target(false), cfg), 120, 32);
        assert!(!has(&text, "どのアプリ、どの画面にも"), "{text}");
    }

    #[test]
    fn tui_narrow_terminal_shows_one_pane_with_breadcrumb() {
        let mut a = app();
        select(&mut a, Field::List(Side::Allow, ListKind::Keys));
        let text = draw(&a, 60, 24);
        assert!(has(&text, "項目"), "{text}");
        a.update(&press(KeyCode::Enter));
        let text = draw(&a, 60, 24);
        println!("{text}");
        assert!(has(&text, "許可リスト (allow) > キー"), "{text}");
        assert!(!has(&text, "全般 (総スイッチ)"), "左の一覧は隠れる\n{text}");
    }

    #[test]
    fn tui_scalar_messages_and_sentinel_display() {
        assert_eq!(
            check_scalar_input(Field::MaxDurationMs, "1000").value,
            Err("1000 より大きい整数".to_string())
        );
        assert!(
            check_scalar_input(Field::MaxDurationMs, "1001")
                .value
                .is_ok()
        );
        assert!(
            check_scalar_input(Field::SentinelFile, "stop.txt")
                .value
                .is_err()
        );
        let mut cfg = Config::default();
        cfg.stop.sentinel_file = "C:\\stop\\ai-cu.STOP".into();
        assert_eq!(
            short_value(&cfg, Field::SentinelFile),
            "C:\\stop\\ai-cu.STOP",
            "引用符もエスケープも付けない"
        );
    }

    #[test]
    fn tui_release_events_are_ignored() {
        let mut a = app();
        assert_eq!(a.update(&release(KeyCode::Down)), Effect::None);
        assert_eq!(a.selected, 0);
        a.update(&release(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None, "Release で確認が開かない");
    }

    #[test]
    fn tui_edit_accepts_altgr_characters() {
        let mut a = app();
        select(&mut a, Field::SentinelFile);
        a.update(&press(KeyCode::Enter));
        a.update(&Event::Key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));
        assert!(matches!(&a.overlay, Overlay::Edit { editor, .. } if editor.text() == "@"));
    }

    #[test]
    fn tui_resize_redraws_at_new_size() {
        let a = app();
        let mut t = Terminal::new(TestBackend::new(120, 32)).unwrap();
        t.draw(|f| render(&a, f)).unwrap();
        assert_eq!(
            App::new(target(false), Config::default()).update(&Event::Resize(60, 20)),
            Effect::None
        );
        t.backend_mut().resize(60, 20);
        t.resize(ratatui::layout::Rect::new(0, 0, 60, 20)).unwrap();
        t.draw(|f| render(&a, f)).unwrap();
        assert_eq!(t.backend().buffer().area.width, 60);
        assert!(has(&buffer_text(&t), "ai-cu 設定"));
    }

    /// 二重線の枠 (ダイアログだけが使う) の左の桁と、上下の行を返す
    fn double_box(buf: &Buffer, w: u16) -> (u16, u16, u16) {
        let find = |s: &str| {
            let i = buf
                .content
                .iter()
                .position(|c| c.symbol() == s)
                .unwrap_or_else(|| panic!("幅 {w}: {s} が無い"));
            buf.pos_of(i)
        };
        let (x0, y0) = find("╔");
        let (_, y1) = find("╚");
        (x0, y0, y1)
    }

    /// ratatui の差分描画は、幅 2 の文字のセルの次を飛ばす (docs/design-notes.md)。左ペインを描いた後に
    /// ヘルプを重ねたとき、ダイアログの左端が左ペインの全角文字の右半分に乗る行でも、枠の左端 (╔ ║ ╚) が
    /// 端末 (差分の適用後の TestBackend) に届くこと。閉じた後は消した全角が元に戻ること (D-046)
    #[test]
    fn tui_dialog_left_border_reaches_terminal_beside_wide_chars() {
        let mut a = app();
        let mut hazard_rows = 0;
        for w in 90..=140u16 {
            let mut t = Terminal::new(TestBackend::new(w, 32)).unwrap();
            assert!(matches!(a.overlay, Overlay::None));
            let base = t.draw(|f| render(&a, f)).unwrap().buffer.clone();
            a.update(&press(KeyCode::Char('?')));
            assert!(matches!(a.overlay, Overlay::Help));
            let logical = t.draw(|f| render(&a, f)).unwrap().buffer.clone();
            let (x0, y0, y1) = double_box(&logical, w);
            let shown = t.backend().buffer();
            for y in y0..=y1 {
                if x0 > 0 && base[(x0 - 1, y)].cell_width() > 1 {
                    hazard_rows += 1;
                }
                assert_eq!(
                    shown[(x0, y)].symbol(),
                    logical[(x0, y)].symbol(),
                    "幅 {w} の行 {y}: 枠の左端が端末に届いていない"
                );
            }
            a.update(&press(KeyCode::Esc));
            let restored = t.draw(|f| render(&a, f)).unwrap().buffer.clone();
            let shown = t.backend().buffer();
            let x = x0.saturating_sub(1);
            for y in y0..=y1 {
                assert_eq!(
                    shown[(x, y)].symbol(),
                    restored[(x, y)].symbol(),
                    "幅 {w} の行 {y}: 閉じた後に左ペインが戻っていない"
                );
            }
        }
        assert!(
            hazard_rows > 0,
            "全角の右半分に乗る行が 1 つも無く、検出力が無い"
        );
    }

    /// 仕組みの単体テスト (ヘルプの文言に依らない): 全角だけで埋めた画面の上にダイアログを重ねても、枠の左端が
    /// 端末に届く。幅を 4 つ回すと、左端が全角の右半分に乗る幅が必ず含まれる (D-046)
    #[test]
    fn tui_dialog_left_border_over_wide_background() {
        let bg = || Paragraph::new(vec![Line::raw("あ".repeat(40)); 12]);
        let mut hazard_rows = 0;
        for w in 60..=63u16 {
            let mut t = Terminal::new(TestBackend::new(w, 12)).unwrap();
            let base = t
                .draw(|f| {
                    let area = f.area();
                    f.render_widget(bg(), area);
                })
                .unwrap()
                .buffer
                .clone();
            let logical = t
                .draw(|f| {
                    let area = f.area();
                    f.render_widget(bg(), area);
                    render_dialog(f, area, "t", &["x".into()], "k", Tone::Info, &theme::CALM);
                })
                .unwrap()
                .buffer
                .clone();
            let (x0, y0, y1) = double_box(&logical, w);
            let shown = t.backend().buffer();
            for y in y0..=y1 {
                if base[(x0 - 1, y)].cell_width() > 1 {
                    hazard_rows += 1;
                }
                assert_eq!(
                    shown[(x0, y)].symbol(),
                    logical[(x0, y)].symbol(),
                    "幅 {w} の行 {y}: 枠の左端が端末に届いていない"
                );
            }
        }
        assert!(hazard_rows > 0);
    }

    // -----------------------------------------------------------------------
    // 操作列の実行 (D-043)
    // -----------------------------------------------------------------------

    const ENABLED: &str = "[general]\ninput_enabled = true\ncapture_enabled = true\n";

    fn report(actions_json: &str, floor_toml: &str) -> PreflightReport {
        let cfg = Config::parse(&format!("version = 1\n{floor_toml}")).unwrap();
        let policy = crate::policy::Policy::new(vec![
            config::Layer::from_config(&cfg, "C:\\cfg\\config.toml").unwrap(),
        ]);
        let script = crate::script::Script::from_json(&format!(
            "{{\"version\":1,\"actions\":[{actions_json}]}}"
        ))
        .unwrap();
        PreflightReport::build(
            Path::new("C:\\scripts\\steps.json"),
            script,
            &policy,
            Some(Rect::new(0, 0, 3840, 2160)),
            vec!["C:\\x\\STOP".into()],
        )
    }

    fn me() -> WindowInfo {
        crate::testing::window("WindowsTerminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS")
    }

    fn scripts() -> Vec<ScriptCandidate> {
        ["a.json", "b.json"]
            .iter()
            .map(|n| ScriptCandidate {
                name: n.to_string(),
                path: PathBuf::from(format!("C:\\s\\{n}")),
            })
            .collect()
    }

    #[test]
    fn tui_run_key_r_opens_script_picker_and_lists_scripts() {
        let mut a = app();
        assert_eq!(a.update(&press(KeyCode::Char('r'))), Effect::LoadScripts);
        assert!(matches!(a.overlay, Overlay::ScriptPicker { .. }));
        let text = draw(&a, 120, 32);
        assert!(has(&text, "読み込み中"), "{text}");
        a.set_scripts(Ok(scripts()));
        let text = draw(&a, 120, 32);
        println!("{text}");
        for needle in ["操作列を実行", "a.json", "b.json", "入力したパス"] {
            assert!(has(&text, needle), "{needle}\n{text}");
        }
        type_str(&mut a, "b");
        let text = draw(&a, 120, 32);
        assert!(
            !has(&text, "a.json") && has(&text, "b.json"),
            "絞り込み\n{text}"
        );
        // 2 回目は読み込まない (F5 で読み直す)
        a.update(&press(KeyCode::Esc));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.update(&press(KeyCode::Char('r'))), Effect::None);
        assert_eq!(a.update(&press(KeyCode::F(5))), Effect::LoadScripts);
        assert!(a.scripts.is_none());
        a.set_scripts(Err("読めない".into()));
        assert_eq!(a.status_kind, StatusKind::Error);
    }

    #[test]
    fn tui_run_refuses_when_dirty_and_keeps_overlay_none() {
        let mut a = app();
        select(&mut a, Field::MaxActions);
        a.update(&press(KeyCode::Enter));
        type_str(&mut a, "7");
        a.update(&press(KeyCode::Enter));
        assert!(a.dirty());
        assert_eq!(a.update(&press(KeyCode::Char('r'))), Effect::None);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status_kind, StatusKind::Warn);
        assert!(
            a.status.contains("未保存") && a.status.contains("s で保存"),
            "{}",
            a.status
        );
    }

    #[test]
    fn tui_script_picker_enter_requests_preflight_for_selected_or_typed_path() {
        let mut a = app();
        a.set_scripts(Ok(scripts()));
        a.update(&press(KeyCode::Char('r')));
        // 行 0 は入力したパス。空のまま Enter は断る
        assert_eq!(a.update(&press(KeyCode::Enter)), Effect::None);
        assert_eq!(a.status_kind, StatusKind::Error);
        a.update(&press(KeyCode::Down));
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::Preflight(PathBuf::from("C:\\s\\a.json"))
        );
        // 手入力のパス
        a.update(&press(KeyCode::Esc));
        a.update(&press(KeyCode::Char('r')));
        type_str(&mut a, "C:\\x\\s.json");
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::Preflight(PathBuf::from("C:\\x\\s.json"))
        );
        // 実行の画面では Ctrl+S で保存しない
        assert_eq!(a.update(&ctrl('s')), Effect::None);
    }

    #[test]
    fn tui_preflight_failure_or_issues_show_result_and_cannot_arm() {
        let mut a = app();
        a.on_preflight(Err(crate::error::AppError::new(
            crate::error::ErrorKind::InvalidScript,
            "steps.json を読めない",
        )));
        assert!(matches!(a.overlay, Overlay::RunResult { ok: false, .. }));
        let text = draw(&a, 120, 32);
        assert!(
            has(&text, "[invalid_script]") && has(&text, "読めない"),
            "{text}"
        );
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::None);
        assert!(matches!(a.overlay, Overlay::RunResult { .. }), "y は無視");
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        // 検証の問題: 実入力が無効な床では policy_denied
        let r = report(r#"{"type":"key","keys":"a"}"#, "");
        assert!(!r.runnable());
        a.on_preflight(Ok(r));
        let text = draw(&a, 120, 32);
        println!("{text}");
        assert!(
            has(&text, "policy_denied") && has(&text, "actions[0]"),
            "{text}"
        );
        assert!(has(&text, "次の手"), "{text}");
        assert!(!has(&text, "y 実行"), "武装できない\n{text}");
        a.update(&press(KeyCode::Esc));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn tui_result_lines_cap_issues_at_8() {
        let actions: Vec<String> = (0..20)
            .map(|_| r#"{"type":"key","keys":"a"}"#.to_string())
            .collect();
        let r = report(&actions.join(","), "");
        assert_eq!(r.validation.issues.len(), 20);
        let lines = preflight_issue_lines(&r);
        let shown = lines.iter().filter(|l| l.contains("actions[")).count();
        assert_eq!(shown, 8, "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("他 12 件")), "{lines:?}");
    }

    /// 実行の失敗と事前検査の問題に fix があれば、次の手の下に「直し方:」の 1 行を出す。無ければ出さない (D-079)
    #[test]
    fn run_failure_shows_fix_line() {
        let fix_line = |lines: &[String]| lines.iter().find(|l| l.starts_with("直し方: ")).cloned();
        let e = AppError::new(crate::error::ErrorKind::ForegroundGuard, "だめ").with_deny(
            Some("allow.processes"),
            Some("許可してよければ、足す".into()),
        );
        let lines = error_lines(&e, None);
        assert_eq!(
            fix_line(&lines).as_deref(),
            Some("直し方: 許可してよければ、足す")
        );
        let at = lines
            .iter()
            .position(|l| l.starts_with("次の手: "))
            .unwrap();
        assert!(lines[at + 1].starts_with("直し方: "), "{lines:?}");
        let plain = AppError::new(crate::error::ErrorKind::ForegroundGuard, "だめ")
            .with_deny(Some("builtin"), None);
        assert_eq!(fix_line(&error_lines(&plain, None)), None);
        // 事前検査: 実入力が無効な床で key は general.input_enabled の問題になり、床の直し方が付く
        let r = report(r#"{"type":"key","keys":"a"}"#, "");
        let lines = preflight_issue_lines(&r);
        let fix = fix_line(&lines).expect("直し方の行");
        assert!(
            fix.contains("C:\\cfg\\config.toml の general.input_enabled を true にする"),
            "{lines:?}"
        );
        // 組み込みのキーだけなら直し方は無い
        let r = report(r#"{"type":"key","keys":"win+r"}"#, ENABLED);
        let lines = preflight_issue_lines(&r);
        assert!(!r.runnable());
        assert_eq!(fix_line(&lines), None, "{lines:?}");
        // 描画にも出る
        let mut a = app();
        a.on_preflight(Ok(report(r#"{"type":"key","keys":"a"}"#, "")));
        let text = draw(&a, 160, 40);
        assert!(has(&text, "直し方"), "{text}");
    }

    #[test]
    fn tui_run_confirm_shows_summary_warnings_and_y_arms() {
        let mut a = app();
        let r = report(
            r#"{"type":"key","keys":"ctrl+s"},{"type":"wait","ms":100},{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":4,"h":3}}"#,
            ENABLED,
        );
        assert!(r.runnable() && r.unrestricted);
        a.on_preflight(Ok(r.clone()));
        assert!(matches!(a.overlay, Overlay::RunConfirm { .. }));
        let text = draw(&a, 140, 40);
        println!("{text}");
        for needle in [
            "steps.json",
            "操作 3 件",
            "バッチ 1",
            "撮影 1",
            "C:\\cfg\\config.toml",
            "50 px",
            "C:\\x\\STOP",
            "⚠ 最初の操作はキー入力",
            "許可リスト",
            "y 実行",
        ] {
            assert!(has(&text, needle), "{needle}\n{text}");
        }
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::ArmRun);
        assert!(
            matches!(&a.overlay, Overlay::Countdown { remaining_ms, self_window: None, .. } if *remaining_ms == RUN_COUNTDOWN_MS)
        );
        // クリック先頭なら注意の文面が変わり、許可リストがあれば赤の警告は出ない
        let mut a = app();
        a.on_preflight(Ok(report(
            r#"{"type":"click","x":10,"y":10}"#,
            &format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"),
        )));
        let text = draw(&a, 140, 40);
        assert!(has(&text, "最初の操作はマウス"), "{text}");
        assert!(!has(&text, "⚠ 許可リスト"), "{text}");
        // y 以外は取りやめ
        a.update(&press(KeyCode::Char('n')));
        assert_eq!(a.overlay, Overlay::None);
        // focus 先頭なら、前面にしなくてよい旨の注意 (D-050)
        let mut a = app();
        a.on_preflight(Ok(report(
            r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"ctrl+s"}"#,
            &format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"),
        )));
        let text = draw(&a, 140, 40);
        assert!(has(&text, "最初の操作は focus"), "{text}");
        assert!(!has(&text, "⚠ 最初の操作はキー入力"), "{text}");
        assert_eq!(
            action_summary(&crate::script::Action::Focus {
                process: "notepad.exe".into(),
                title: Some("メモ".into())
            }),
            "focus notepad.exe \"メモ\""
        );
    }

    #[test]
    fn tui_run_countdown_ticks_ignores_keys_and_fires_run_once() {
        let mut a = app();
        let r = report(r#"{"type":"click","x":10,"y":10}"#, ENABLED);
        a.on_preflight(Ok(r.clone()));
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::ArmRun);
        a.set_self_window(Some(me()));
        assert_eq!(a.tick(1000), Effect::None);
        let text = draw(&a, 120, 32);
        assert!(has(&text, "実行まで 4 秒"), "{text}");
        assert_eq!(a.update(&press(KeyCode::Char('x'))), Effect::None);
        assert!(
            matches!(a.overlay, Overlay::Countdown { .. }),
            "他のキーは無視"
        );
        match a.tick(RUN_COUNTDOWN_MS) {
            Effect::RunScript(req) => {
                assert_eq!(req.report, r);
                assert_eq!(req.self_window, me());
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(a.overlay, Overlay::Running { .. }));
        let text = draw(&a, 120, 32);
        assert!(has(&text, "実行中") && has(&text, "マウス"), "{text}");
        assert_eq!(a.tick(1000), Effect::None, "2 回目は無い");
        assert_eq!(
            a.update(&press(KeyCode::Char('q'))),
            Effect::None,
            "実行中は q も無視"
        );
    }

    #[test]
    fn tui_run_countdown_esc_and_q_cancel_without_firing() {
        for key in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('Q')] {
            let mut a = app();
            a.on_preflight(Ok(report(r#"{"type":"click","x":10,"y":10}"#, ENABLED)));
            a.update(&press(KeyCode::Char('y')));
            a.set_self_window(Some(me()));
            assert_eq!(a.update(&press(key)), Effect::None);
            assert_eq!(a.overlay, Overlay::None, "{key:?}");
            assert!(a.status.contains("取りやめ"), "{}", a.status);
            assert_eq!(a.tick(RUN_COUNTDOWN_MS), Effect::None);
        }
        // Ctrl+C は従来どおり終了 (D-043)。何も送らない
        let mut a = app();
        a.on_preflight(Ok(report(r#"{"type":"click","x":10,"y":10}"#, ENABLED)));
        a.update(&press(KeyCode::Char('y')));
        assert_eq!(a.update(&ctrl('c')), Effect::Quit);
    }

    #[test]
    fn tui_run_countdown_refuses_to_fire_without_self_window() {
        // 前面のウィンドウ (自分の端末) が取れなければ、除外なしでは走らせない (D-045、fail-closed)
        let mut a = app();
        a.on_preflight(Ok(report(r#"{"type":"click","x":10,"y":10}"#, ENABLED)));
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::ArmRun);
        a.set_self_window(None);
        assert_eq!(a.tick(RUN_COUNTDOWN_MS), Effect::None);
        assert!(
            matches!(a.overlay, Overlay::RunResult { ok: false, .. }),
            "{:?}",
            a.overlay
        );
        let text = draw(&a, 120, 32);
        assert!(
            has(&text, "特定できない") && has(&text, "何も送っていない"),
            "{text}"
        );
        assert_eq!(a.status_kind, StatusKind::Error);
    }

    #[test]
    fn tui_run_reopen_after_preflight_keeps_list_and_prefills_path() {
        let mut a = app();
        a.set_scripts(Ok(scripts()));
        a.update(&press(KeyCode::Char('r')));
        a.update(&press(KeyCode::Down));
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::Preflight(PathBuf::from("C:\\s\\a.json"))
        );
        a.on_preflight(Ok(report(r#"{"type":"click","x":10,"y":10}"#, ENABLED)));
        a.update(&press(KeyCode::Esc));
        // 2 回目: 前回のパスが入力欄に入っていても、一覧は絞り込まれずに全件出る
        a.update(&press(KeyCode::Char('r')));
        let text = draw(&a, 120, 32);
        println!("{text}");
        assert!(has(&text, "steps.json"), "前回のパスが入力欄にある\n{text}");
        assert!(has(&text, "a.json") && has(&text, "b.json"), "{text}");
        a.update(&press(KeyCode::Down));
        a.update(&press(KeyCode::Down));
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::Preflight(PathBuf::from("C:\\s\\b.json")),
            "一覧から別のファイルを選べる"
        );
        // 行 0 の Enter は前回のパスをそのまま使う
        a.update(&press(KeyCode::Esc));
        a.update(&press(KeyCode::Char('r')));
        assert_eq!(
            a.update(&press(KeyCode::Enter)),
            Effect::Preflight(PathBuf::from("C:\\scripts\\steps.json"))
        );
        // 文字を打つと置き換わり、絞り込みが効く
        a.update(&press(KeyCode::Esc));
        a.update(&press(KeyCode::Char('r')));
        type_str(&mut a, "b");
        let text = draw(&a, 120, 32);
        assert!(!has(&text, "a.json") && has(&text, "b.json"), "{text}");
    }

    #[test]
    fn tui_run_result_ignores_ctrl_c_as_stray_key() {
        // 読み捨てをすり抜けた打鍵 (操作列の ctrl+c など) で TUI を落とさない (D-045)
        let mut a = app();
        a.on_preflight(Err(crate::error::AppError::new(
            crate::error::ErrorKind::InvalidScript,
            "x",
        )));
        assert_eq!(a.update(&ctrl('c')), Effect::None);
        assert!(matches!(a.overlay, Overlay::RunResult { .. }));
        assert_eq!(a.update(&ctrl('s')), Effect::None);
        a.update(&press(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.update(&ctrl('c')), Effect::Quit, "閉じた後は従来どおり");
    }

    #[test]
    fn tui_dialog_wraps_long_lines_and_keeps_keys_visible() {
        // 長い hint (単語の区切りが無い日本語) でも、末尾のキーの案内が切れない
        let mut a = app();
        a.on_preflight(Ok(report(r#"{"type":"key","keys":"a"}"#, ENABLED)));
        a.update(&press(KeyCode::Char('y')));
        a.set_self_window(Some(me()));
        a.tick(RUN_COUNTDOWN_MS);
        a.on_run_finished(Err(crate::error::AppError::new(
            crate::error::ErrorKind::ForegroundGuard,
            "前面のウィンドウ (process=\"WindowsTerminal.exe\", class=\"CASCADIA_HOSTING_WINDOW_CLASS\", pid=4242) は ai-cu を動かしている端末なので送らない",
        )
        .at(0)
        .with_hint(crate::run::SELF_WINDOW_HINT)));
        for (w, h) in [(120, 32), (60, 24)] {
            let text = draw(&a, w, h);
            println!("{text}");
            assert!(has(&text, "Enter / Esc 閉じる"), "{w}x{h}\n{text}");
            assert!(has(&text, "[foreground_guard]"), "{w}x{h}\n{text}");
        }
        // 確認の画面と、問題が多い結果の画面も、狭い端末で案内が残る
        let mut a = app();
        a.on_preflight(Ok(report(r#"{"type":"key","keys":"ctrl+s"}"#, ENABLED)));
        let text = draw(&a, 60, 24);
        println!("{text}");
        assert!(has(&text, "y 実行"), "{text}");
        let actions: Vec<String> = (0..20)
            .map(|_| r#"{"type":"key","keys":"a"}"#.to_string())
            .collect();
        a.on_preflight(Ok(report(&actions.join(","), "")));
        let text = draw(&a, 60, 24);
        println!("{text}");
        assert!(has(&text, "Enter / Esc 閉じる"), "{text}");
    }

    #[test]
    fn tui_step_advances_countdown_with_real_elapsed_even_when_keys_arrive() {
        let mut a = app();
        a.on_preflight(Ok(report(r#"{"type":"click","x":10,"y":10}"#, ENABLED)));
        assert_eq!(
            a.step(Some(&press(KeyCode::Char('y'))), 10),
            vec![Effect::ArmRun]
        );
        a.set_self_window(Some(me()));
        // キーが届いた周回でも時間は進む (無視するキー)
        assert_eq!(
            a.step(Some(&press(KeyCode::Char('x'))), 1200),
            vec![Effect::None]
        );
        assert!(
            matches!(&a.overlay, Overlay::Countdown { remaining_ms, .. } if *remaining_ms == RUN_COUNTDOWN_MS - 1210)
        );
        // タイムアウトの周回
        assert_eq!(a.step(None, 1000), vec![Effect::None]);
        let effects = a.step(None, RUN_COUNTDOWN_MS);
        assert!(matches!(effects[..], [Effect::RunScript(_)]), "{effects:?}");
        // カウントダウン以外では、時間を進めても何も起きない
        let mut b = app();
        assert_eq!(b.step(None, 10_000), vec![Effect::None]);
        assert_eq!(
            b.step(Some(&press(KeyCode::Down)), 10_000),
            vec![Effect::None]
        );
        assert_eq!(b.selected, 1);
    }

    /// 呼び出しを記録する runner (TUI の配線のテスト用)
    struct RecordingRunner {
        report: PreflightReport,
        foreground: Option<WindowInfo>,
        calls: Vec<String>,
        ran: Vec<(crate::script::Script, Option<WindowInfo>)>,
    }

    impl ScriptRunner for RecordingRunner {
        fn preflight(&mut self, path: &Path) -> Result<PreflightReport, AppError> {
            self.calls.push(format!("preflight {}", path.display()));
            Ok(self.report.clone())
        }
        fn foreground(&mut self) -> Option<WindowInfo> {
            self.calls.push("foreground".into());
            self.foreground.clone()
        }
        fn run(
            &mut self,
            script: &crate::script::Script,
            self_window: Option<&WindowInfo>,
        ) -> Result<RunReport, AppError> {
            self.calls.push("run".into());
            self.ran.push((script.clone(), self_window.cloned()));
            Ok(RunReport {
                executed: script.actions.len(),
                screenshots: Vec::new(),
                focused: Vec::new(),
                closed: Vec::new(),
                waited: Vec::new(),
                elapsed_ms: 1,
            })
        }
    }

    #[test]
    fn tui_apply_effect_wires_preflight_arm_and_run_to_the_runner() {
        let r = report(r#"{"type":"click","x":10,"y":10}"#, ENABLED);
        let mut runner = RecordingRunner {
            report: r.clone(),
            foreground: Some(me()),
            calls: Vec::new(),
            ran: Vec::new(),
        };
        let mut a = app();
        let path = PathBuf::from("C:\\s\\a.json");
        assert_eq!(
            apply_effect(&mut a, &Effect::Preflight(path.clone()), &mut runner),
            None
        );
        assert!(matches!(a.overlay, Overlay::RunConfirm { .. }));
        assert_eq!(a.update(&press(KeyCode::Char('y'))), Effect::ArmRun);
        assert_eq!(apply_effect(&mut a, &Effect::ArmRun, &mut runner), None);
        assert!(
            matches!(&a.overlay, Overlay::Countdown { self_window: Some(w), .. } if *w == me())
        );
        let Effect::RunScript(req) = a.tick(RUN_COUNTDOWN_MS) else {
            panic!("RunScript")
        };
        apply_run(&mut a, &req, &mut runner);
        assert_eq!(
            runner.calls,
            vec!["preflight C:\\s\\a.json", "foreground", "run"]
        );
        assert_eq!(runner.ran, vec![(r.script.clone(), Some(me()))]);
        assert!(matches!(a.overlay, Overlay::RunResult { ok: true, .. }));
        assert_eq!(apply_effect(&mut a, &Effect::Quit, &mut runner), Some(0));
    }

    /// close (D-062): 確認の画面は「待ちと撮影だけ」と言わず、閉じることを警告する。結果の画面は消えたかを出す
    #[test]
    fn tui_confirm_and_result_describe_close() {
        let r = report(
            r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#,
            ENABLED,
        );
        assert_eq!(r.first_batch, Some(FirstBatch::Close));
        assert_eq!(r.closes, 1);
        let lines = confirm_lines(&r).join("\n");
        assert!(lines.contains("最初の操作は close"), "{lines}");
        assert!(lines.contains("close が 1 件"), "{lines}");
        assert!(!lines.contains("待ちと撮影だけ"), "{lines}");
        let mut a = app();
        a.on_preflight(Ok(r));
        a.update(&press(KeyCode::Char('y')));
        a.set_self_window(Some(me()));
        a.tick(RUN_COUNTDOWN_MS);
        let info = |gone, hidden| crate::run::CloseInfo {
            action_index: 0,
            hwnd: 0x20,
            process: "notepad.exe".into(),
            title: "a.txt - メモ帳".into(),
            gone,
            hidden,
        };
        let finish = |a: &mut App, c: crate::run::CloseInfo| {
            a.on_run_finished(Ok(crate::run::RunReport {
                executed: 1,
                screenshots: Vec::new(),
                focused: Vec::new(),
                closed: vec![c],
                waited: Vec::new(),
                elapsed_ms: 1000,
            }));
            draw(a, 120, 32)
        };
        let text = finish(&mut a, info(false, false));
        assert!(has(&text, "まだ開いている"), "{text}");
        // 隠れただけ (トレイ。D-096) は「閉じた」と言わず、プロセスが動いていると言う
        let text = finish(&mut a, info(true, true));
        assert!(has(&text, "隠れた"), "{text}");
        assert!(has(&text, "プロセスは動いている"), "{text}");
        assert!(!has(&text, "閉じた"), "{text}");
        // 破棄は今までどおり「閉じた」
        let text = finish(&mut a, info(true, false));
        assert!(has(&text, "閉じた actions[0]"), "{text}");
        assert!(!has(&text, "隠れた"), "{text}");
    }

    #[test]
    fn tui_run_result_renders_report_and_error_with_hint() {
        let mut a = app();
        a.on_preflight(Ok(report(
            r#"{"type":"click","x":10,"y":10},{"type":"type","text":"hello"}"#,
            ENABLED,
        )));
        a.update(&press(KeyCode::Char('y')));
        a.set_self_window(Some(me()));
        a.tick(RUN_COUNTDOWN_MS);
        a.on_run_finished(Ok(crate::run::RunReport {
            executed: 2,
            screenshots: vec![crate::run::ShotInfo {
                action_index: 1,
                path: "C:\\out\\a.png".into(),
                x: 0,
                y: 0,
                width: 4,
                height: 3,
                scale: 1.0,
                window: None,
                settled: None,
                settle_ms: None,
            }],
            focused: Vec::new(),
            closed: Vec::new(),
            waited: Vec::new(),
            elapsed_ms: 1234,
        }));
        assert!(matches!(a.overlay, Overlay::RunResult { ok: true, .. }));
        let text = draw(&a, 120, 32);
        println!("{text}");
        for needle in ["成功", "2 操作", "1234 ms", "C:\\out\\a.png"] {
            assert!(has(&text, needle), "{needle}\n{text}");
        }
        assert_eq!(a.status_kind, StatusKind::Ok);
        assert_eq!(
            a.update(&press(KeyCode::Char('q'))),
            Effect::None,
            "読み捨てを抜けた打鍵は無視"
        );
        assert!(matches!(a.overlay, Overlay::RunResult { .. }));
        a.update(&press(KeyCode::Esc));
        assert_eq!(a.overlay, Overlay::None);
        // 失敗: 終了コードの名前、失敗した操作の添字と要約、次の手
        a.on_preflight(Ok(report(
            r#"{"type":"click","x":10,"y":10},{"type":"type","text":"hello"}"#,
            ENABLED,
        )));
        a.update(&press(KeyCode::Char('y')));
        a.set_self_window(Some(me()));
        a.tick(RUN_COUNTDOWN_MS);
        a.on_run_finished(Err(crate::error::AppError::new(
            crate::error::ErrorKind::ForegroundGuard,
            "前面が違う",
        )
        .at(1)
        .with_hint("対象を前面にする")));
        let text = draw(&a, 120, 32);
        println!("{text}");
        for needle in [
            "[foreground_guard]",
            "終了コード 7",
            "actions[1]",
            "type",
            "5 文字",
            "前面が違う",
            "対象を前面にする",
        ] {
            assert!(has(&text, needle), "{needle}\n{text}");
        }
        assert_eq!(a.status_kind, StatusKind::Error);
    }

    /// クリックの 1 行要約に修飾キーを正規の順で + でつないで出す。修飾なしは今の文字列のまま (D-137)
    #[test]
    fn action_summary_shows_click_modifiers() {
        use crate::script::{Button, ClickModifier};
        let click = |modifiers: Vec<ClickModifier>| Action::Click {
            x: 1,
            y: 2,
            button: Button::Left,
            modifiers,
        };
        assert_eq!(
            action_summary(&click(vec![ClickModifier::Shift, ClickModifier::Ctrl])),
            "click (1, 2) left ctrl+shift"
        );
        assert_eq!(action_summary(&click(vec![])), "click (1, 2) left");
        assert_eq!(
            action_summary(&Action::DoubleClick {
                x: 3,
                y: 4,
                button: Button::Right,
                modifiers: vec![ClickModifier::Alt],
            }),
            "double_click (3, 4) right alt"
        );
    }

    #[test]
    fn tui_action_summary_covers_every_kind() {
        let s =
            crate::script::Script::from_json(&crate::cli::schema()["example"].to_string()).unwrap();
        for action in &s.actions {
            let line = action_summary(action);
            assert!(line.contains(action.kind().name()), "{line}");
        }
        // example に無い種類 (drag、scroll、double_click、move、close、wait_for) も 1 行要約に種類の名前が出る
        let drag = crate::script::Script::from_json(
            r#"{"version":1,"actions":[{"type":"drag","from":{"x":1,"y":2},"to":{"x":3,"y":4}},{"type":"scroll","clicks":-2},{"type":"double_click","x":5,"y":6},{"type":"move","x":7,"y":8},{"type":"close","process":"notepad.exe","title":"メモ"},{"type":"wait_for","process":"notepad.exe","title":"メモ","element":{"name":"保存","type":"button"},"text":"保存しました","absent":true},{"type":"wait_for","process":"notepad.exe"}]}"#,
        )
        .unwrap();
        for action in &drag.actions {
            let line = action_summary(action);
            assert!(line.contains(action.kind().name()), "{line}");
        }
        assert!(action_summary(&drag.actions[0]).contains("(1, 2)"));
        assert!(action_summary(&drag.actions[1]).contains("-2"));
        // wait_for の要約: 題名、要素 (name/type)、文字列、absent の順。None の項目と absent=false は省く
        let full = action_summary(&drag.actions[5]);
        assert_eq!(
            full,
            "wait_for notepad.exe \"メモ\" element=保存/button text=\"保存しました\" absent"
        );
        assert_eq!(action_summary(&drag.actions[6]), "wait_for notepad.exe");
        // 画面の待ち (D-081) の要約: 撮る対象 (window、region、全画面) と主な値
        let screen = crate::script::Script::from_json(
            r#"{"version":1,"actions":[{"type":"wait_stable","window":{"process":"app.exe","title":"メモ"},"stable_ms":500},{"type":"wait_change","region":{"x":1,"y":2,"w":30,"h":40},"threshold":0.05},{"type":"wait_stable"}]}"#,
        )
        .unwrap();
        assert_eq!(
            action_summary(&screen.actions[0]),
            "wait_stable app.exe \"メモ\" 500 ms threshold=0"
        );
        assert_eq!(
            action_summary(&screen.actions[1]),
            "wait_change 1,2,30,40 threshold=0.05"
        );
        assert_eq!(
            action_summary(&screen.actions[2]),
            "wait_stable 全画面 300 ms threshold=0"
        );
    }

    #[test]
    fn tui_list_scripts_lists_only_json_files_in_the_directory() {
        let dir = crate::testing::unique_temp_dir("tui-scripts");
        for f in ["b.JSON", "a.json", "c.txt"] {
            std::fs::write(dir.join(f), "{}").unwrap();
        }
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("d.json"), "{}").unwrap();
        let list = list_scripts(&dir).unwrap();
        assert_eq!(
            list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["a.json", "b.JSON"]
        );
        assert_eq!(list[0].path, dir.join("a.json"));
        assert!(list_scripts(&dir.join("missing")).is_err());
    }

    #[test]
    fn tui_script_picker_renders_in_narrow_mode_and_help_mentions_run() {
        let mut a = app();
        a.set_scripts(Ok(scripts()));
        a.update(&press(KeyCode::Char('r')));
        let text = draw(&a, 60, 24);
        println!("{text}");
        assert!(has(&text, "操作列を実行") && has(&text, "a.json"), "{text}");
        let a = app();
        let text = draw(&a, 120, 32);
        assert!(has(&text, "r 実行"), "{text}");
        let mut a = app();
        a.update(&press(KeyCode::Char('?')));
        let text = draw(&a, 120, 32);
        assert!(has(&text, "操作列の実行"), "{text}");
    }

    #[test]
    fn tui_load_broken_file_offers_recovery_and_saves_with_backup() {
        let dir = crate::testing::unique_temp_dir("tui-broken");
        let f = dir.join("config.toml");
        std::fs::write(&f, "version = 1\n[general\n").unwrap();
        let mut a = App::load(Target {
            path: f.clone(),
            overlay: false,
        });
        assert!(a.replace_broken);
        assert!(a.status.contains(".bak"), "{}", a.status);
        assert_eq!(a.update(&press(KeyCode::Char('s'))), Effect::Save);
        config::save_with(&a.target.path, &a.config, a.replace_broken).unwrap();
        a.on_saved(Ok(()));
        assert_eq!(Config::load(&f).unwrap(), Config::default());
        let broken: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.broken-")
            })
            .collect();
        assert_eq!(broken.len(), 1);
        assert_eq!(
            std::fs::read_to_string(broken[0].path()).unwrap(),
            "version = 1\n[general\n"
        );
    }

    /// 拒否の rule (D-079) に入る設定のキーは、どれも TUI で項目を選んだとき、右の枠の先頭に灰色で出るキーにある。
    /// 設定のキーでない builtin と save_path は、その一覧に入れない
    #[test]
    fn config_rules_are_tui_field_keys() {
        let keys: Vec<String> = fields().into_iter().map(Field::key).collect();
        for rule in crate::policy::CONFIG_RULES {
            assert!(keys.iter().any(|k| k == rule), "{rule} が TUI の項目に無い");
        }
        for fixed in [crate::policy::RULE_BUILTIN, crate::policy::RULE_SAVE_PATH] {
            assert!(!crate::policy::CONFIG_RULES.contains(&fixed), "{fixed}");
            assert!(!keys.iter().any(|k| k == fixed), "{fixed}");
        }
    }

    /// 床の fix に添える表示名 (`policy::tui_label`) は、そのキーの項目の左の一覧での場所と一字一句同じ (D-080)
    #[test]
    fn config_rules_tui_labels_match_field_locations() {
        for rule in crate::policy::CONFIG_RULES {
            let f = fields()
                .into_iter()
                .find(|f| f.key() == rule)
                .unwrap_or_else(|| panic!("{rule} が TUI の項目に無い"));
            assert_eq!(
                Some(f.location().as_str()),
                crate::policy::tui_label(rule),
                "{rule}"
            );
        }
    }
}
