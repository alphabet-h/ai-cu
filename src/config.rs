//! 設定ファイル (TOML) の書式、読み込みと検証、場所の解決 (D-028、D-029)、差分を残す保存 (D-018)。

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::keys::KeyCombo;
use crate::script::ActionKind;

pub const CONFIG_VERSION: u32 = 1;
pub const ENV_CONFIG: &str = "AI_CU_CONFIG";
pub const ENV_DEV_FORBID: &str = "AI_CU_DEV_FORBID_REAL_INPUT";
pub const ENV_DEV_FLOOR: &str = "AI_CU_DEV_FLOOR";
pub const ENV_STOP_FILE: &str = "AI_CU_STOP_FILE";

// ---------------------------------------------------------------------------
// 書式
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub allow: Rules,
    #[serde(default)]
    pub deny: Rules,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub stop: Stop,
    #[serde(default)]
    pub capture: Capture,
    #[serde(default)]
    pub mcp: Mcp,
    #[serde(default)]
    pub indicator: IndicatorConfig,
    /// TUI の見た目 (D-129)。判定には関わらないので `Layer` には持たない
    #[serde(default)]
    pub tui: Tui,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            general: General::default(),
            allow: Rules::default(),
            deny: Rules::default(),
            limits: Limits::default(),
            stop: Stop::default(),
            capture: Capture::default(),
            mcp: Mcp::default(),
            indicator: IndicatorConfig::default(),
            tui: Tui::default(),
        }
    }
}

/// TUI の配色の選び方 (D-129)。`Auto` は端末と環境から決める (`NO_COLOR`、VT の可否)。
/// 並びは TUI の中で巡回する順 (`ALL`、`next`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    Auto,
    Calm,
    CalmLight,
    Ansi,
    Mono,
}

impl ThemeName {
    /// 巡回の順 (auto → calm → calm-light → ansi → mono)
    pub const ALL: [ThemeName; 5] = [
        ThemeName::Auto,
        ThemeName::Calm,
        ThemeName::CalmLight,
        ThemeName::Ansi,
        ThemeName::Mono,
    ];

    /// 設定に書く名前 (serde の名前と同じ)
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeName::Auto => "auto",
            ThemeName::Calm => "calm",
            ThemeName::CalmLight => "calm-light",
            ThemeName::Ansi => "ansi",
            ThemeName::Mono => "mono",
        }
    }

    /// 巡回で次のテーマ。最後の次は最初に戻る
    pub fn next(self) -> ThemeName {
        match self {
            ThemeName::Auto => ThemeName::Calm,
            ThemeName::Calm => ThemeName::CalmLight,
            ThemeName::CalmLight => ThemeName::Ansi,
            ThemeName::Ansi => ThemeName::Mono,
            ThemeName::Mono => ThemeName::Auto,
        }
    }
}

/// TUI の見た目の設定 (D-129)。許可と拒否の判定 (D-018) には関わらない。
/// 既定 (`auto`) のときは保存で `[tui]` を書き出さない (古い ai-cu が読めなくなるのを避ける)。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tui {
    /// 配色。既定 `auto`
    #[serde(default)]
    pub theme: ThemeName,
}

/// `indicator.lead_ms` の上限 (ミリ秒)。超えたら設定のエラー (D-093。誤記で毎回長く止まるのを防ぐ)
pub const INDICATOR_LEAD_MAX_MS: u64 = 30_000;

/// 操作の前の枠と予告の間 (D-093)。既定は知らせる側 (枠を出し、3 秒の予告、音は鳴らさない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndicatorConfig {
    /// 画面を変える操作の前に、画面の縁に青い枠と案内を出すか。既定 true (どれかの層が true なら出す)
    #[serde(default = "IndicatorConfig::default_enabled")]
    pub enabled: bool,
    /// 枠を出してから最初の操作までの予告の間 (ミリ秒。0 なら待たない)。既定 3000。上限 `INDICATOR_LEAD_MAX_MS`
    #[serde(default = "IndicatorConfig::default_lead_ms")]
    pub lead_ms: u64,
    /// 予告の間に入るときに通知音を鳴らすか。既定 false
    #[serde(default)]
    pub sound: bool,
}

impl IndicatorConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_lead_ms() -> u64 {
        3000
    }
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        IndicatorConfig {
            enabled: Self::default_enabled(),
            lead_ms: Self::default_lead_ms(),
            sound: false,
        }
    }
}

/// MCP の道具の名前 (D-078)。`mcp::tools()` の `name` と同じ並び (テストで見張る)。
/// `config` は `mcp` に依存しないので、読み込みで名前を検査するための一覧をここに置く
pub const MCP_TOOL_NAMES: [&str; 19] = [
    "guide",
    "config",
    "status",
    "windows",
    "shot",
    "find",
    "tree",
    "text",
    "wait_for",
    "focus",
    "click",
    "double_click",
    "drag",
    "move",
    "scroll",
    "type",
    "key",
    "close",
    "run",
];

/// MCP サーバーモードの設定 (D-078)。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mcp {
    /// 呼ぶたびにクライアント (Claude Code) に人の承認を求めさせる道具の名前。`tools/list` で
    /// `anthropic/requiresUserInteraction` の印を付ける。書いた層の和集合 (D-078)
    #[serde(default)]
    pub require_approval: Vec<String>,
}

/// 撮影の制限 (D-049)。既定は禁止側 (全画面は撮らない、shot の前に 3 秒の無操作)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    /// 全画面 (region を省略した撮影、または仮想デスクトップ全体を覆う矩形) を許すか。既定 false
    #[serde(default)]
    pub full_screen: bool,
    /// `shot` の直前に、人の最後の入力からこれ以上の無操作を求める (ミリ秒)。0 なら求めない。既定 3000
    #[serde(default = "Capture::default_min_idle_ms")]
    pub min_idle_ms: u64,
    /// 撮影の保存先として許すフォルダ (絶対パス)。空なら制限なし。書いた層はすべてが許すときだけ通す (D-075)
    #[serde(default)]
    pub save_dirs: Vec<String>,
}

impl Capture {
    fn default_min_idle_ms() -> u64 {
        3000
    }
}

impl Default for Capture {
    fn default() -> Self {
        Capture {
            full_screen: false,
            min_idle_ms: Self::default_min_idle_ms(),
            save_dirs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    /// マウスとキーの実入力の総スイッチ。既定は false (fail-closed)
    #[serde(default)]
    pub input_enabled: bool,
    /// 画面撮影の総スイッチ。既定は false
    #[serde(default)]
    pub capture_enabled: bool,
    /// パスワードの要素 (UIA の IsPassword) にフォーカスがあるときの type / key を許す。既定は false (D-090)
    #[serde(default)]
    pub password_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub processes: Vec<String>,
    #[serde(default)]
    pub classes: Vec<String>,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "Limits::default_max_actions")]
    pub max_actions: u32,
    #[serde(default = "Limits::default_max_duration_ms")]
    pub max_duration_ms: u64,
    #[serde(default = "Limits::default_max_text_len")]
    pub max_text_len: u32,
}

impl Limits {
    fn default_max_actions() -> u32 {
        1000
    }
    fn default_max_duration_ms() -> u64 {
        600_000
    }
    fn default_max_text_len() -> u32 {
        10_000
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_actions: Self::default_max_actions(),
            max_duration_ms: Self::default_max_duration_ms(),
            max_text_len: Self::default_max_text_len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    /// このファイルが存在したら停止する。空なら使わない
    #[serde(default)]
    pub sentinel_file: String,
    /// 自分が送った移動の線分から、実際のカーソルがこれより離れていたら、利用者が動かしたとみなして停止する
    #[serde(default = "Stop::default_threshold")]
    pub mouse_threshold_px: u32,
}

impl Stop {
    fn default_threshold() -> u32 {
        50
    }
}

impl Default for Stop {
    fn default() -> Self {
        Stop {
            sentinel_file: String::new(),
            mouse_threshold_px: Self::default_threshold(),
        }
    }
}

// ---------------------------------------------------------------------------
// エラー
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub path: Option<PathBuf>,
    pub message: String,
}

impl ConfigError {
    pub fn new(path: Option<&Path>, message: impl Into<String>) -> Self {
        ConfigError {
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{}: {}", p.display(), self.message),
            None => f.write_str(&self.message),
        }
    }
}

// ---------------------------------------------------------------------------
// 解析と検証
// ---------------------------------------------------------------------------

/// プロセス名の正規化: 小文字にして、パスの部分と末尾の `.exe` を外す。
pub fn normalize_process(s: &str) -> String {
    let base = s
        .trim()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    base.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(base)
}

pub fn normalize_class(s: &str) -> String {
    s.trim().to_lowercase()
}

/// 検証済みの規則 (正規化してある)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompiledRules {
    pub actions: Vec<ActionKind>,
    pub processes: Vec<String>,
    pub classes: Vec<String>,
    pub regions: Vec<Rect>,
    pub keys: Vec<KeyCombo>,
}

impl CompiledRules {
    fn compile(rules: &Rules, side: &str) -> Result<Self, String> {
        let mut out = CompiledRules::default();
        for a in &rules.actions {
            out.actions.push(
                ActionKind::parse(a)
                    .ok_or_else(|| format!("{side}.actions: 未知の操作名 {a:?}"))?,
            );
        }
        for p in &rules.processes {
            let n = normalize_process(p);
            if n.is_empty() {
                return Err(format!("{side}.processes: 空の名前"));
            }
            out.processes.push(n);
        }
        for c in &rules.classes {
            let n = normalize_class(c);
            if n.is_empty() {
                return Err(format!("{side}.classes: 空の名前"));
            }
            out.classes.push(n);
        }
        for r in &rules.regions {
            out.regions.push(
                r.parse::<Rect>()
                    .map_err(|e| format!("{side}.regions: {e}"))?,
            );
        }
        for k in &rules.keys {
            out.keys.push(
                k.parse::<KeyCombo>()
                    .map_err(|e| format!("{side}.keys: {e}"))?,
            );
        }
        Ok(out)
    }
}

/// 層の役割 (D-079)。拒否の直し方 (`policy::Denied::fix`) の <どこで> を決める
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerRole {
    /// 床 (人間の設定のファイル)
    Floor,
    /// 床の場所はあるが、ファイルが無い (何も許可しない閉じた層)
    NoFloor,
    /// 床の場所そのものを引けない (何も許可しない閉じた層。`FloorSource::Absent`、`Policy::closed`)
    Unresolved,
    /// 追加の層 (`AI_CU_CONFIG`、`--config`)
    Overlay,
}

/// 判定に使う 1 つの層。床 (人間の設定) か、追加の層 (`AI_CU_CONFIG`、`--config`)。
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// 表示用の出所 (ファイルのパス、または「床の設定が無い」などの説明)
    pub source: String,
    /// 層の役割 (D-079)。`closed` は `Unresolved`、`from_config` は `Floor` で作り、`load_layers` が直す
    pub role: LayerRole,
    pub general: General,
    pub allow: CompiledRules,
    pub deny: CompiledRules,
    pub limits: Limits,
    pub stop: Stop,
    pub capture: Capture,
    /// `capture.save_dirs` を検査して字面で正規化したもの (D-075)。判定と表示はこちらを使い、
    /// `capture.save_dirs` (書いたままの文字列) は使わない
    pub save_dirs: Vec<PathBuf>,
    /// `mcp.require_approval` を検査して小文字にそろえ、重複を除いたもの (D-078)
    pub mcp_require_approval: Vec<String>,
    /// 操作の前の枠と予告の間 (D-093)。`[indicator]` を書いていない層も既定値で合成に加わる
    pub indicator: IndicatorConfig,
}

/// 設定に書くパス 1 つの検査 (D-075)。絶対パスでない、または `%` を含めば断る (環境変数は展開しない。
/// 相対パスは呼び出し側の作業ディレクトリで意味が変わり、黙って効かなくなる)。空文字では呼ばない (呼ぶ側が先に除く)。
pub(crate) fn check_setting_abs_path(key: &str, s: &str) -> Result<(), String> {
    if !Path::new(s).is_absolute() || s.contains('%') {
        return Err(format!(
            "{key} は絶対パスで書く (環境変数の展開はしない): {s:?}"
        ));
    }
    Ok(())
}

/// `capture.save_dirs` の検査と正規化 (D-075)。要素ごとに、空文字 (trim 後)、`\\?\` と `\\.\` で始まるもの、
/// 絶対パスでないものと `%` を含むものを断り、字面で `.` と `..` を畳んで返す。検査と正規化はここだけで行う (TUI も使う)。
pub fn normalized_save_dirs(dirs: &[String]) -> Result<Vec<PathBuf>, String> {
    dirs.iter()
        .map(|d| {
            let d = d.trim();
            if d.is_empty() {
                return Err("capture.save_dirs に空のフォルダは書けない".to_string());
            }
            if crate::image::is_verbatim_or_device(d) {
                return Err(format!(
                    "capture.save_dirs に \\\\?\\ や \\\\.\\ で始まるパスは使えない: {d:?}"
                ));
            }
            check_setting_abs_path("capture.save_dirs", d)?;
            Ok(crate::image::normalize_save_path(d, Path::new("")))
        })
        .collect()
}

/// `mcp.require_approval` の検査と正規化 (D-078)。要素ごとに trim して小文字にそろえ、空の名前と
/// `MCP_TOOL_NAMES` に無い名前を断り、重複は最初の 1 つを残す。検査と正規化はここだけで行う (TUI も使う)。
pub fn normalized_mcp_tools(names: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in names {
        let n = raw.trim().to_lowercase();
        if n.is_empty() {
            return Err("mcp.require_approval: 空の名前".to_string());
        }
        if !MCP_TOOL_NAMES.contains(&n.as_str()) {
            return Err(format!("mcp.require_approval: 未知の道具の名前 {raw:?}"));
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

impl Layer {
    /// 何も許可しない層 (床の設定が無いとき。D-018 の fail-closed)。
    pub fn closed(source: impl Into<String>) -> Layer {
        Layer {
            source: source.into(),
            role: LayerRole::Unresolved,
            general: General::default(),
            allow: CompiledRules::default(),
            deny: CompiledRules::default(),
            limits: Limits::default(),
            stop: Stop::default(),
            capture: Capture::default(),
            save_dirs: Vec::new(),
            mcp_require_approval: Vec::new(),
            indicator: IndicatorConfig::default(),
        }
    }

    pub fn from_config(cfg: &Config, source: impl Into<String>) -> Result<Layer, String> {
        if cfg.limits.max_actions == 0 {
            return Err("limits.max_actions は 1 以上".into());
        }
        if cfg.limits.max_duration_ms <= crate::validate::DEADLINE_MARGIN_MS {
            return Err(format!(
                "limits.max_duration_ms は実行の余裕 {} ms より大きくする",
                crate::validate::DEADLINE_MARGIN_MS
            ));
        }
        if cfg.stop.mouse_threshold_px == 0 {
            return Err("stop.mouse_threshold_px は 1 以上".into());
        }
        let sentinel = cfg.stop.sentinel_file.trim();
        // 相対パスは呼び出し側の作業ディレクトリで解決され、停止が黙って効かなくなりうるので拒否する
        if !sentinel.is_empty() {
            check_setting_abs_path("stop.sentinel_file", sentinel)?;
        }
        if cfg.indicator.lead_ms > INDICATOR_LEAD_MAX_MS {
            return Err(format!(
                "indicator.lead_ms は {INDICATOR_LEAD_MAX_MS} 以下にする (ミリ秒): {}",
                cfg.indicator.lead_ms
            ));
        }
        let save_dirs = normalized_save_dirs(&cfg.capture.save_dirs)?;
        let mcp_require_approval = normalized_mcp_tools(&cfg.mcp.require_approval)?;
        Ok(Layer {
            source: source.into(),
            role: LayerRole::Floor,
            general: cfg.general.clone(),
            allow: CompiledRules::compile(&cfg.allow, "allow")?,
            deny: CompiledRules::compile(&cfg.deny, "deny")?,
            limits: cfg.limits.clone(),
            stop: cfg.stop.clone(),
            capture: cfg.capture.clone(),
            save_dirs,
            mcp_require_approval,
            indicator: cfg.indicator.clone(),
        })
    }
}

impl Config {
    /// TOML を読む。版を先に確かめ、未知のフィールドと未知の版は拒否し、中身も検証する。
    pub fn parse(text: &str) -> Result<Config, String> {
        let text = text.trim_start_matches('\u{feff}');
        let table: toml::Table =
            toml::from_str(text).map_err(|e| format!("TOML として読めない: {e}"))?;
        match table.get("version") {
            Some(toml::Value::Integer(v)) if *v == i64::from(CONFIG_VERSION) => {}
            Some(toml::Value::Integer(v)) => {
                return Err(format!(
                    "設定の version {v} は未対応 (対応しているのは {CONFIG_VERSION})"
                ));
            }
            Some(_) => return Err("version は整数で書く".into()),
            None => return Err(format!("version = {CONFIG_VERSION} が必要")),
        }
        let cfg: Config = toml::from_str(text).map_err(|e| format!("設定の書式が不正: {e}"))?;
        Layer::from_config(&cfg, "")?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::new(Some(path), format!("読めない: {e}")))?;
        Config::parse(&text).map_err(|m| ConfigError::new(Some(path), m))
    }
}

// ---------------------------------------------------------------------------
// 場所の解決 (D-029)
// ---------------------------------------------------------------------------

/// 環境変数の参照。テストでは表で差し替える。
pub trait EnvLookup {
    fn var(&self, key: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var_os(key).map(|v| v.to_string_lossy().into_owned())
    }
}

#[derive(Debug, Clone, Default)]
pub struct MapEnv(pub HashMap<String, String>);

impl MapEnv {
    pub fn new(pairs: &[(&str, &str)]) -> Self {
        MapEnv(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
}

impl EnvLookup for MapEnv {
    fn var(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// 開発シェル (`AI_CU_DEV_FORBID_REAL_INPUT` が空でない値で設定されている) か。
pub fn dev_forbid_active(env: &dyn EnvLookup) -> bool {
    // 値を問わず、変数があれば開発シェルとみなす (D-022 の「存在するとき」。"0" や空でも緩めない)
    env.var(ENV_DEV_FORBID).is_some()
}

/// 既定の場所 (床) の解決。実装を 1 つに隔離して差し替えられるようにする (D-010、D-029)。
pub trait DefaultLocator {
    fn default_config_path(&self) -> Result<PathBuf, String>;
}

/// 実際の既定の場所: `SHGetKnownFolderPath(FOLDERID_RoamingAppData)\ai-cu\config.toml`。
/// 環境変数 `APPDATA` は読まない (AI が書き換えて床を差し替えられないように)。
pub struct SystemLocator;

impl DefaultLocator for SystemLocator {
    fn default_config_path(&self) -> Result<PathBuf, String> {
        let base = crate::win32::known_folder::roaming_app_data()?;
        Ok(base.join("ai-cu").join("config.toml"))
    }
}

/// 固定のパスを返す locator (テスト用。`target/` 配下を指す)。
pub struct FixedLocator(pub PathBuf);

impl DefaultLocator for FixedLocator {
    fn default_config_path(&self) -> Result<PathBuf, String> {
        Ok(self.0.clone())
    }
}

/// 呼ばれたら panic する locator。テストで、実の既定の場所に触れていないことを保証する。
pub struct TripwireLocator;

impl DefaultLocator for TripwireLocator {
    fn default_config_path(&self) -> Result<PathBuf, String> {
        panic!("tripwire: 既定の設定の場所が引かれた (テストは target/ を指すこと)");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloorSource {
    /// 既定の場所 (人間の設定)
    Default(PathBuf),
    /// 開発シェルで `AI_CU_DEV_FLOOR` が指すファイル
    DevFloor(PathBuf),
    /// 床が無い (何も許可しない層になる)
    Absent(String),
}

impl FloorSource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            FloorSource::Default(p) | FloorSource::DevFloor(p) => Some(p),
            FloorSource::Absent(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub floor: FloorSource,
    pub env_overlay: Option<PathBuf>,
    pub cli_overlay: Option<PathBuf>,
    pub dev_mode: bool,
}

impl ConfigPaths {
    /// 表示と編集の対象: コマンドラインの指定、環境変数、既定の場所の順で最初のもの (CLAUDE.md)。
    pub fn edit_target(&self) -> Option<PathBuf> {
        self.cli_overlay
            .clone()
            .or_else(|| self.env_overlay.clone())
            .or_else(|| self.floor.path().map(Path::to_path_buf))
    }
}

pub fn resolve_paths(
    cli: Option<&Path>,
    env: &dyn EnvLookup,
    locator: &dyn DefaultLocator,
) -> ConfigPaths {
    let dev_mode = dev_forbid_active(env);
    let floor = if dev_mode {
        // 開発シェルでは利用者の実際の設定を読まない (D-010)。既定の場所は引かない
        match env.var(ENV_DEV_FLOOR).filter(|v| !v.trim().is_empty()) {
            Some(p) => FloorSource::DevFloor(PathBuf::from(p)),
            None => FloorSource::Absent(format!(
                "開発シェル ({ENV_DEV_FORBID}) では既定の設定を読まない ({ENV_DEV_FLOOR} も未設定)"
            )),
        }
    } else {
        match locator.default_config_path() {
            Ok(p) => FloorSource::Default(p),
            Err(e) => FloorSource::Absent(format!("既定の設定の場所を引けない: {e}")),
        }
    };
    ConfigPaths {
        floor,
        env_overlay: env
            .var(ENV_CONFIG)
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from),
        cli_overlay: cli.map(Path::to_path_buf),
        dev_mode,
    }
}

/// 判定に使う層を読む。床のファイルが無ければ「何も許可しない層」。床が読めない・壊れているとき、
/// 指定された追加の層が無い・壊れているときは、黙って既定に落とさずエラーにする (D-018)。
pub fn load_layers(paths: &ConfigPaths) -> Result<Vec<Layer>, ConfigError> {
    let mut layers = Vec::new();
    match &paths.floor {
        FloorSource::Absent(why) => layers.push(Layer::closed(format!("(基本の層なし) {why}"))),
        FloorSource::Default(p) | FloorSource::DevFloor(p) => {
            if p.exists() {
                layers.push(load_layer(p)?);
            } else {
                let mut closed = Layer::closed(format!(
                    "(基本の層なし) {} が無い。実入力と撮影は許可されない",
                    p.display()
                ));
                closed.role = LayerRole::NoFloor;
                layers.push(closed);
            }
        }
    }
    for p in [&paths.env_overlay, &paths.cli_overlay]
        .into_iter()
        .flatten()
    {
        if !p.exists() {
            return Err(ConfigError::new(Some(p), "指定された設定ファイルが無い"));
        }
        let mut overlay = load_layer(p)?;
        overlay.role = LayerRole::Overlay;
        layers.push(overlay);
    }
    Ok(layers)
}

fn load_layer(p: &Path) -> Result<Layer, ConfigError> {
    let cfg = Config::load(p)?;
    Layer::from_config(&cfg, p.display().to_string()).map_err(|m| ConfigError::new(Some(p), m))
}

// ---------------------------------------------------------------------------
// 保存 (TUI から。コメントを残す差分編集)
// ---------------------------------------------------------------------------

/// 新しく作るときの雛形。コメントで各項目を説明する。
pub const TEMPLATE: &str = r#"# ai-cu の設定ファイル。書式は README.md の「設定ファイル」を参照。
version = 1

[general]
# マウスとキーの実入力を許すか (既定 false = すべて拒否)
input_enabled = false
# 画面撮影を許すか (既定 false)
capture_enabled = false
# パスワード欄にフォーカスがあるときの type / key を許すか (既定 false = パスワードは人が打つ)
password_input = false

# allow のリストは、空なら制限なし、1 件でもあれば一致が必須。deny は allow より強い。
[allow]
actions = []
processes = []
classes = []
regions = []
keys = []

# 雛形の deny.processes は、AI に操作させると危ないもの (端末とシェル、管理ツール、パスワード管理ソフト) を最初から並べる。
# 使わないものを消してもよいが、入力を有効にする前に、自分の端末とパスワード管理ソフトが入っているかを確かめる。
[deny]
actions = []
processes = [
    "cmd.exe", "powershell.exe", "pwsh.exe", "WindowsTerminal.exe", "OpenConsole.exe", "conhost.exe",
    "regedit.exe", "mmc.exe",
    "KeePass.exe", "KeePassXC.exe", "1Password.exe", "Bitwarden.exe",
]
classes = []
regions = []
keys = []

[limits]
max_actions = 1000
max_duration_ms = 600000
max_text_len = 10000

[stop]
# このファイルが存在したら停止する (空なら使わない)
sentinel_file = ""
# 利用者がマウスをこれ以上動かしたら停止する (ピクセル)
mouse_threshold_px = 50

[capture]
# 全画面 (region を省略した撮影、または画面全体を覆う矩形) を許すか (既定 false)
full_screen = false
# shot、focus、前面にある対象への close の前に、人の最後の入力からこれ以上の無操作を求める (ミリ秒。0 なら求めない)
min_idle_ms = 3000
# 撮影の保存先として許すフォルダ (絶対パス)。空なら制限なし。書いた層はすべてが許すときだけ通す
save_dirs = []

[mcp]
# MCP の道具のうち、呼ぶたびにクライアント (Claude Code) に人の承認を求めさせるもの (道具の名前。例 ["click", "type", "run"])
require_approval = []

[indicator]
# 画面を変える操作 (マウス、キー、focus、close) の前に、画面の縁に青い枠と案内を出すか (既定 true)
enabled = true
# 枠を出してから最初の操作までの予告の間 (ミリ秒。0 なら待たない。枠が lead_ms 以上前から出ていれば待たず、出て間もなければ残りだけ待つ。上限 30000)
lead_ms = 3000
# 予告の間に入るときに通知音を鳴らすか (既定 false)
sound = false

# [tui]
# theme = "auto"   # auto | calm | calm-light | ansi | mono (TUI の配色。D-129)
"#;

/// 値が意味として同じか (書式やコメントは見ない)。
fn same_value(old: &toml_edit::Value, new: &toml_edit::Value) -> bool {
    use toml_edit::Value as V;
    match (old, new) {
        (V::Boolean(a), V::Boolean(b)) => a.value() == b.value(),
        (V::Integer(a), V::Integer(b)) => a.value() == b.value(),
        (V::String(a), V::String(b)) => a.value() == b.value(),
        (V::Array(a), V::Array(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| x.as_str().is_some() && x.as_str() == y.as_str())
        }
        _ => false,
    }
}

fn set_preserving(table: &mut toml_edit::Table, key: &str, new: toml_edit::Value) {
    match table.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
        // 同じ値なら触らない (複数行の配列の中のコメントや改行を残す)
        Some(old) if same_value(old, &new) => {}
        Some(old) => {
            let decor = old.decor().clone();
            *old = new;
            *old.decor_mut() = decor;
        }
        None => {
            table.insert(key, toml_edit::Item::Value(new));
        }
    }
}

fn table_mut<'a>(doc: &'a mut toml_edit::DocumentMut, name: &str) -> &'a mut toml_edit::Table {
    let root = doc.as_table_mut();
    let is_table = root.get(name).is_some_and(toml_edit::Item::is_table);
    if !is_table {
        root.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    root.get_mut(name)
        .and_then(toml_edit::Item::as_table_mut)
        .expect("直前に表を入れた")
}

fn string_array(items: &[String]) -> toml_edit::Value {
    let mut arr = toml_edit::Array::new();
    for s in items {
        arr.push(s.as_str());
    }
    toml_edit::Value::Array(arr)
}

/// 既存の TOML の文字列に、設定の値を反映した文字列を返す。コメントと並びは残す。
pub fn render_edit(existing: Option<&str>, cfg: &Config) -> Result<String, String> {
    let base = existing.unwrap_or(TEMPLATE).trim_start_matches('\u{feff}');
    let mut doc: toml_edit::DocumentMut = base
        .parse()
        .map_err(|e| format!("既存の設定を TOML として読めない: {e}"))?;
    set_preserving(doc.as_table_mut(), "version", i64::from(cfg.version).into());
    {
        let t = table_mut(&mut doc, "general");
        set_preserving(t, "input_enabled", cfg.general.input_enabled.into());
        set_preserving(t, "capture_enabled", cfg.general.capture_enabled.into());
        set_preserving(t, "password_input", cfg.general.password_input.into());
    }
    for (name, rules) in [("allow", &cfg.allow), ("deny", &cfg.deny)] {
        let t = table_mut(&mut doc, name);
        set_preserving(t, "actions", string_array(&rules.actions));
        set_preserving(t, "processes", string_array(&rules.processes));
        set_preserving(t, "classes", string_array(&rules.classes));
        set_preserving(t, "regions", string_array(&rules.regions));
        set_preserving(t, "keys", string_array(&rules.keys));
    }
    {
        let t = table_mut(&mut doc, "limits");
        set_preserving(t, "max_actions", i64::from(cfg.limits.max_actions).into());
        set_preserving(
            t,
            "max_duration_ms",
            to_i64(cfg.limits.max_duration_ms)?.into(),
        );
        set_preserving(t, "max_text_len", i64::from(cfg.limits.max_text_len).into());
    }
    {
        let t = table_mut(&mut doc, "stop");
        set_preserving(t, "sentinel_file", cfg.stop.sentinel_file.as_str().into());
        set_preserving(
            t,
            "mouse_threshold_px",
            i64::from(cfg.stop.mouse_threshold_px).into(),
        );
    }
    {
        let t = table_mut(&mut doc, "capture");
        set_preserving(t, "full_screen", cfg.capture.full_screen.into());
        set_preserving(t, "min_idle_ms", to_i64(cfg.capture.min_idle_ms)?.into());
        set_preserving(t, "save_dirs", string_array(&cfg.capture.save_dirs));
    }
    {
        let t = table_mut(&mut doc, "mcp");
        set_preserving(
            t,
            "require_approval",
            string_array(&cfg.mcp.require_approval),
        );
    }
    {
        let t = table_mut(&mut doc, "indicator");
        set_preserving(t, "enabled", cfg.indicator.enabled.into());
        set_preserving(t, "lead_ms", to_i64(cfg.indicator.lead_ms)?.into());
        set_preserving(t, "sound", cfg.indicator.sound.into());
    }
    // `[tui]` は既定 (auto) なら書き出さない (D-129)。ファイルにすでにあれば、auto に戻しても値を更新する
    if cfg.tui.theme != ThemeName::Auto || doc.as_table().get("tui").is_some() {
        let t = table_mut(&mut doc, "tui");
        set_preserving(t, "theme", cfg.tui.theme.as_str().into());
    }
    let out = doc.to_string();
    // 書いたものが読み戻せることを確かめる (壊れた設定を保存しない)
    let back = Config::parse(&out)?;
    if &back != cfg {
        return Err("保存しようとした内容を読み戻すと値が変わる".into());
    }
    Ok(out)
}

fn to_i64(v: u64) -> Result<i64, String> {
    i64::try_from(v).map_err(|_| format!("値が大きすぎる: {v}"))
}

/// 設定を保存する。既存のファイルは `.bak` に写し、一時ファイルに書いてから置き換える。
pub fn save(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    save_with(path, cfg, false)
}

/// `replace_broken` が真なら、既存の内容 (読めなかった設定) を引き継がず雛形から書く。元は `.bak` に残る。
pub fn save_with(path: &Path, cfg: &Config, replace_broken: bool) -> Result<(), ConfigError> {
    let existing = if path.exists() {
        Some(
            std::fs::read(path)
                .map_err(|e| ConfigError::new(Some(path), format!("読めない: {e}")))?,
        )
    } else {
        None
    };
    let base = if replace_broken {
        None
    } else {
        match &existing {
            Some(bytes) => Some(
                String::from_utf8(bytes.clone())
                    .map_err(|_| ConfigError::new(Some(path), "UTF-8 ではない"))?,
            ),
            None => None,
        }
    };
    let text = render_edit(base.as_deref(), cfg).map_err(|m| ConfigError::new(Some(path), m))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| ConfigError::new(Some(path), format!("フォルダを作れない: {e}")))?;
    }
    if existing.is_some() {
        // 読めなかった元の設定は、普段の .bak (次の保存で上書きされる) とは別の名前に残す
        let bak = if replace_broken {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            sibling(path, &format!(".broken-{secs}.bak"))
        } else {
            sibling(path, ".bak")
        };
        std::fs::copy(path, &bak)
            .map_err(|e| ConfigError::new(Some(path), format!("バックアップを作れない: {e}")))?;
    }
    let tmp = sibling(path, ".tmp");
    std::fs::write(&tmp, text.as_bytes())
        .map_err(|e| ConfigError::new(Some(path), format!("一時ファイルに書けない: {e}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| ConfigError::new(Some(path), format!("置き換えられない: {e}")))?;
    Ok(())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        crate::testing::unique_temp_dir(&format!("config-{name}"))
    }

    /// 雛形の deny.processes に最初から並べる名前 (D-117)
    const TEMPLATE_DENY_PROCESSES: [&str; 12] = [
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "WindowsTerminal.exe",
        "OpenConsole.exe",
        "conhost.exe",
        "regedit.exe",
        "mmc.exe",
        "KeePass.exe",
        "KeePassXC.exe",
        "1Password.exe",
        "Bitwarden.exe",
    ];

    /// 雛形は、deny.processes の初期の並び (D-117) のほかは既定の設定と同じ
    #[test]
    fn config_template_parses_to_default() {
        let mut expected = Config::default();
        expected.deny.processes = TEMPLATE_DENY_PROCESSES.map(String::from).to_vec();
        assert_eq!(Config::parse(TEMPLATE).unwrap(), expected);
    }

    /// 雛形の層は、端末とパスワード管理ソフトの窓を deny に当てる (D-117)。入力と撮影は既定で無効のまま
    #[test]
    fn template_layer_denies_terminals_and_password_managers() {
        let cfg = Config::parse(TEMPLATE).unwrap();
        assert!(!cfg.general.input_enabled && !cfg.general.capture_enabled);
        let layer = Layer::from_config(&cfg, "t").unwrap();
        let policy = crate::policy::Policy::new(vec![layer]);
        for exe in [
            "cmd.exe",
            "WindowsTerminal.exe",
            "PWSH.EXE",
            "keepassxc",
            "Bitwarden.exe",
        ] {
            assert!(
                policy.check_process(exe).is_err(),
                "雛形の deny が {exe} を拒否しない"
            );
        }
        assert!(policy.check_process("notepad.exe").is_ok());
    }

    #[test]
    fn config_empty_or_missing_version_is_rejected() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse("[general]\ninput_enabled = true\n").is_err());
    }

    #[test]
    fn config_minimal_uses_fail_closed_defaults() {
        let c = Config::parse("version = 1\n").unwrap();
        assert!(!c.general.input_enabled);
        assert!(!c.general.capture_enabled);
        assert_eq!(c.limits, Limits::default());
        // 撮影の制限 (D-049): 全画面は既定で拒否、shot の前に 3 秒の無操作を求める
        assert!(!c.capture.full_screen);
        assert_eq!(c.capture.min_idle_ms, 3000);
    }

    /// パスワード欄への入力の総スイッチ (D-090): 書かなければ false、書けば読め、保存でコメントと値が残る
    #[test]
    fn password_input_defaults_false_and_round_trips() {
        assert!(
            !Config::parse("version = 1\n")
                .unwrap()
                .general
                .password_input
        );
        let c = Config::parse("version = 1\n[general]\npassword_input = true\n").unwrap();
        assert!(c.general.password_input);
        assert!(!c.general.input_enabled);
        let dir = test_dir("password-input");
        let f = dir.join("config.toml");
        let original =
            "# 人間のコメント\nversion = 1\n\n[general]\ninput_enabled = true # 行末のコメント\n";
        std::fs::write(&f, original).unwrap();
        let mut cfg = Config::load(&f).unwrap();
        assert!(!cfg.general.password_input);
        cfg.general.password_input = true;
        save(&f, &cfg).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("# 人間のコメント"), "{text}");
        assert!(text.contains("# 行末のコメント"), "{text}");
        assert!(text.contains("password_input = true"), "{text}");
        let back = Config::load(&f).unwrap();
        assert!(back.general.password_input);
        assert_eq!(back, cfg);
    }

    #[test]
    fn config_capture_section_parses_and_round_trips() {
        let c =
            Config::parse("version = 1\n[capture]\nfull_screen = true\nmin_idle_ms = 0\n").unwrap();
        assert!(c.capture.full_screen);
        assert_eq!(c.capture.min_idle_ms, 0);
        assert!(Config::parse("version = 1\n[capture]\nfullscreen = true\n").is_err());
        let text = render_edit(None, &c).unwrap();
        assert!(text.contains("full_screen = true"), "{text}");
        assert!(text.contains("min_idle_ms = 0"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
        // 既存のファイルに [capture] が無くても、保存すると足される
        let old = "version = 1\n[general]\ninput_enabled = true\n";
        let text = render_edit(Some(old), &c).unwrap();
        assert!(text.contains("[capture]"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
    }

    /// `[indicator]` を書かなければ既定値 (true、3000、false)。書いたキーだけが変わる (D-093)
    #[test]
    fn indicator_defaults_when_section_missing() {
        let cases: [(&str, IndicatorConfig); 4] = [
            ("", IndicatorConfig::default()),
            (
                "[indicator]\n",
                IndicatorConfig {
                    enabled: true,
                    lead_ms: 3000,
                    sound: false,
                },
            ),
            (
                "[indicator]\nenabled = false\n",
                IndicatorConfig {
                    enabled: false,
                    lead_ms: 3000,
                    sound: false,
                },
            ),
            (
                "[indicator]\nlead_ms = 0\nsound = true\n",
                IndicatorConfig {
                    enabled: true,
                    lead_ms: 0,
                    sound: true,
                },
            ),
        ];
        assert_eq!(
            IndicatorConfig::default(),
            IndicatorConfig {
                enabled: true,
                lead_ms: 3000,
                sound: false,
            }
        );
        for (body, want) in cases {
            let c = Config::parse(&format!("version = 1\n{body}")).unwrap();
            assert_eq!(c.indicator, want, "{body:?}");
            let l = Layer::from_config(&c, "t").unwrap();
            assert_eq!(l.indicator, want, "{body:?}");
        }
        // 閉じた層 (基本の層なし) も既定値
        assert_eq!(Layer::closed("x").indicator, IndicatorConfig::default());
    }

    /// 未知のキーと型の違う値は拒否する (D-093。色や太さのキーは今は無い)
    #[test]
    fn indicator_rejects_unknown_key() {
        for body in [
            "[indicator]\ncolor = 'red'\n",
            "[indicator]\nEnabled = true\n",
            "[indicator]\nlead = 3000\n",
            "[indicator]\nenabled = 1\n",
            "[indicator]\nlead_ms = -1\n",
            "[indicator]\nsound = 'yes'\n",
        ] {
            assert!(
                Config::parse(&format!("version = 1\n{body}")).is_err(),
                "{body:?}"
            );
        }
    }

    /// `lead_ms` の上限は 30000 (D-093)。30001 は設定のエラーで、文面にキーと上限が出る
    #[test]
    fn indicator_rejects_lead_over_limit() {
        let ok = Config::parse("version = 1\n[indicator]\nlead_ms = 30000\n").unwrap();
        assert_eq!(ok.indicator.lead_ms, INDICATOR_LEAD_MAX_MS);
        for v in [30_001u64, 3_000_000] {
            let e =
                Config::parse(&format!("version = 1\n[indicator]\nlead_ms = {v}\n")).unwrap_err();
            assert!(e.contains("indicator.lead_ms"), "{e}");
            assert!(e.contains("30000"), "{e}");
            // 構造体から作っても同じ判定 (TUI の保存は render_edit の読み戻しでここを通る)
            let mut c = Config::default();
            c.indicator.lead_ms = v;
            assert!(Layer::from_config(&c, "t").is_err());
            assert!(render_edit(None, &c).is_err());
        }
    }

    /// `[indicator]` の 3 キーを書けて、読み戻すと同じ。既存のファイルに節が無くても足され、コメントは残る
    #[test]
    fn indicator_render_edit_round_trips() {
        let c = Config {
            indicator: IndicatorConfig {
                enabled: false,
                lead_ms: 1500,
                sound: true,
            },
            ..Config::default()
        };
        let text = render_edit(None, &c).unwrap();
        assert!(text.contains("[indicator]"), "{text}");
        assert!(text.contains("enabled = false"), "{text}");
        assert!(text.contains("lead_ms = 1500"), "{text}");
        assert!(text.contains("sound = true"), "{text}");
        // 雛形のコメントが残る
        assert!(
            text.contains("# 予告の間に入るときに通知音を鳴らすか"),
            "{text}"
        );
        assert_eq!(Config::parse(&text).unwrap(), c);
        let old = "version = 1\n# 手書きのコメント\n[general]\ninput_enabled = false\n";
        let text = render_edit(Some(old), &c).unwrap();
        assert!(text.contains("# 手書きのコメント"), "{text}");
        assert!(text.contains("[indicator]"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
        // 既定に戻して保存し直しても読み戻せる
        let back = render_edit(Some(&text), &Config::default()).unwrap();
        assert_eq!(Config::parse(&back).unwrap(), Config::default());
    }

    /// 書き出した文字列に、表の見出し `[tui]` の行があるか (コメントの `# [tui]` は数えない)
    fn has_tui_table(text: &str) -> bool {
        text.lines().any(|l| l.trim() == "[tui]")
    }

    /// `[tui]` を書かなければ theme は auto。書けば読める。表だけで中身が無くても auto (D-129)
    #[test]
    fn tui_theme_defaults_to_auto_and_parses() {
        let c = Config::parse("version = 1\n").unwrap();
        assert_eq!(c.tui.theme, ThemeName::Auto);
        assert_eq!(c.tui, Tui::default());
        let c = Config::parse("version = 1\n[tui]\n").unwrap();
        assert_eq!(c.tui.theme, ThemeName::Auto);
        let c = Config::parse("version = 1\n[tui]\ntheme = \"calm-light\"\n").unwrap();
        assert_eq!(c.tui.theme, ThemeName::CalmLight);
        // 名前はどれも `as_str` の字面で読める
        for t in ThemeName::ALL {
            let text = format!("version = 1\n[tui]\ntheme = \"{}\"\n", t.as_str());
            assert_eq!(Config::parse(&text).unwrap().tui.theme, t, "{text}");
        }
    }

    /// 未知のテーマ名、大文字、型の違う値、未知のキーは読み込みのエラー (D-129)
    #[test]
    fn tui_rejects_unknown_theme_and_keys() {
        for body in [
            "[tui]\ntheme = \"neon\"\n",
            "[tui]\ntheme = \"Calm\"\n",
            "[tui]\ntheme = \"calm_light\"\n",
            "[tui]\ntheme = 1\n",
            "[tui]\ncolor = \"x\"\n",
            "[tui]\nTheme = \"calm\"\n",
        ] {
            assert!(
                Config::parse(&format!("version = 1\n{body}")).is_err(),
                "{body:?}"
            );
        }
    }

    /// 既定 (auto) のときは `[tui]` を書き出さない。古い ai-cu が `deny_unknown_fields` で読めなくなるのを、
    /// 既定のままの利用者に波及させない (D-129)。雛形の注釈 (`# [tui]`) は表ではない
    #[test]
    fn render_edit_omits_tui_when_auto() {
        let text = render_edit(None, &Config::default()).unwrap();
        assert!(!has_tui_table(&text), "{text}");
        assert!(text.contains("# [tui]"), "雛形の注釈が無い: {text}");
        let text = render_edit(Some("version = 1\n"), &Config::default()).unwrap();
        assert!(!has_tui_table(&text), "{text}");
        // calm にすると表と値が足される。既存の文字列に表が無くても、雛形からでも
        let calm = Config {
            tui: Tui {
                theme: ThemeName::Calm,
            },
            ..Config::default()
        };
        for base in [Some("version = 1\n"), None] {
            let text = render_edit(base, &calm).unwrap();
            assert!(has_tui_table(&text), "{text}");
            assert!(text.contains("theme = \"calm\""), "{text}");
            assert_eq!(Config::parse(&text).unwrap(), calm);
        }
    }

    /// calm で書いた文字列を元に auto へ戻して保存し直しても、読み戻すと auto。
    /// 既存の `[tui]` に手書きのコメントがあれば残る
    #[test]
    fn tui_theme_round_trips_back_to_auto() {
        let calm = Config {
            tui: Tui {
                theme: ThemeName::Calm,
            },
            ..Config::default()
        };
        let text = render_edit(
            Some("version = 1\n[tui]\n# 手書き\ntheme = \"calm\"\n"),
            &calm,
        )
        .unwrap();
        assert!(text.contains("# 手書き"), "{text}");
        let back = render_edit(Some(&text), &Config::default()).unwrap();
        assert_eq!(Config::parse(&back).unwrap().tui.theme, ThemeName::Auto);
        assert!(back.contains("# 手書き"), "{back}");
        // 雛形から calm にして auto に戻しても同じ
        let text = render_edit(None, &calm).unwrap();
        let back = render_edit(Some(&text), &Config::default()).unwrap();
        assert_eq!(Config::parse(&back).unwrap(), Config::default());
    }

    /// テーマ名の並びは auto → calm → calm-light → ansi → mono → auto (TUI の巡回の順。D-129)
    #[test]
    fn theme_name_cycles_through_all() {
        assert_eq!(ThemeName::ALL.len(), 5);
        assert_eq!(ThemeName::default(), ThemeName::Auto);
        assert_eq!(ThemeName::Auto.next(), ThemeName::Calm);
        assert_eq!(ThemeName::Mono.next(), ThemeName::Auto);
        // `next` は `ALL` の並びをたどって一周する
        let mut t = ThemeName::Auto;
        for want in ThemeName::ALL.iter().skip(1).chain([&ThemeName::Auto]) {
            t = t.next();
            assert_eq!(t, *want);
        }
        assert_eq!(
            ThemeName::ALL.map(ThemeName::as_str),
            ["auto", "calm", "calm-light", "ansi", "mono"]
        );
    }

    #[test]
    fn config_capture_save_dirs_parses_validates_and_round_trips() {
        let c = Config::parse(
            "version = 1\n[capture]\nsave_dirs = ['C:\\Shots', 'D:\\work\\.\\x\\..\\png']\n",
        )
        .unwrap();
        assert_eq!(
            c.capture.save_dirs,
            ["C:\\Shots", "D:\\work\\.\\x\\..\\png"]
        );
        // 層は字面で正規化した一覧を持つ
        let layer = Layer::from_config(&c, "t").unwrap();
        assert_eq!(
            layer.save_dirs,
            [PathBuf::from("C:\\Shots"), PathBuf::from("D:\\work\\png")]
        );
        assert!(Layer::closed("t").save_dirs.is_empty());
        assert!(
            Config::parse("version = 1\n")
                .unwrap()
                .capture
                .save_dirs
                .is_empty()
        );
        let text = render_edit(None, &c).unwrap();
        assert!(text.contains("save_dirs = ["), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
        // 既存のファイルに鍵が無くても、保存すると足される
        let old = "version = 1\n[capture]\nfull_screen = false\n";
        let text = render_edit(Some(old), &c).unwrap();
        assert!(text.contains("save_dirs"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
        for bad in [
            "shots",
            "%USERPROFILE%\\\\s",
            "",
            "  ",
            "\\\\\\\\?\\\\C:\\\\s",
            "\\\\\\\\.\\\\C:\\\\s",
            "//?/C:/s",
        ] {
            let text = format!("version = 1\n[capture]\nsave_dirs = [\"{bad}\"]\n");
            assert!(Config::parse(&text).is_err(), "{text}");
        }
    }

    fn mcp_config(names: &[&str]) -> Config {
        let mut c = Config::default();
        c.mcp.require_approval = names.iter().map(|s| s.to_string()).collect();
        c
    }

    /// `mcp.require_approval` (D-078): trim して小文字にそろえ、重複は最初の 1 つを残す
    #[test]
    fn mcp_require_approval_normalizes_case_space_and_duplicates() {
        let c = Config::parse(
            "version = 1\n[mcp]\nrequire_approval = [\"Click\", \" type \", \"click\"]\n",
        )
        .unwrap();
        // 設定の値は書いたまま、層は正規化した一覧
        assert_eq!(c.mcp.require_approval, ["Click", " type ", "click"]);
        let layer = Layer::from_config(&c, "t").unwrap();
        assert_eq!(layer.mcp_require_approval, ["click", "type"]);
        assert_eq!(
            normalized_mcp_tools(&c.mcp.require_approval).unwrap(),
            ["click", "type"]
        );
    }

    /// 未知の名前と空の名前は設定エラー (D-078)。検査は `Layer::from_config` にあり、それを呼ぶ `Config::parse` も同じ文で断る
    #[test]
    fn mcp_require_approval_rejects_unknown_and_empty_names() {
        for (names, want) in [
            (&["clik"][..], "未知の道具の名前 \"clik\""),
            (&["  "][..], "空の名前"),
        ] {
            let e = Layer::from_config(&mcp_config(names), "t").unwrap_err();
            assert!(e.contains("mcp.require_approval"), "{e}");
            assert!(e.contains(want), "{e}");
            let quoted: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
            let text = format!(
                "version = 1\n[mcp]\nrequire_approval = [{}]\n",
                quoted.join(", ")
            );
            let e = Config::parse(&text).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn mcp_section_rejects_unknown_key() {
        assert!(Config::parse("version = 1\n[mcp]\nrequire = []\n").is_err());
        assert!(Config::parse("version = 1\n[mcp]\nrequire_approval = []\n").is_ok());
    }

    /// 雛形の層と、`[mcp]` の無い上書き層は、どの道具にも承認を求めない (D-078)
    #[test]
    fn template_and_missing_mcp_section_mean_no_tools() {
        let template = Layer::from_config(&Config::parse(TEMPLATE).unwrap(), "t").unwrap();
        assert!(template.mcp_require_approval.is_empty());
        assert!(TEMPLATE.contains("[mcp]\n"), "{TEMPLATE}");
        let overlay = Config::parse("version = 1\n[general]\ninput_enabled = true\n").unwrap();
        assert!(overlay.mcp.require_approval.is_empty());
        let overlay = Layer::from_config(&overlay, "t").unwrap();
        assert!(overlay.mcp_require_approval.is_empty());
        assert!(Layer::closed("t").mcp_require_approval.is_empty());
    }

    #[test]
    fn render_edit_round_trips_mcp_require_approval() {
        let c = mcp_config(&["click"]);
        let text = render_edit(None, &c).unwrap();
        assert!(text.contains("require_approval = [\"click\"]"), "{text}");
        // 雛形のコメントが残る
        assert!(
            text.contains("# MCP の道具のうち、呼ぶたびにクライアント"),
            "{text}"
        );
        assert!(text.contains("# ai-cu の設定ファイル"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
        // 既存のファイルに [mcp] が無くても、保存すると足される
        let old = "version = 1\n[general]\ninput_enabled = false # 行末\n";
        let text = render_edit(Some(old), &c).unwrap();
        assert!(text.contains("[mcp]"), "{text}");
        assert!(text.contains("# 行末"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), c);
    }

    #[test]
    fn config_rejects_unknown_keys_and_versions() {
        for bad in [
            "version = 2\n",
            "version = \"1\"\n",
            "version = 1\nfoo = 1\n",
            "version = 1\n[general]\ninput_enable = true\n",
            "version = 1\n[allow]\nwindows = []\n",
            "version = 1\n[unknown]\n",
            "version = 1\n[allow]\nactions = [\"teleport\"]\n",
            "version = 1\n[deny]\nkeys = [\"ctrl+\"]\n",
            "version = 1\n[allow]\nregions = [\"1,2,3\"]\n",
            "version = 1\n[limits]\nmax_actions = 0\n",
            "version = 1\n[limits]\nmax_duration_ms = 1000\n",
            "version = 1\n[stop]\nmouse_threshold_px = 0\n",
            "version = 1\n[allow]\nprocesses = [\"\"]\n",
            "version = 1\n[stop]\nsentinel_file = \"stop.txt\"\n",
            "version = 1\n[stop]\nsentinel_file = '%TEMP%\\stop'\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    /// 未知のキーを書いた設定の失敗の文に、どの一覧かと使える名前が出る (D-131)
    #[test]
    fn config_unknown_key_error_names_side_and_usable_keys() {
        let e = Config::parse("version = 1\n[deny]\nkeys = [\"nope\"]\n").unwrap_err();
        assert!(e.contains("deny.keys"), "{e}");
        assert!(e.contains("kanji"), "{e}");
    }

    #[test]
    fn config_normalizes_names() {
        assert_eq!(normalize_process("C:\\Windows\\Notepad.EXE"), "notepad");
        assert_eq!(normalize_process("notepad"), "notepad");
        assert_eq!(
            normalize_class(" Chrome_WidgetWin_1 "),
            "chrome_widgetwin_1"
        );
    }

    #[test]
    fn config_resolve_uses_locator_outside_dev_shell() {
        let env = MapEnv::new(&[(ENV_CONFIG, "env.toml")]);
        let loc = FixedLocator(PathBuf::from("target/floor.toml"));
        let p = resolve_paths(Some(Path::new("cli.toml")), &env, &loc);
        assert_eq!(
            p.floor,
            FloorSource::Default(PathBuf::from("target/floor.toml"))
        );
        assert_eq!(p.env_overlay, Some(PathBuf::from("env.toml")));
        assert_eq!(p.edit_target(), Some(PathBuf::from("cli.toml")));
        let p = resolve_paths(None, &env, &loc);
        assert_eq!(p.edit_target(), Some(PathBuf::from("env.toml")));
        let p = resolve_paths(None, &MapEnv::default(), &loc);
        assert_eq!(p.edit_target(), Some(PathBuf::from("target/floor.toml")));
    }

    #[test]
    fn config_dev_shell_never_touches_default_location() {
        // TripwireLocator は呼ばれたら panic する
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        let p = resolve_paths(None, &env, &TripwireLocator);
        assert!(matches!(p.floor, FloorSource::Absent(_)));
        assert!(p.dev_mode);
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1"), (ENV_DEV_FLOOR, "target/x.toml")]);
        let p = resolve_paths(None, &env, &TripwireLocator);
        assert_eq!(
            p.floor,
            FloorSource::DevFloor(PathBuf::from("target/x.toml"))
        );
        // 値が "0" や空でも、変数があれば開発シェル (既定の場所を引かない)
        for v in ["0", ""] {
            let p = resolve_paths(None, &MapEnv::new(&[(ENV_DEV_FORBID, v)]), &TripwireLocator);
            assert!(p.dev_mode, "{v:?}");
        }
    }

    #[test]
    fn config_save_keeps_unchanged_multiline_array_comments() {
        let dir = test_dir("multiline");
        let f = dir.join("config.toml");
        let original = "version = 1\n[deny]\nprocesses = [\n  \"cmd\", # シェル\n  \"powershell\", # これも\n]\n";
        std::fs::write(&f, original).unwrap();
        let mut cfg = Config::load(&f).unwrap();
        cfg.general.capture_enabled = true;
        save(&f, &cfg).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(
            text.contains("# シェル") && text.contains("# これも"),
            "{text}"
        );
        assert_eq!(Config::load(&f).unwrap(), cfg);
    }

    #[test]
    fn config_missing_floor_is_closed_layer_but_missing_overlay_is_error() {
        let dir = test_dir("missing");
        let paths = ConfigPaths {
            floor: FloorSource::Default(dir.join("nope.toml")),
            env_overlay: None,
            cli_overlay: None,
            dev_mode: false,
        };
        let layers = load_layers(&paths).unwrap();
        assert_eq!(layers.len(), 1);
        assert!(!layers[0].general.input_enabled);
        let paths = ConfigPaths {
            cli_overlay: Some(dir.join("nope-cli.toml")),
            ..paths
        };
        assert!(load_layers(&paths).is_err());
    }

    /// 層の役割 (D-079): 床のファイルが無ければ NoFloor、場所を引けなければ Unresolved、追加の層は Overlay
    #[test]
    fn load_layers_marks_missing_floor_file_and_overlays() {
        let dir = test_dir("roles");
        let missing = ConfigPaths {
            floor: FloorSource::Default(dir.join("nope.toml")),
            env_overlay: None,
            cli_overlay: None,
            dev_mode: false,
        };
        let layers = load_layers(&missing).unwrap();
        assert_eq!(layers[0].role, LayerRole::NoFloor);
        let absent = ConfigPaths {
            floor: FloorSource::Absent("test".into()),
            ..missing.clone()
        };
        assert_eq!(load_layers(&absent).unwrap()[0].role, LayerRole::Unresolved);
        let floor = dir.join("floor.toml");
        let env = dir.join("env.toml");
        let cli = dir.join("cli.toml");
        for f in [&floor, &env, &cli] {
            std::fs::write(f, "version = 1\n").unwrap();
        }
        let all = ConfigPaths {
            floor: FloorSource::DevFloor(floor),
            env_overlay: Some(env),
            cli_overlay: Some(cli),
            dev_mode: true,
        };
        let roles: Vec<LayerRole> = load_layers(&all).unwrap().iter().map(|l| l.role).collect();
        assert_eq!(
            roles,
            vec![LayerRole::Floor, LayerRole::Overlay, LayerRole::Overlay]
        );
        // 床なしでも追加の層は Overlay
        let no_floor_with_overlay = ConfigPaths {
            floor: FloorSource::Default(dir.join("nope.toml")),
            ..all
        };
        let roles: Vec<LayerRole> = load_layers(&no_floor_with_overlay)
            .unwrap()
            .iter()
            .map(|l| l.role)
            .collect();
        assert_eq!(
            roles,
            vec![LayerRole::NoFloor, LayerRole::Overlay, LayerRole::Overlay]
        );
    }

    #[test]
    fn config_broken_floor_is_error_not_default() {
        let dir = test_dir("broken");
        let f = dir.join("floor.toml");
        std::fs::write(&f, "version = 1\n[general\n").unwrap();
        let paths = ConfigPaths {
            floor: FloorSource::Default(f),
            env_overlay: None,
            cli_overlay: None,
            dev_mode: false,
        };
        assert!(load_layers(&paths).is_err());
    }

    #[test]
    fn config_save_preserves_comments_and_makes_backup() {
        let dir = test_dir("save");
        let f = dir.join("config.toml");
        let original = "# 人間のコメント\nversion = 1\n\n[general]\ninput_enabled = false # 行末のコメント\n\n[deny]\n# deny の説明\nprocesses = [\"cmd\"]\n";
        std::fs::write(&f, original).unwrap();
        let mut cfg = Config::load(&f).unwrap();
        cfg.general.input_enabled = true;
        cfg.deny.processes.push("powershell".into());
        save(&f, &cfg).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("# 人間のコメント"), "{text}");
        assert!(text.contains("# 行末のコメント"), "{text}");
        assert!(text.contains("# deny の説明"), "{text}");
        assert!(text.contains("input_enabled = true"), "{text}");
        assert_eq!(Config::load(&f).unwrap(), cfg);
        let bak = std::fs::read_to_string(dir.join("config.toml.bak")).unwrap();
        assert_eq!(bak, original);
    }

    #[test]
    fn config_save_creates_from_template() {
        let dir = test_dir("create");
        let f = dir.join("new").join("config.toml");
        let mut cfg = Config::default();
        cfg.allow.keys.push("ctrl+s".into());
        save(&f, &cfg).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("# ai-cu の設定ファイル"));
        assert_eq!(Config::load(&f).unwrap(), cfg);
    }
}
