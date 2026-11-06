//! 操作列 JSON の書式 (D-030)。`{"version": 1, "actions": [...]}`。未知のフィールド・種類・版は拒否する。

use serde::{Deserialize, Serialize};

use crate::geometry::{Point, Rect};
use crate::keys::KeyCombo;

pub const SCRIPT_VERSION: u32 = 1;
pub const MAX_DRAG_STEPS: u32 = 1000;
pub const MAX_SINGLE_WAIT_MS: u64 = 600_000;
pub const MAX_DRAG_DURATION_MS: u64 = 60_000;
pub const MAX_SCROLL_CLICKS: i32 = 100;
/// MCP で `path` の無い撮影に使う仮のパス。ディスクには書かず、結果の `image` にだけ入れる。
/// 保存先の規則 (D-075) は判定しない。検証 (`validate`) が `mcp` に依らないよう、ここに置く
pub const INLINE_PATH: &str = "mcp:inline";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub version: u32,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

impl From<Pos> for Point {
    fn from(p: Pos) -> Self {
        Point::new(p.x, p.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl From<Region> for Rect {
    fn from(r: Region) -> Self {
        Rect::new(r.x, r.y, r.w, r.h)
    }
}

pub const DEFAULT_DRAG_STEPS: u32 = 20;
pub const DEFAULT_DRAG_MS: u64 = 300;

fn default_steps() -> u32 {
    DEFAULT_DRAG_STEPS
}

fn default_drag_ms() -> u64 {
    DEFAULT_DRAG_MS
}

pub const DEFAULT_TYPE_INTERVAL_MS: u64 = 20;
pub const MAX_TYPE_INTERVAL_MS: u64 = 1000;
pub const DEFAULT_SHOT_SCALE: f64 = 1.0;

fn default_scale() -> f64 {
    DEFAULT_SHOT_SCALE
}

/// 撮る対象のウィンドウ (D-053)。`focus` と同じ探し方 (実行ファイル名の正規化、題名の部分一致、前面に近いもの)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowTarget {
    /// 実行ファイル名 (例 `notepad.exe`。大文字小文字と `.exe` の有無は問わない)
    pub process: String,
    /// 題名に含む文字列 (大文字小文字を無視)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

fn default_type_interval() -> u64 {
    DEFAULT_TYPE_INTERVAL_MS
}

/// `wait_for` の `timeout_ms` の既定 (D-068)。起動や描画の待ちに足り、MCP のクライアントの制限時間にも収まる
pub const DEFAULT_WAIT_FOR_TIMEOUT_MS: u64 = 10_000;

fn default_wait_for_timeout() -> u64 {
    DEFAULT_WAIT_FOR_TIMEOUT_MS
}

/// 画面の待ち (`wait_stable` と screenshot の settle) の `stable_ms` の既定 (D-081)。一手ずつの settle の既定と同じ長さ
pub const DEFAULT_STABLE_MS: u64 = 300;
/// 画面の待ちの `stable_ms` の下限 (D-081)。撮る間隔 (100 ms) より短い静けさは確かめられない
pub const MIN_STABLE_MS: u64 = 100;
/// 画面の待ちの `stable_ms` の上限 (D-081)
pub const MAX_STABLE_MS: u64 = 60_000;
/// `wait_change` の `threshold` の既定 (D-081)。基準と比べるので、キャレットの点滅 1 回では満たさない
pub const DEFAULT_CHANGE_THRESHOLD: f64 = 0.001;

/// screenshot の settle の `timeout_ms` の既定 (D-081)。落ち着かなくても最後の画像で撮る上限
pub const DEFAULT_SETTLE_TIMEOUT_MS: u64 = 3000;

fn default_stable_ms() -> u64 {
    DEFAULT_STABLE_MS
}

fn default_settle_timeout() -> u64 {
    DEFAULT_SETTLE_TIMEOUT_MS
}

/// screenshot の撮る前の落ち着きの待ち (D-081)。`wait_stable` と同じ判定で待ち、落ち着いたときか時間切れのときの
/// 最後のフレームを保存する (時間切れでも失敗にしない)。一手ずつの `settle_ms: "auto"` はこれを既定値で付ける
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settle {
    /// 静けさが続く長さ (既定 300、100 以上 60000 以下)
    #[serde(default = "default_stable_ms")]
    pub stable_ms: u64,
    /// 変わった点の割合の許容 (0 以上 1 未満。既定 0)
    #[serde(default)]
    pub threshold: f64,
    /// 待つ上限 (既定 3000、`stable_ms` 以上 600000 以下)
    #[serde(default = "default_settle_timeout")]
    pub timeout_ms: u64,
}

impl Default for Settle {
    fn default() -> Self {
        Settle {
            stable_ms: DEFAULT_STABLE_MS,
            threshold: 0.0,
            timeout_ms: DEFAULT_SETTLE_TIMEOUT_MS,
        }
    }
}

fn default_change_threshold() -> f64 {
    DEFAULT_CHANGE_THRESHOLD
}

/// `wait_for` の要素の絞り込み (D-068)。`name` は部分一致 (大文字小文字を無視)、`type` は種類の名前
/// (`elements::parse_control_type` で正規化)。どちらか一方は要る
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub control_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Click {
        x: i32,
        y: i32,
        #[serde(default)]
        button: Button,
    },
    DoubleClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: Button,
    },
    Drag {
        from: Pos,
        to: Pos,
        #[serde(default)]
        button: Button,
        /// 始点と終点の間の補間の回数
        #[serde(default = "default_steps")]
        steps: u32,
        /// ドラッグ全体にかける時間
        #[serde(default = "default_drag_ms")]
        duration_ms: u64,
    },
    Move {
        x: i32,
        y: i32,
    },
    Scroll {
        /// 指定すると、先にその座標へ移動する
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        /// 正で上 (奥) または右。1 で 1 ノッチ (WHEEL_DELTA = 120)
        clicks: i32,
        #[serde(default)]
        horizontal: bool,
    },
    Type {
        text: String,
        /// 文字と文字の間の待ち (ミリ秒)。受け手が追いつけずに文字を落とさないよう、既定で間を空ける (D-040)
        #[serde(default = "default_type_interval")]
        interval_ms: u64,
    },
    Key {
        keys: String,
    },
    Wait {
        ms: u64,
    },
    Screenshot {
        path: String,
        /// 撮る矩形。`window` と排他。両方省略すると仮想デスクトップ全体
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Region>,
        /// 撮る対象のウィンドウ (D-053)。矩形は撮る直前に `focus` と同じ探し方で解く
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<WindowTarget>,
        /// 保存する画像の縮尺 (0 より大きく 1 以下。既定 1)。画像の点 (px, py) は画面の (x + px / scale, y + py / scale)
        #[serde(default = "default_scale")]
        scale: f64,
        /// 長辺の上限 (1 以上、物理ピクセル)。縮尺は scale と max_edge / 長辺 の小さい方 (D-072)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_edge: Option<u32>,
        /// 撮る前に画面が落ち着くのを待つ (D-081)。省略すると待たずに撮る
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settle: Option<Settle>,
    },
    /// 許可されたアプリのウィンドウを前面にする (D-050)。`SetForegroundWindow` を 1 回呼ぶだけで、最小化は復元しない
    Focus {
        /// 実行ファイル名 (例 `notepad.exe`。大文字小文字と `.exe` の有無は問わない)
        process: String,
        /// 題名に含む文字列 (大文字小文字を無視)。省略すると、そのプロセスの最前のウィンドウ
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// 許可されたアプリのウィンドウに `WM_CLOSE` を置いて閉じる (D-062)。前面は変えず、入力も送らない。
    /// 閉じるか (保存の確認を出すか) は相手のアプリが決める
    Close {
        /// 実行ファイル名 (例 `notepad.exe`。大文字小文字と `.exe` の有無は問わない)
        process: String,
        /// 題名に含む文字列 (大文字小文字を無視)。取り違えを防ぐため必須
        title: String,
    },
    /// 対象のウィンドウ、その要素、要素の文字列が出る (`absent` なら消える) まで待つ (D-068)。読むだけで何も送らない。
    /// 条件は一番細かい指定で決まる (text > element > window)。`absent` はその否定。
    /// 250 ms ごとに確かめ、`timeout_ms` のうちに満たされなければ終了コード 15
    WaitFor {
        /// 実行ファイル名 (`focus` と同じ探し方)
        process: String,
        /// 題名に含む文字列 (大文字小文字を無視)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// 要素の絞り込み。`text` があればその絞り込みの中で文字列を読む
        #[serde(default, skip_serializing_if = "Option::is_none")]
        element: Option<ElementFilter>,
        /// 要素の文字列に含む文字列 (改行は LF にそろえて部分一致。大文字小文字を区別)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// true なら「無くなるまで」待つ
        #[serde(default)]
        absent: bool,
        /// 待つ上限 (既定 10000、最大 600000)。0 は 1 回だけ確かめる
        #[serde(default = "default_wait_for_timeout")]
        timeout_ms: u64,
    },
    /// 画面が落ち着くまで待つ (D-081)。直前のフレームとの差が `threshold` 以下の比較が `stable_ms` 以上続けば満たす。
    /// 撮る対象は `screenshot` と同じ (`region` / `window`。両方省略は全画面)。100 ms ごとに撮り、時間切れは 15
    WaitStable {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Region>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<WindowTarget>,
        /// 静けさが続く長さ (既定 300、100 以上 60000 以下)
        #[serde(default = "default_stable_ms")]
        stable_ms: u64,
        /// 変わった点の割合の許容 (0 以上 1 未満。既定 0)
        #[serde(default)]
        threshold: f64,
        /// 待つ上限 (既定 10000、`stable_ms` 以上 600000 以下)
        #[serde(default = "default_wait_for_timeout")]
        timeout_ms: u64,
    },
    /// 画面が変わるまで待つ (D-081)。待ち始めの最初のフレーム (基準) との差が `threshold` を超えれば満たす。
    /// 撮る対象は `screenshot` と同じ。100 ms ごとに撮り、時間切れは 15
    WaitChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Region>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<WindowTarget>,
        /// 変わった点の割合の下限 (これを超えたら変わった。0 以上 1 未満。既定 0.001)
        #[serde(default = "default_change_threshold")]
        threshold: f64,
        /// 待つ上限 (既定 10000、1 以上 600000 以下)
        #[serde(default = "default_wait_for_timeout")]
        timeout_ms: u64,
    },
}

/// 操作の種類。設定の `actions` リストで使う名前 (D-028)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActionKind {
    Click,
    DoubleClick,
    Drag,
    Move,
    Scroll,
    Type,
    Key,
    Wait,
    Screenshot,
    Focus,
    Close,
    WaitFor,
    WaitStable,
    WaitChange,
}

impl ActionKind {
    pub const ALL: [ActionKind; 14] = [
        ActionKind::Click,
        ActionKind::DoubleClick,
        ActionKind::Drag,
        ActionKind::Move,
        ActionKind::Scroll,
        ActionKind::Type,
        ActionKind::Key,
        ActionKind::Wait,
        ActionKind::Screenshot,
        ActionKind::Focus,
        ActionKind::Close,
        ActionKind::WaitFor,
        ActionKind::WaitStable,
        ActionKind::WaitChange,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ActionKind::Click => "click",
            ActionKind::DoubleClick => "double_click",
            ActionKind::Drag => "drag",
            ActionKind::Move => "move",
            ActionKind::Scroll => "scroll",
            ActionKind::Type => "type",
            ActionKind::Key => "key",
            ActionKind::Wait => "wait",
            ActionKind::Screenshot => "screenshot",
            ActionKind::Focus => "focus",
            ActionKind::Close => "close",
            ActionKind::WaitFor => "wait_for",
            ActionKind::WaitStable => "wait_stable",
            ActionKind::WaitChange => "wait_change",
        }
    }

    /// 大文字小文字と `-` / `_` の違いを無視して解析する。
    pub fn parse(s: &str) -> Option<ActionKind> {
        let norm = s.trim().to_ascii_lowercase().replace('-', "_");
        let norm = if norm == "doubleclick" || norm == "dblclick" {
            "double_click".to_string()
        } else {
            norm
        };
        ActionKind::ALL.into_iter().find(|k| k.name() == norm)
    }

    /// 本物のマウスかキーボードを動かすか (`focus` は前面を変え、`close` はウィンドウを閉じるが、入力は送らない。
    /// `wait_for` は読むだけ。`wait_stable` と `wait_change` は撮るだけ)。
    pub fn is_input(self) -> bool {
        !matches!(
            self,
            ActionKind::Wait
                | ActionKind::Screenshot
                | ActionKind::Focus
                | ActionKind::Close
                | ActionKind::WaitFor
                | ActionKind::WaitStable
                | ActionKind::WaitChange
        )
    }
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Action::Click { .. } => ActionKind::Click,
            Action::DoubleClick { .. } => ActionKind::DoubleClick,
            Action::Drag { .. } => ActionKind::Drag,
            Action::Move { .. } => ActionKind::Move,
            Action::Scroll { .. } => ActionKind::Scroll,
            Action::Type { .. } => ActionKind::Type,
            Action::Key { .. } => ActionKind::Key,
            Action::Wait { .. } => ActionKind::Wait,
            Action::Screenshot { .. } => ActionKind::Screenshot,
            Action::Focus { .. } => ActionKind::Focus,
            Action::Close { .. } => ActionKind::Close,
            Action::WaitFor { .. } => ActionKind::WaitFor,
            Action::WaitStable { .. } => ActionKind::WaitStable,
            Action::WaitChange { .. } => ActionKind::WaitChange,
        }
    }

    /// マウスが触れる点 (ドラッグは始点と終点。補間の点は直線上なので、領域の判定は計画側で全点を見る)。
    pub fn mouse_points(&self) -> Vec<Point> {
        match self {
            Action::Click { x, y, .. }
            | Action::DoubleClick { x, y, .. }
            | Action::Move { x, y } => {
                vec![Point::new(*x, *y)]
            }
            Action::Drag { from, to, .. } => vec![(*from).into(), (*to).into()],
            Action::Scroll {
                x: Some(x),
                y: Some(y),
                ..
            } => vec![Point::new(*x, *y)],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// JSON として読めない、または書式に合わない (未知のフィールドや種類を含む)
    Parse(String),
    /// 版が未知
    Version(u32),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::Parse(m) => write!(f, "操作列の JSON を読めない: {m}"),
            ScriptError::Version(v) => {
                write!(
                    f,
                    "操作列の version {v} は未対応 (対応しているのは {SCRIPT_VERSION})"
                )
            }
        }
    }
}

impl Script {
    pub fn from_json(text: &str) -> Result<Script, ScriptError> {
        // 版を先に見る。版が違うと他のフィールドの意味も違いうるので、書式エラーより版エラーを優先する
        let raw: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| ScriptError::Parse(e.to_string()))?;
        match raw.get("version").and_then(serde_json::Value::as_u64) {
            Some(v) if v == u64::from(SCRIPT_VERSION) => {}
            Some(v) => return Err(ScriptError::Version(u32::try_from(v).unwrap_or(u32::MAX))),
            None => return Err(ScriptError::Parse("version (数値) が必要".into())),
        }
        serde_json::from_value(raw).map_err(|e| ScriptError::Parse(e.to_string()))
    }
}

/// 設定に依存しない、個々の操作の形の検査。問題があれば説明を返す。
pub fn shape_issue(action: &Action) -> Option<String> {
    match action {
        Action::Drag {
            steps, duration_ms, ..
        } => {
            if *steps == 0 || *steps > MAX_DRAG_STEPS {
                return Some(format!("drag の steps は 1 から {MAX_DRAG_STEPS}"));
            }
            if *duration_ms > MAX_DRAG_DURATION_MS {
                return Some(format!(
                    "drag の duration_ms は {MAX_DRAG_DURATION_MS} 以下"
                ));
            }
            None
        }
        Action::Scroll { x, y, clicks, .. } => {
            if x.is_some() != y.is_some() {
                return Some("scroll の x と y は両方指定するか、両方省略する".into());
            }
            // abs() は i32::MIN で溢れる (release では負のまま返り検査を抜ける)
            if *clicks == 0 || clicks.unsigned_abs() > MAX_SCROLL_CLICKS.unsigned_abs() {
                return Some(format!(
                    "scroll の clicks は 0 以外で、絶対値 {MAX_SCROLL_CLICKS} 以下"
                ));
            }
            None
        }
        Action::Type { text, .. } if text.is_empty() => Some("type の text が空".into()),
        Action::Type { interval_ms, .. } if *interval_ms > MAX_TYPE_INTERVAL_MS => Some(format!(
            "type の interval_ms は {MAX_TYPE_INTERVAL_MS} 以下"
        )),
        Action::Type { text, .. }
            if text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) =>
        {
            Some("type の text に改行とタブ以外の制御文字は使えない".into())
        }
        Action::Key { keys } => keys.parse::<KeyCombo>().err(),
        Action::Wait { ms } if *ms > MAX_SINGLE_WAIT_MS => {
            Some(format!("wait の ms は {MAX_SINGLE_WAIT_MS} 以下"))
        }
        Action::Screenshot {
            path,
            region,
            window,
            scale,
            max_edge,
            settle,
        } => {
            if path.trim().is_empty() {
                return Some("screenshot の path が空".into());
            }
            if let Some(m) = capture_target_issue("screenshot", region.as_ref(), window.as_ref()) {
                return Some(m);
            }
            if !(scale.is_finite() && *scale > 0.0 && *scale <= 1.0) {
                return Some("screenshot の scale は 0 より大きく 1 以下".into());
            }
            if *max_edge == Some(0) {
                return Some("screenshot の max_edge は 1 以上".into());
            }
            settle.as_ref().and_then(|s| {
                screen_wait_issue(
                    "screenshot の settle",
                    Some(s.stable_ms),
                    s.threshold,
                    s.timeout_ms,
                )
            })
        }
        Action::Focus { process, title } => {
            if process.trim().is_empty() {
                return Some("focus の process が空".into());
            }
            if title.as_deref().is_some_and(|t| t.trim().is_empty()) {
                return Some("focus の title は省略するか、空でない文字列にする".into());
            }
            None
        }
        Action::Close { process, title } => {
            if process.trim().is_empty() {
                return Some("close の process が空".into());
            }
            if title.trim().is_empty() {
                return Some(
                    "close の title が空 (取り違えを防ぐため必須。windows で題名を確かめる)".into(),
                );
            }
            None
        }
        Action::WaitFor {
            process,
            title,
            element,
            text,
            timeout_ms,
            ..
        } => {
            if process.trim().is_empty() {
                return Some("wait_for の process が空".into());
            }
            if title.as_deref().is_some_and(|t| t.trim().is_empty()) {
                return Some("wait_for の title は省略するか、空でない文字列にする".into());
            }
            if let Some(f) = element {
                if f.name.is_none() && f.control_type.is_none() {
                    return Some("wait_for の element には name か type のどちらかが要る".into());
                }
                if let Some(t) = &f.control_type
                    && crate::elements::parse_control_type(t).is_none()
                {
                    return Some(format!(
                        "wait_for の element.type が未知: {t:?} (種類の名前は ai-cu find --help)"
                    ));
                }
                if f.name.as_deref().is_some_and(|n| n.trim().is_empty()) {
                    return Some(
                        "wait_for の element.name は省略するか、空でない文字列にする".into(),
                    );
                }
            }
            if text.as_deref().is_some_and(|t| t.is_empty()) {
                return Some("wait_for の text は省略するか、空でない文字列にする".into());
            }
            if *timeout_ms > MAX_SINGLE_WAIT_MS {
                return Some(format!(
                    "wait_for の timeout_ms は {MAX_SINGLE_WAIT_MS} 以下"
                ));
            }
            None
        }
        Action::WaitStable {
            region,
            window,
            stable_ms,
            threshold,
            timeout_ms,
        } => capture_target_issue("wait_stable", region.as_ref(), window.as_ref()).or_else(|| {
            screen_wait_issue("wait_stable", Some(*stable_ms), *threshold, *timeout_ms)
        }),
        Action::WaitChange {
            region,
            window,
            threshold,
            timeout_ms,
        } => capture_target_issue("wait_change", region.as_ref(), window.as_ref())
            .or_else(|| screen_wait_issue("wait_change", None, *threshold, *timeout_ms)),
        _ => None,
    }
}

/// 撮る対象 (`region` / `window`) の形の検査。`screenshot` と画面の待ち (D-081) が共有する。
/// `name` は文の主語にする操作の名前 (screenshot の文は D-081 の前と同じ)
fn capture_target_issue(
    name: &str,
    region: Option<&Region>,
    window: Option<&WindowTarget>,
) -> Option<String> {
    if let Some(r) = region
        && (r.w == 0 || r.h == 0)
    {
        return Some(format!("{name} の region の幅と高さは 1 以上"));
    }
    if region.is_some() && window.is_some() {
        return Some(format!("{name} の region と window はどちらか一方だけ"));
    }
    if let Some(w) = window {
        if w.process.trim().is_empty() {
            return Some(format!("{name} の window.process が空"));
        }
        if w.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
            return Some(format!(
                "{name} の window.title は省略するか、空でない文字列にする"
            ));
        }
    }
    None
}

/// 画面の待ち (D-081) の値の範囲の検査。`wait_stable` と screenshot の settle は `stable_ms` に Some を、
/// `wait_change` は None を渡す。`name` は文の主語 (範囲の文は主語のほかは同じ)
fn screen_wait_issue(
    name: &str,
    stable_ms: Option<u64>,
    threshold: f64,
    timeout_ms: u64,
) -> Option<String> {
    if let Some(s) = stable_ms
        && !(MIN_STABLE_MS..=MAX_STABLE_MS).contains(&s)
    {
        return Some(format!(
            "{name} の stable_ms は {MIN_STABLE_MS} 以上 {MAX_STABLE_MS} 以下"
        ));
    }
    // NaN もここで断る (比較がすべて偽になるので範囲に入らない)
    if !(0.0..1.0).contains(&threshold) {
        return Some(format!("{name} の threshold は 0 以上 1 未満"));
    }
    if timeout_ms > MAX_SINGLE_WAIT_MS {
        return Some(format!("{name} の timeout_ms は {MAX_SINGLE_WAIT_MS} 以下"));
    }
    match stable_ms {
        Some(s) if timeout_ms < s => Some(format!(
            "{name} の timeout_ms は stable_ms ({s}) 以上 (短いと落ち着きを確かめられない)"
        )),
        None if timeout_ms == 0 => Some(format!(
            "{name} の timeout_ms は 1 以上 (0 では基準を撮るだけで必ず時間切れになる)"
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_parses_all_action_types() {
        let json = r#"{"version":1,"actions":[
            {"type":"click","x":10,"y":20},
            {"type":"double_click","x":1,"y":2,"button":"right"},
            {"type":"drag","from":{"x":0,"y":0},"to":{"x":5,"y":5},"steps":3},
            {"type":"move","x":3,"y":4},
            {"type":"scroll","clicks":-2},
            {"type":"type","text":"こんにちは\n"},
            {"type":"key","keys":"ctrl+s"},
            {"type":"wait","ms":100},
            {"type":"screenshot","path":"target/a.png","region":{"x":0,"y":0,"w":10,"h":10}}
        ]}"#;
        let s = Script::from_json(json).unwrap();
        assert_eq!(s.actions.len(), 9);
        assert!(matches!(
            &s.actions[5],
            Action::Type {
                interval_ms: DEFAULT_TYPE_INTERVAL_MS,
                ..
            }
        ));
        assert_eq!(
            s.actions[1],
            Action::DoubleClick {
                x: 1,
                y: 2,
                button: Button::Right
            }
        );
        match &s.actions[2] {
            Action::Drag {
                duration_ms, steps, ..
            } => {
                assert_eq!(*duration_ms, 300);
                assert_eq!(*steps, 3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn script_rejects_unknown_fields_types_and_versions() {
        let unknown_field = r#"{"version":1,"actions":[{"type":"click","x":1,"y":2,"z":3}]}"#;
        assert!(matches!(
            Script::from_json(unknown_field),
            Err(ScriptError::Parse(_))
        ));
        let unknown_type = r#"{"version":1,"actions":[{"type":"teleport","x":1}]}"#;
        assert!(matches!(
            Script::from_json(unknown_type),
            Err(ScriptError::Parse(_))
        ));
        let top_level = r#"{"version":1,"actions":[],"extra":true}"#;
        assert!(matches!(
            Script::from_json(top_level),
            Err(ScriptError::Parse(_))
        ));
        let version = r#"{"version":2,"actions":[]}"#;
        assert_eq!(Script::from_json(version), Err(ScriptError::Version(2)));
        assert!(matches!(
            Script::from_json(r#"{"actions":[]}"#),
            Err(ScriptError::Parse(_))
        ));
        assert!(matches!(
            Script::from_json("not json"),
            Err(ScriptError::Parse(_))
        ));
        let nested = r#"{"version":1,"actions":[{"type":"drag","from":{"x":0,"y":0,"q":1},"to":{"x":1,"y":1}}]}"#;
        assert!(matches!(
            Script::from_json(nested),
            Err(ScriptError::Parse(_))
        ));
    }

    /// focus (D-050): process は必須、title は省略可。空の process と空の title は形の誤り
    #[test]
    fn script_parses_focus_and_rejects_empty_target() {
        let s = Script::from_json(
            r#"{"version":1,"actions":[{"type":"focus","process":"notepad.exe"},{"type":"focus","process":"Notepad.exe","title":"メモ"}]}"#,
        )
        .unwrap();
        assert_eq!(
            s.actions[0],
            Action::Focus {
                process: "notepad.exe".into(),
                title: None
            }
        );
        assert_eq!(s.actions[0].kind(), ActionKind::Focus);
        assert_eq!(ActionKind::parse("Focus"), Some(ActionKind::Focus));
        assert!(!ActionKind::Focus.is_input(), "入力は送らない");
        assert!(s.actions[0].mouse_points().is_empty());
        match &s.actions[1] {
            Action::Focus { title, .. } => assert_eq!(title.as_deref(), Some("メモ")),
            other => panic!("{other:?}"),
        }
        for a in [
            Action::Focus {
                process: " ".into(),
                title: None,
            },
            Action::Focus {
                process: "notepad".into(),
                title: Some("".into()),
            },
        ] {
            assert!(shape_issue(&a).is_some(), "{a:?}");
        }
        assert!(
            Script::from_json(
                r#"{"version":1,"actions":[{"type":"focus","process":"a","hwnd":1}]}"#
            )
            .is_err(),
            "未知のフィールドは拒否"
        );
    }

    /// screenshot の window (D-053): region と排他。process は必須、title は省略可。scale は 0 より大きく 1 以下
    #[test]
    fn script_parses_screenshot_window_and_scale_and_rejects_bad_shapes() {
        let s = Script::from_json(
            r#"{"version":1,"actions":[{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe"}},{"type":"screenshot","path":"b.png","window":{"process":"Notepad","title":"メモ"},"scale":0.5}]}"#,
        )
        .unwrap();
        assert_eq!(
            s.actions[0],
            Action::Screenshot {
                path: "a.png".into(),
                region: None,
                window: Some(WindowTarget {
                    process: "notepad.exe".into(),
                    title: None
                }),
                scale: 1.0,
                max_edge: None,
                settle: None,
            }
        );
        assert!(shape_issue(&s.actions[0]).is_none());
        match &s.actions[1] {
            Action::Screenshot {
                window: Some(w),
                scale,
                ..
            } => {
                assert_eq!(w.title.as_deref(), Some("メモ"));
                assert_eq!(*scale, 0.5);
            }
            other => panic!("{other:?}"),
        }
        let shot =
            |region: Option<Region>, window: Option<WindowTarget>, scale: f64| Action::Screenshot {
                path: "a.png".into(),
                region,
                window,
                scale,
                max_edge: None,
                settle: None,
            };
        let target = |p: &str, t: Option<&str>| WindowTarget {
            process: p.into(),
            title: t.map(Into::into),
        };
        let both = shot(
            Some(Region {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            }),
            Some(target("n", None)),
            1.0,
        );
        assert!(shape_issue(&both).unwrap().contains("window"));
        for bad in [
            shot(None, Some(target(" ", None)), 1.0),
            shot(None, Some(target("n", Some(""))), 1.0),
            shot(None, None, 0.0),
            shot(None, None, 1.5),
            shot(None, None, f64::NAN),
        ] {
            assert!(shape_issue(&bad).is_some(), "{bad:?}");
        }
        assert!(
            Script::from_json(
                r#"{"version":1,"actions":[{"type":"screenshot","path":"a.png","window":{"process":"n","hwnd":1}}]}"#
            )
            .is_err(),
            "未知のフィールドは拒否"
        );
    }

    /// screenshot の max_edge (D-072): 書けば読み、書かなければ None。0 は形の誤り。null は省略と同じ
    #[test]
    fn script_parses_screenshot_max_edge_and_rejects_zero() {
        let s = Script::from_json(
            r#"{"version":1,"actions":[{"type":"screenshot","path":"a.png","max_edge":1568},{"type":"screenshot","path":"b.png"},{"type":"screenshot","path":"c.png","max_edge":0},{"type":"screenshot","path":"d.png","max_edge":null}]}"#,
        )
        .unwrap();
        let edge = |a: &Action| match a {
            Action::Screenshot { max_edge, .. } => *max_edge,
            other => panic!("{other:?}"),
        };
        assert_eq!(edge(&s.actions[0]), Some(1568));
        assert!(shape_issue(&s.actions[0]).is_none());
        assert_eq!(edge(&s.actions[1]), None, "書かないと None");
        assert_eq!(edge(&s.actions[2]), Some(0));
        assert_eq!(
            shape_issue(&s.actions[2]).as_deref(),
            Some("screenshot の max_edge は 1 以上")
        );
        assert_eq!(edge(&s.actions[3]), None, "null は省略と同じ");
        assert!(
            Script::from_json(
                r#"{"version":1,"actions":[{"type":"screenshot","path":"a.png","max_edge":-1}]}"#
            )
            .is_err(),
            "負の数は型の誤り"
        );
    }

    /// wait_for (D-068): 全フィールドつきと最小 (process だけ) の解析、既定値 (absent false、timeout_ms 10000)、
    /// 種類の名前の解析 (wait-for)、入力ではない、点を持たない
    #[test]
    fn script_parses_wait_for_with_defaults_and_full_fields() {
        let s = Script::from_json(
            r#"{"version":1,"actions":[
                {"type":"wait_for","process":"notepad.exe"},
                {"type":"wait_for","process":"Notepad.exe","title":"メモ","element":{"name":"保存","type":"button"},"text":"保存しました","absent":true,"timeout_ms":500}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            s.actions[0],
            Action::WaitFor {
                process: "notepad.exe".into(),
                title: None,
                element: None,
                text: None,
                absent: false,
                timeout_ms: DEFAULT_WAIT_FOR_TIMEOUT_MS,
            }
        );
        assert_eq!(DEFAULT_WAIT_FOR_TIMEOUT_MS, 10_000);
        assert_eq!(
            s.actions[1],
            Action::WaitFor {
                process: "Notepad.exe".into(),
                title: Some("メモ".into()),
                element: Some(ElementFilter {
                    name: Some("保存".into()),
                    control_type: Some("button".into()),
                }),
                text: Some("保存しました".into()),
                absent: true,
                timeout_ms: 500,
            }
        );
        for a in &s.actions {
            assert!(shape_issue(a).is_none(), "{a:?}");
            assert_eq!(a.kind(), ActionKind::WaitFor);
            assert!(a.mouse_points().is_empty());
        }
        assert_eq!(ActionKind::WaitFor.name(), "wait_for");
        assert_eq!(ActionKind::parse("wait-for"), Some(ActionKind::WaitFor));
        assert_eq!(ActionKind::parse("WAIT_FOR"), Some(ActionKind::WaitFor));
        assert!(!ActionKind::WaitFor.is_input(), "読むだけで何も送らない");
        assert_eq!(ActionKind::ALL.len(), 14);
        // 出力 (serialize) では省いた項目を出さず、名前が type に戻る
        let v = serde_json::to_value(&s.actions[0]).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type":"wait_for","process":"notepad.exe","absent":false,"timeout_ms":10000})
        );
        let v = serde_json::to_value(&s.actions[1]).unwrap();
        assert_eq!(
            v["element"],
            serde_json::json!({"name":"保存","type":"button"})
        );
    }

    /// wait_for の形の誤り (D-068): 空の process / title / text、空の element、未知の type、空の name、
    /// timeout_ms の上限超え。未知のフィールド (本体と element の中) は読めない
    #[test]
    fn script_wait_for_rejects_bad_shapes_and_unknown_fields() {
        let base = Action::WaitFor {
            process: "notepad.exe".into(),
            title: None,
            element: None,
            text: None,
            absent: false,
            timeout_ms: 100,
        };
        let with = |f: &dyn Fn(&mut Action)| {
            let mut a = base.clone();
            f(&mut a);
            a
        };
        let filter = |name: Option<&str>, ty: Option<&str>| {
            Some(ElementFilter {
                name: name.map(Into::into),
                control_type: ty.map(Into::into),
            })
        };
        let bad: Vec<(Action, &str)> = vec![
            (
                with(&|a| {
                    if let Action::WaitFor { process, .. } = a {
                        *process = " ".into();
                    }
                }),
                "process が空",
            ),
            (
                with(&|a| {
                    if let Action::WaitFor { title, .. } = a {
                        *title = Some("".into());
                    }
                }),
                "title は省略するか",
            ),
            (
                with(&|a| {
                    if let Action::WaitFor { element, .. } = a {
                        *element = filter(None, None);
                    }
                }),
                "name か type のどちらか",
            ),
            (
                with(&|a| {
                    if let Action::WaitFor { element, .. } = a {
                        *element = filter(None, Some("bogus"));
                    }
                }),
                "element.type が未知",
            ),
            (
                with(&|a| {
                    if let Action::WaitFor { element, .. } = a {
                        *element = filter(Some(" "), None);
                    }
                }),
                "element.name は省略するか",
            ),
            (
                with(&|a| {
                    if let Action::WaitFor { text, .. } = a {
                        *text = Some("".into());
                    }
                }),
                "text は省略するか",
            ),
            (
                with(&|a| {
                    if let Action::WaitFor { timeout_ms, .. } = a {
                        *timeout_ms = MAX_SINGLE_WAIT_MS + 1;
                    }
                }),
                "timeout_ms は 600000 以下",
            ),
        ];
        for (a, expect) in &bad {
            let issue = shape_issue(a).unwrap_or_else(|| panic!("{a:?}"));
            assert!(issue.contains(expect), "{issue}");
            assert!(issue.starts_with("wait_for の"), "{issue}");
        }
        // 種類の名前は正規化して受ける。上限ちょうどは通る。0 も通る (1 回だけ確かめる)
        assert!(
            shape_issue(&with(&|a| {
                if let Action::WaitFor { element, .. } = a {
                    *element = filter(None, Some("List-Item"));
                }
            }))
            .is_none()
        );
        for ms in [0, MAX_SINGLE_WAIT_MS] {
            assert!(
                shape_issue(&with(&|a| {
                    if let Action::WaitFor { timeout_ms, .. } = a {
                        *timeout_ms = ms;
                    }
                }))
                .is_none()
            );
        }
        for json in [
            r#"{"version":1,"actions":[{"type":"wait_for","process":"a","hwnd":1}]}"#,
            r#"{"version":1,"actions":[{"type":"wait_for","process":"a","element":{"id":"x"}}]}"#,
            r#"{"version":1,"actions":[{"type":"wait_for"}]}"#,
        ] {
            assert!(
                matches!(Script::from_json(json), Err(ScriptError::Parse(_))),
                "{json}"
            );
        }
    }

    /// wait_stable と wait_change (D-081): 最小 (type だけ) と全フィールドつきの解析、既定値 (stable_ms 300、
    /// threshold は stable 0 / change 0.001、timeout_ms 10000)、種類の名前の解析 (wait-stable)、入力ではない、点を持たない
    #[test]
    fn script_parses_wait_stable_and_wait_change_with_defaults() {
        let s = Script::from_json(
            r#"{"version":1,"actions":[
                {"type":"wait_stable"},
                {"type":"wait_stable","window":{"process":"app.exe","title":"メモ"},"stable_ms":500,"threshold":0.01,"timeout_ms":2000},
                {"type":"wait_change"},
                {"type":"wait_change","region":{"x":0,"y":0,"w":800,"h":600},"threshold":0.05,"timeout_ms":1}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            s.actions[0],
            Action::WaitStable {
                region: None,
                window: None,
                stable_ms: 300,
                threshold: 0.0,
                timeout_ms: 10_000,
            }
        );
        assert_eq!(DEFAULT_STABLE_MS, 300);
        assert_eq!(
            s.actions[1],
            Action::WaitStable {
                region: None,
                window: Some(WindowTarget {
                    process: "app.exe".into(),
                    title: Some("メモ".into()),
                }),
                stable_ms: 500,
                threshold: 0.01,
                timeout_ms: 2000,
            }
        );
        assert_eq!(
            s.actions[2],
            Action::WaitChange {
                region: None,
                window: None,
                threshold: 0.001,
                timeout_ms: 10_000,
            }
        );
        assert_eq!(
            s.actions[3],
            Action::WaitChange {
                region: Some(Region {
                    x: 0,
                    y: 0,
                    w: 800,
                    h: 600
                }),
                window: None,
                threshold: 0.05,
                timeout_ms: 1,
            }
        );
        for a in &s.actions {
            assert!(shape_issue(a).is_none(), "{a:?}");
            assert!(a.mouse_points().is_empty());
        }
        assert_eq!(s.actions[0].kind(), ActionKind::WaitStable);
        assert_eq!(s.actions[2].kind(), ActionKind::WaitChange);
        assert_eq!(ActionKind::WaitStable.name(), "wait_stable");
        assert_eq!(ActionKind::WaitChange.name(), "wait_change");
        assert_eq!(
            ActionKind::parse("wait-stable"),
            Some(ActionKind::WaitStable)
        );
        assert_eq!(
            ActionKind::parse("Wait-Change"),
            Some(ActionKind::WaitChange)
        );
        assert!(!ActionKind::WaitStable.is_input(), "撮るだけで何も送らない");
        assert!(!ActionKind::WaitChange.is_input(), "撮るだけで何も送らない");
        assert_eq!(ActionKind::ALL[12], ActionKind::WaitStable);
        assert_eq!(ActionKind::ALL[13], ActionKind::WaitChange);
        // 出力 (serialize) では省いた region / window を出さない
        let v = serde_json::to_value(&s.actions[0]).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type":"wait_stable","stable_ms":300,"threshold":0.0,"timeout_ms":10000})
        );
    }

    /// 画面の待ちの形の誤り (D-081): region と window の両方、空の region と window、threshold の 1 と負と NaN、
    /// stable_ms の 99 と 60001、timeout_ms の上限超え、wait_stable の timeout_ms < stable_ms、wait_change の
    /// timeout_ms 0。範囲の文は主語のほかは 2 つの操作で同じ。未知のフィールドは読めない
    #[test]
    fn script_wait_screen_rejects_bad_shapes() {
        let stable = |region: Option<Region>,
                      window: Option<WindowTarget>,
                      stable_ms: u64,
                      threshold: f64,
                      timeout_ms: u64| Action::WaitStable {
            region,
            window,
            stable_ms,
            threshold,
            timeout_ms,
        };
        let change = |region: Option<Region>,
                      window: Option<WindowTarget>,
                      threshold: f64,
                      timeout_ms: u64| Action::WaitChange {
            region,
            window,
            threshold,
            timeout_ms,
        };
        let r = |w: u32| {
            Some(Region {
                x: 0,
                y: 0,
                w,
                h: 10,
            })
        };
        let win = |p: &str, t: Option<&str>| {
            Some(WindowTarget {
                process: p.into(),
                title: t.map(Into::into),
            })
        };
        let bad: Vec<(Action, &str)> = vec![
            (
                stable(r(10), win("a", None), 300, 0.0, 1000),
                "wait_stable の region と window はどちらか一方だけ",
            ),
            (
                change(r(10), win("a", None), 0.1, 1000),
                "wait_change の region と window はどちらか一方だけ",
            ),
            (
                stable(r(0), None, 300, 0.0, 1000),
                "wait_stable の region の幅と高さは 1 以上",
            ),
            (
                change(None, win(" ", None), 0.1, 1000),
                "wait_change の window.process が空",
            ),
            (
                stable(None, win("a", Some(" ")), 300, 0.0, 1000),
                "wait_stable の window.title は省略するか",
            ),
            (
                stable(None, None, 300, 1.0, 1000),
                "wait_stable の threshold は 0 以上 1 未満",
            ),
            (
                stable(None, None, 300, -0.1, 1000),
                "wait_stable の threshold は 0 以上 1 未満",
            ),
            (
                change(None, None, f64::NAN, 1000),
                "wait_change の threshold は 0 以上 1 未満",
            ),
            (
                change(None, None, 1.0, 1000),
                "wait_change の threshold は 0 以上 1 未満",
            ),
            (
                stable(None, None, 99, 0.0, 1000),
                "wait_stable の stable_ms は 100 以上 60000 以下",
            ),
            (
                stable(None, None, 60_001, 0.0, 600_000),
                "wait_stable の stable_ms は 100 以上 60000 以下",
            ),
            (
                stable(None, None, 300, 0.0, MAX_SINGLE_WAIT_MS + 1),
                "wait_stable の timeout_ms は 600000 以下",
            ),
            (
                change(None, None, 0.1, MAX_SINGLE_WAIT_MS + 1),
                "wait_change の timeout_ms は 600000 以下",
            ),
            (
                stable(None, None, 300, 0.0, 299),
                "wait_stable の timeout_ms は stable_ms (300) 以上",
            ),
            (
                change(None, None, 0.1, 0),
                "wait_change の timeout_ms は 1 以上",
            ),
        ];
        for (a, expect) in &bad {
            let issue = shape_issue(a).unwrap_or_else(|| panic!("{a:?}"));
            assert!(issue.starts_with(expect), "{issue} / {expect}");
        }
        // 範囲の文は主語のほかは同じ
        let tail = |a: &Action, name: &str| {
            shape_issue(a)
                .unwrap()
                .strip_prefix(name)
                .unwrap()
                .to_string()
        };
        assert_eq!(
            tail(&stable(None, None, 300, 1.0, 1000), "wait_stable"),
            tail(&change(None, None, 1.0, 1000), "wait_change")
        );
        assert_eq!(
            tail(&stable(None, None, 300, 0.0, 600_001), "wait_stable"),
            tail(&change(None, None, 0.1, 600_001), "wait_change")
        );
        // 境界ちょうどは通る
        for ok in [
            stable(None, None, MIN_STABLE_MS, 0.0, MIN_STABLE_MS),
            stable(None, None, MAX_STABLE_MS, 0.999, MAX_SINGLE_WAIT_MS),
            change(None, None, 0.0, 1),
            change(None, win("a", None), 0.5, MAX_SINGLE_WAIT_MS),
        ] {
            assert!(shape_issue(&ok).is_none(), "{ok:?}");
        }
        for json in [
            r#"{"version":1,"actions":[{"type":"wait_stable","hwnd":1}]}"#,
            r#"{"version":1,"actions":[{"type":"wait_change","stable_ms":300}]}"#,
            r#"{"version":1,"actions":[{"type":"wait_stable","window":{"process":"a","hwnd":1}}]}"#,
        ] {
            assert!(
                matches!(Script::from_json(json), Err(ScriptError::Parse(_))),
                "{json}"
            );
        }
    }

    /// screenshot の settle (D-081): `{}` は既定 (300 / 0 / 3000)、書いた値は読む。省略すると None で、書き出しにも出ない。
    /// timeout_ms 0 と stable_ms 未満、threshold 1、stable_ms の範囲外は形の誤りで、範囲の文は wait_stable と主語のほかは同じ。
    /// settle の中の未知のキーは解析の誤り
    #[test]
    fn script_parses_screenshot_settle_and_rejects_bad_values() {
        let s = Script::from_json(
            r#"{"version":1,"actions":[{"type":"screenshot","path":"a.png","settle":{}},{"type":"screenshot","path":"b.png","settle":{"stable_ms":500,"threshold":0.01,"timeout_ms":2000}},{"type":"screenshot","path":"c.png"}]}"#,
        )
        .unwrap();
        let settle = |a: &Action| match a {
            Action::Screenshot { settle, .. } => *settle,
            other => panic!("{other:?}"),
        };
        assert_eq!(settle(&s.actions[0]), Some(Settle::default()));
        assert_eq!(
            Settle::default(),
            Settle {
                stable_ms: DEFAULT_STABLE_MS,
                threshold: 0.0,
                timeout_ms: DEFAULT_SETTLE_TIMEOUT_MS,
            }
        );
        assert_eq!((DEFAULT_STABLE_MS, DEFAULT_SETTLE_TIMEOUT_MS), (300, 3000));
        assert_eq!(
            settle(&s.actions[1]),
            Some(Settle {
                stable_ms: 500,
                threshold: 0.01,
                timeout_ms: 2000,
            })
        );
        assert_eq!(settle(&s.actions[2]), None);
        for a in &s.actions {
            assert!(shape_issue(a).is_none(), "{a:?}");
        }
        let v = serde_json::to_value(&s.actions[2]).unwrap();
        assert!(v.get("settle").is_none(), "省略は書き出さない: {v}");
        let shot = |stable_ms: u64, threshold: f64, timeout_ms: u64| Action::Screenshot {
            path: "a.png".into(),
            region: None,
            window: None,
            scale: 1.0,
            max_edge: None,
            settle: Some(Settle {
                stable_ms,
                threshold,
                timeout_ms,
            }),
        };
        for (bad, expect) in [
            (
                shot(300, 0.0, 0),
                "screenshot の settle の timeout_ms は stable_ms (300) 以上",
            ),
            (
                shot(500, 0.0, 499),
                "screenshot の settle の timeout_ms は stable_ms (500) 以上",
            ),
            (
                shot(300, 1.0, 3000),
                "screenshot の settle の threshold は 0 以上 1 未満",
            ),
            (
                shot(300, f64::NAN, 3000),
                "screenshot の settle の threshold は 0 以上 1 未満",
            ),
            (
                shot(99, 0.0, 3000),
                "screenshot の settle の stable_ms は 100 以上 60000 以下",
            ),
            (
                shot(300, 0.0, MAX_SINGLE_WAIT_MS + 1),
                "screenshot の settle の timeout_ms は 600000 以下",
            ),
        ] {
            let issue = shape_issue(&bad).unwrap_or_else(|| panic!("{bad:?}"));
            assert!(issue.starts_with(expect), "{issue} / {expect}");
        }
        assert!(shape_issue(&shot(MIN_STABLE_MS, 0.999, MIN_STABLE_MS)).is_none());
        // 範囲の文は wait_stable と主語のほかは同じ
        let stable = |stable_ms: u64, threshold: f64, timeout_ms: u64| Action::WaitStable {
            region: None,
            window: None,
            stable_ms,
            threshold,
            timeout_ms,
        };
        for (s, t, to) in [(300, 1.0, 3000), (300, 0.0, 200), (99, 0.0, 3000)] {
            assert_eq!(
                shape_issue(&shot(s, t, to))
                    .unwrap()
                    .strip_prefix("screenshot の settle"),
                shape_issue(&stable(s, t, to))
                    .unwrap()
                    .strip_prefix("wait_stable"),
            );
        }
        assert!(
            matches!(
                Script::from_json(
                    r#"{"version":1,"actions":[{"type":"screenshot","path":"a.png","settle":{"ms":300}}]}"#
                ),
                Err(ScriptError::Parse(_))
            ),
            "settle の中の未知のキーは拒否"
        );
    }

    #[test]
    fn script_accepts_bom() {
        assert!(Script::from_json("\u{feff}{\"version\":1,\"actions\":[]}").is_ok());
    }

    #[test]
    fn action_kind_parse_is_case_insensitive() {
        assert_eq!(
            ActionKind::parse("Double-Click"),
            Some(ActionKind::DoubleClick)
        );
        assert_eq!(ActionKind::parse("TYPE"), Some(ActionKind::Type));
        assert_eq!(ActionKind::parse("teleport"), None);
    }

    /// clicks の上限は符号付きの両端で効く。i32::MIN は release (overflow-checks なし) で
    /// `abs()` が負のまま返り、検査を抜けて plan が約 21 億ノッチを展開していた
    #[test]
    fn scroll_clicks_bounds_hold_at_integer_extremes() {
        let scroll = |clicks| Action::Scroll {
            x: None,
            y: None,
            clicks,
            horizontal: false,
        };
        for clicks in [
            i32::MIN,
            i32::MIN + 1,
            i32::MAX,
            -(MAX_SCROLL_CLICKS + 1),
            MAX_SCROLL_CLICKS + 1,
        ] {
            assert!(shape_issue(&scroll(clicks)).is_some(), "{clicks}");
        }
        for clicks in [-MAX_SCROLL_CLICKS, -1, 1, MAX_SCROLL_CLICKS] {
            assert!(shape_issue(&scroll(clicks)).is_none(), "{clicks}");
        }
    }

    #[test]
    fn shape_issues() {
        let bad = [
            Action::Drag {
                from: Pos { x: 0, y: 0 },
                to: Pos { x: 1, y: 1 },
                button: Button::Left,
                steps: 0,
                duration_ms: 1,
            },
            Action::Scroll {
                x: Some(1),
                y: None,
                clicks: 1,
                horizontal: false,
            },
            Action::Scroll {
                x: None,
                y: None,
                clicks: 0,
                horizontal: false,
            },
            Action::Type {
                text: String::new(),
                interval_ms: 20,
            },
            Action::Type {
                text: "a\u{7}".into(),
                interval_ms: 20,
            },
            Action::Type {
                text: "a".into(),
                interval_ms: MAX_TYPE_INTERVAL_MS + 1,
            },
            Action::Key {
                keys: "ctrl+nope".into(),
            },
            Action::Wait {
                ms: MAX_SINGLE_WAIT_MS + 1,
            },
            Action::Screenshot {
                path: " ".into(),
                region: None,
                window: None,
                scale: 1.0,
                max_edge: None,
                settle: None,
            },
        ];
        for a in &bad {
            assert!(shape_issue(a).is_some(), "{a:?}");
        }
        assert!(
            shape_issue(&Action::Type {
                text: "a\tb\r\n".into(),
                interval_ms: 0,
            })
            .is_none()
        );
    }
}
