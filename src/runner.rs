//! 実行の一連の流れ (D-044)。CLI の `run` と TUI の実行が、同じ順序で同じ安全機構を通るように 1 か所に束ねる。
//! 装置は `DeviceFactory` 越しにだけ作る (テストはモック)。

use std::path::{Path, PathBuf};

use crate::backend::{RealIndicator, RealInputLock, RealSink, Win32Desktop};
use crate::config::{ConfigPaths, ENV_STOP_FILE, EnvLookup, load_layers};
use crate::device::{
    BackendError, Desktop, Indicator, InputLock, InputSink, SystemClock, WindowInfo,
};
use crate::error::{AppError, ErrorKind};
use crate::estop::{SentinelStop, StopSet, StopSource};
use crate::geometry::Rect;
use crate::hint::Surface;
use crate::plan::{Step, Target};
use crate::policy::Policy;
use crate::run::{ExecOptions, Executor, ImageWriter, RunReport, backend_error, validation_error};
use crate::script::Script;
use crate::validate::{Validation, validate};

/// 実際の装置を作る口。テストではモックを返すか、呼ばれたら失敗させる。
pub trait DeviceFactory {
    fn input(&mut self, env: &dyn EnvLookup) -> Result<Box<dyn InputSink>, BackendError>;
    fn desktop(&mut self) -> Box<dyn Desktop>;
    /// 仮想デスクトップの矩形 (読み取りだけ)。`validate` と `--dry-run` が座標の範囲を見るのに使う (D-058)。
    /// 読めなければ None (範囲は実行の前の検証で見る)
    fn screen(&mut self) -> Option<Rect>;
    /// 画面の縁の枠と予告の音 (D-093)。工場が持つ枠を借りる取っ手を返す (MCP の常駐で枠を持ち続けるため)。
    /// MCP の枠の延長と状態の読みに使う。実行器に渡すのは `indicator_and_lock`
    fn indicator(&mut self) -> Box<dyn Indicator + '_>;
    /// 実行器に渡す枠と入力の錠 (D-113) を 1 回で借りる。別々に借りると `devices` の可変の借用が重なるため
    fn indicator_and_lock(&mut self) -> (Box<dyn Indicator + '_>, Box<dyn InputLock + '_>);
}

/// MCP の常駐で、実行の後に枠を残す時間 (ミリ秒。D-093、D-107)。その間の次の実行は予告の間を省く。
/// 出ている枠は、装置に触れる MCP の道具の呼び出しの始めと終わりにこの長さへ延ばす (`Indicator::extend`。
/// 設定の `indicator.enabled` が偽なら延ばさない)。
/// CLI と TUI は 0 (プロセスの終わりで消える / カウントダウンを持つ)。止められた実行の後は実行器が 0 にする
pub const MCP_OVERLAY_LINGER_MS: u64 = 30000;

/// 実際の装置の工場。枠 (`RealIndicator`) はプロセスの間持ち続ける (MCP の常駐で残す時間を効かせる。D-093)。
/// 入力の錠 (`RealInputLock`) も持ち続ける (名前付きミューテックスのハンドルを最初の試みで作り、閉じずに使い回す。D-113)
#[derive(Default)]
pub struct RealDevices {
    indicator: RealIndicator,
    input_lock: RealInputLock,
}

impl RealDevices {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入力の錠の名前を差し替えた工場 (実機のテスト用。pid を入れた名前を渡し、利用者の ai-cu の錠を握らない。D-113)
    pub fn with_input_lock_name(name: &str) -> Self {
        Self {
            indicator: RealIndicator::default(),
            input_lock: RealInputLock::with_name(name),
        }
    }

    /// 枠を出せない警告と、入力の錠を解けない警告の書き先を替える (MCP では `mcp::log` にして
    /// `--log` のファイルにも載せる。D-061、D-113)
    pub fn set_warn_sink(&mut self, sink: fn(&str)) {
        self.indicator.set_warn_sink(sink);
        self.input_lock.set_warn_sink(sink);
    }
}

impl DeviceFactory for RealDevices {
    fn input(&mut self, env: &dyn EnvLookup) -> Result<Box<dyn InputSink>, BackendError> {
        Ok(Box::new(RealSink::new(env)?))
    }

    fn desktop(&mut self) -> Box<dyn Desktop> {
        Box::new(Win32Desktop::new())
    }

    fn screen(&mut self) -> Option<Rect> {
        Win32Desktop::new().virtual_screen().ok()
    }

    /// 工場が持つ実の枠を借りる (枠のスレッドは最初の `show` で立つ)
    fn indicator(&mut self) -> Box<dyn Indicator + '_> {
        Box::new(&mut self.indicator)
    }

    /// 工場が持つ実の枠と入力の錠を、別々のフィールドとして分けて借りる (D-093、D-113)
    fn indicator_and_lock(&mut self) -> (Box<dyn Indicator + '_>, Box<dyn InputLock + '_>) {
        (
            Box::new(&mut self.indicator),
            Box::new(&mut self.input_lock),
        )
    }
}

/// 物理ピクセルの座標を扱うには、DPI 認識の宣言が要る。失敗していたら装置に触れない。
pub fn require_dpi(dpi: &Result<(), String>) -> Result<(), AppError> {
    dpi.as_ref().map_err(|e| {
        AppError::new(
            ErrorKind::OsError,
            format!(
                "DPI 認識 (Per-Monitor V2) を宣言できないので、物理ピクセルの座標を扱えない: {e}"
            ),
        )
    })?;
    Ok(())
}

/// 停止の目印のファイル: 設定の各層のものと、環境変数 `AI_CU_STOP_FILE` (足すことしかできない。D-032)。
pub fn sentinel_paths(policy: &Policy, env: &dyn EnvLookup) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = policy
        .sentinel_files()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if let Some(f) = env.var(ENV_STOP_FILE).filter(|v| !v.trim().is_empty()) {
        // 呼び出し側の作業ディレクトリで解決し、絶対パスにして持つ (実行中に作業ディレクトリが変わっても同じ場所を見る)
        let p = PathBuf::from(f);
        files.push(std::path::absolute(&p).unwrap_or(p));
    }
    files
}

pub fn stop_set(policy: &Policy, env: &dyn EnvLookup) -> StopSet {
    let mut stops = StopSet::new();
    stops.push(Box::new(SentinelStop::new(sentinel_paths(policy, env))));
    stops
}

/// ファイルから操作列を読む。読めない、解析できない、形が違うときは `invalid_script`。
pub fn read_script_file(path: &Path) -> Result<Script, AppError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        AppError::new(
            ErrorKind::InvalidScript,
            format!("{} を読めない: {e}", path.display()),
        )
    })?;
    Script::from_json(&text).map_err(|e| AppError::new(ErrorKind::InvalidScript, e.to_string()))
}

/// 判定に使う層を読んで方針にする。床が無ければ何も許可しない層になる (D-018)。
pub fn load_policy(paths: &ConfigPaths) -> Result<Policy, AppError> {
    let layers = load_layers(paths).map_err(|e| AppError::new(ErrorKind::Config, e.to_string()))?;
    Ok(Policy::new(layers))
}

/// 1 回の実行に付ける除外と停止要因。
pub struct RunOptions<'a> {
    /// TUI からの実行で除外する自分の端末 (D-045)
    pub self_window: Option<&'a WindowInfo>,
    /// ai-cu を起動した側のプロセス (設定にかかわらず対象にしない。D-055)
    pub host_pids: &'a [u32],
    /// 失敗の次の手 (hint) の文を選ぶ呼ばれ方 (D-056)
    pub surface: Surface,
    /// この実行に足す停止要因 (MCP の取り消しなど。D-055)。設定の目印のファイルと環境変数のものに加わる
    pub extra_stops: Vec<Box<dyn StopSource>>,
    /// 実行の後に枠を残す時間 (ミリ秒。D-093)。既定 0。止められた実行の後は実行器が 0 にする
    pub linger_ms: u64,
    /// 予告の間を省く (TUI はカウントダウンを持つので。D-093)。枠は出す。既定 false
    pub skip_lead: bool,
}

impl RunOptions<'_> {
    pub fn none() -> Self {
        RunOptions {
            self_window: None,
            host_pids: &[],
            surface: Surface::Cli,
            extra_stops: Vec::new(),
            linger_ms: 0,
            skip_lead: false,
        }
    }
}

/// 画面つきで検証し、通ったときだけ入力の装置を作り (開発シェルならここで拒否される)、停止要因を束ねて、
/// 実行器で走らせる (実行器は冒頭でもう一度検証する)。許可の無い設定では、入力の装置を作らない。
pub fn execute(
    script: &Script,
    policy: &Policy,
    desktop: &dyn Desktop,
    opts: RunOptions,
    env: &dyn EnvLookup,
    devices: &mut dyn DeviceFactory,
    images: &mut dyn ImageWriter,
) -> Result<RunReport, AppError> {
    let screen = desktop.virtual_screen().map_err(backend_error)?;
    if let Some(e) = validation_error(&validate(script, policy, Some(screen))) {
        return Err(e);
    }
    let mut sink = devices.input(env).map_err(backend_error)?;
    // 枠と入力の錠は入力の装置を作れた (開発シェルで断られなかった) 後に借りる (D-093、D-113)
    let (mut indicator, mut input_lock) = devices.indicator_and_lock();
    let mut clock = SystemClock::new();
    let mut stops = stop_set(policy, env);
    for s in opts.extra_stops {
        stops.push(s);
    }
    Executor {
        policy,
        sink: sink.as_mut(),
        desktop,
        clock: &mut clock,
        stops: &mut stops,
        images,
        indicator: indicator.as_mut(),
        input_lock: input_lock.as_mut(),
    }
    .execute_on(
        script,
        &ExecOptions {
            self_window: opts.self_window,
            host_pids: opts.host_pids,
            surface: opts.surface,
            linger_ms: opts.linger_ms,
            skip_lead: opts.skip_lead,
        },
    )
}

/// 最初のバッチの届く先 (確認の画面で、前面の注意を出し分ける)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstBatch {
    Mouse,
    Keyboard,
    /// 先頭が `focus` (対象を前面にしてから送る。D-050)
    Focus,
    /// 先頭が `close` (前面は変えず、対象に WM_CLOSE を置く。D-062)
    Close,
}

/// 事前検査の結果 (TUI の確認の画面の材料)。装置には触れていない。
#[derive(Debug, Clone, PartialEq)]
pub struct PreflightReport {
    pub path: PathBuf,
    /// 解析済みの操作列。実行にはこれをそのまま渡す (見たものが走る)
    pub script: Script,
    /// 保存済みの層で、画面の大きさつきで検証した結果
    pub validation: Validation,
    pub batches: usize,
    pub first_batch: Option<FirstBatch>,
    pub wait_total_ms: u64,
    pub screenshots: usize,
    /// `close` の数 (ウィンドウを閉じる。D-062)
    pub closes: usize,
    /// 判定に使った層の出所 (床と追加の層)
    pub layer_sources: Vec<String>,
    pub max_duration_ms: u64,
    pub mouse_threshold_px: u32,
    pub sentinel_files: Vec<String>,
    /// 実入力が有効で、アプリ、クラス、領域の許可リストがどの層でも空 (どのアプリにも送れる)
    pub unrestricted: bool,
}

impl PreflightReport {
    /// 解析済みの操作列と方針から、確認の画面の材料を組み立てる (装置には触れない)。
    pub fn build(
        path: &Path,
        script: Script,
        policy: &Policy,
        screen: Option<Rect>,
        sentinel_files: Vec<String>,
    ) -> PreflightReport {
        let validation = validate(&script, policy, screen);
        let (batches, first_batch, wait_total_ms, screenshots) = summarize(&validation);
        let closes = validation
            .plans
            .iter()
            .flatten()
            .filter(|s| matches!(s, Step::Close { .. }))
            .count();
        PreflightReport {
            path: path.to_path_buf(),
            script,
            validation,
            batches,
            first_batch,
            wait_total_ms,
            screenshots,
            closes,
            layer_sources: policy.layers.iter().map(|l| l.source.clone()).collect(),
            max_duration_ms: policy.max_duration_ms(),
            mouse_threshold_px: policy.mouse_threshold_px(),
            sentinel_files,
            unrestricted: policy.unrestricted(),
        }
    }

    pub fn actions(&self) -> usize {
        self.script.actions.len()
    }

    pub fn runnable(&self) -> bool {
        self.validation.ok()
    }
}

/// TUI に注入する実行の口 (D-044)。テストではモック。
pub trait ScriptRunner {
    /// 読む → 解析 → 保存済みの層で画面つきの検証 → 要約。入力の装置は作らない
    fn preflight(&mut self, path: &Path) -> Result<PreflightReport, AppError>;
    /// 前面のウィンドウ (読み取りだけ)
    fn foreground(&mut self) -> Option<WindowInfo>;
    /// 実入力。CLI の `run` と同じ経路で、`self_window` には送らない
    fn run(
        &mut self,
        script: &Script,
        self_window: Option<&WindowInfo>,
    ) -> Result<RunReport, AppError>;
}

pub struct RealRunner<'a> {
    pub paths: ConfigPaths,
    pub env: &'a dyn EnvLookup,
    pub devices: &'a mut dyn DeviceFactory,
    pub images: &'a mut dyn ImageWriter,
    /// DPI 認識の宣言の結果 (失敗なら事前検査も実行も拒否する)
    pub dpi: Result<(), String>,
    /// ai-cu を起動した側のプロセス (設定にかかわらず対象にしない。D-055)
    pub host_pids: Vec<u32>,
}

fn summarize(v: &Validation) -> (usize, Option<FirstBatch>, u64, usize) {
    let steps = v.plans.iter().flatten();
    let batches = steps
        .clone()
        .filter(|s| matches!(s, Step::Batch { .. }))
        .count();
    let first = steps.clone().find_map(|s| match s {
        Step::Batch {
            target: Target::Mouse { .. },
            ..
        } => Some(FirstBatch::Mouse),
        Step::Batch {
            target: Target::Keyboard,
            ..
        } => Some(FirstBatch::Keyboard),
        Step::Focus { .. } => Some(FirstBatch::Focus),
        Step::Close { .. } => Some(FirstBatch::Close),
        _ => None,
    });
    let wait: u64 = v.plans.iter().map(|p| crate::plan::total_sleep_ms(p)).sum();
    let shots = steps.filter(|s| matches!(s, Step::Capture { .. })).count();
    (batches, first, wait, shots)
}

impl ScriptRunner for RealRunner<'_> {
    fn preflight(&mut self, path: &Path) -> Result<PreflightReport, AppError> {
        require_dpi(&self.dpi)?;
        let policy = load_policy(&self.paths)?;
        let script = read_script_file(path)?;
        // 画面の大きさ (読み取りだけ) を使って、実行器と同じ判定にする
        let desktop = self.devices.desktop();
        let screen = desktop.virtual_screen().ok();
        let sentinel_files = sentinel_paths(&policy, self.env)
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        Ok(PreflightReport::build(
            path,
            script,
            &policy,
            screen,
            sentinel_files,
        ))
    }

    fn foreground(&mut self) -> Option<WindowInfo> {
        self.devices.desktop().foreground_window().ok().flatten()
    }

    fn run(
        &mut self,
        script: &Script,
        self_window: Option<&WindowInfo>,
    ) -> Result<RunReport, AppError> {
        require_dpi(&self.dpi)?;
        // 方針は常に保存済みのファイルから作る (編集中の設定は使わない。D-043)
        let policy = load_policy(&self.paths)?;
        let desktop = self.devices.desktop();
        execute(
            script,
            &policy,
            desktop.as_ref(),
            RunOptions {
                self_window,
                host_pids: &self.host_pids,
                surface: Surface::Cli,
                extra_stops: Vec::new(),
                // TUI は実行前のカウントダウン (D-043) を持つので、枠は出すが予告の間は省く。枠は残さない (D-093)
                linger_ms: 0,
                skip_lead: true,
            },
            self.env,
            self.devices,
            self.images,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::config::{ENV_DEV_FORBID, ENV_STOP_FILE, FloorSource, MapEnv};
    use crate::error::ErrorKind;
    use crate::run::SELF_WINDOW_HINT;
    use crate::testing::{
        DesktopOnly, MemoryImages, MockDevices, NoDevices, World, unique_temp_dir, window,
    };

    const ENABLED: &str = "[general]\ninput_enabled = true\ncapture_enabled = true\n";
    const CLICK: &str = r#"{"version":1,"actions":[{"type":"click","x":10,"y":10}]}"#;
    const KEY: &str = r#"{"version":1,"actions":[{"type":"key","keys":"a"}]}"#;

    fn floor_file(dir: &Path, body: &str) -> PathBuf {
        let f = dir.join("floor.toml");
        std::fs::write(&f, format!("version = 1\n{body}")).unwrap();
        f
    }

    fn script_file(dir: &Path, body: &str) -> PathBuf {
        let f = dir.join("steps.json");
        std::fs::write(&f, body).unwrap();
        f
    }

    fn paths(floor: &Path) -> ConfigPaths {
        ConfigPaths {
            floor: FloorSource::DevFloor(floor.to_path_buf()),
            env_overlay: None,
            cli_overlay: None,
            dev_mode: true,
        }
    }

    /// 既定の工場の入力の錠は `INPUT_LOCK_NAME` を使う (錠は開かずに名前だけ読む)。差し替えた名前はそのまま持つ (D-113)
    #[test]
    fn real_devices_default_input_lock_name() {
        use crate::win32::input_mutex::INPUT_LOCK_NAME;
        assert_eq!(RealDevices::new().input_lock.name(), INPUT_LOCK_NAME);
        assert_eq!(INPUT_LOCK_NAME, "Local\\ai-cu-input");
        let name = format!("Local\\ai-cu-input-test-{}-name", std::process::id());
        assert_eq!(
            RealDevices::with_input_lock_name(&name).input_lock.name(),
            name
        );
    }

    #[test]
    fn runner_preflight_is_device_free_and_summarizes_the_plan() {
        let dir = unique_temp_dir("runner-preflight");
        let floor = floor_file(&dir, ENABLED);
        let script = script_file(
            &dir,
            r#"{"version":1,"actions":[{"type":"click","x":10,"y":10},{"type":"key","keys":"ctrl+s"},{"type":"wait","ms":100},{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":4,"h":3}}]}"#,
        );
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1"), (ENV_STOP_FILE, "C:\\x\\STOP")]);
        // 入力の装置を作ろうとしたら失敗する工場。画面の大きさは読む
        let mut devices = DesktopOnly(World::new().shared());
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut devices,
            images: &mut images,
            dpi: Ok(()),
            host_pids: vec![],
        };
        let report = r.preflight(&script).unwrap();
        assert!(report.runnable(), "{:?}", report.validation.issues);
        assert_eq!(report.actions(), 4);
        assert_eq!(report.batches, 2);
        assert_eq!(report.first_batch, Some(FirstBatch::Mouse));
        assert!(report.wait_total_ms >= 100, "{}", report.wait_total_ms);
        assert_eq!(report.screenshots, 1);
        assert_eq!(report.layer_sources, vec![floor.display().to_string()]);
        assert!(report.unrestricted, "許可リストが空なら赤の警告の材料");
        assert_eq!(report.max_duration_ms, 600_000);
        assert_eq!(report.mouse_threshold_px, 50);
        assert_eq!(report.sentinel_files, vec!["C:\\x\\STOP".to_string()]);
        assert_eq!(report.path, script);
        assert_eq!(
            report.script.actions.len(),
            4,
            "解析済みの操作列をそのまま持つ"
        );
    }

    #[test]
    fn runner_preflight_reports_policy_issues_as_not_runnable() {
        let dir = unique_temp_dir("runner-preflight-denied");
        let floor = floor_file(&dir, "");
        let script = script_file(&dir, KEY);
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        let mut devices = DesktopOnly(World::new().shared());
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut devices,
            images: &mut images,
            dpi: Ok(()),
            host_pids: vec![],
        };
        let report = r.preflight(&script).unwrap();
        assert!(!report.runnable());
        assert_eq!(report.first_batch, Some(FirstBatch::Keyboard));
        assert!(
            !report.unrestricted,
            "実入力が無効なら unrestricted ではない"
        );
        assert_eq!(
            report.validation.error_kind(),
            Some(ErrorKind::PolicyDenied)
        );
    }

    #[test]
    fn runner_preflight_requires_dpi_and_rejects_unreadable_script() {
        let dir = unique_temp_dir("runner-preflight-errs");
        let floor = floor_file(&dir, ENABLED);
        let script = script_file(&dir, CLICK);
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut NoDevices,
            images: &mut images,
            dpi: Err("test".into()),
            host_pids: vec![],
        };
        assert_eq!(r.preflight(&script).unwrap_err().kind, ErrorKind::OsError);
        r.dpi = Ok(());
        let mut devices = DesktopOnly(World::new().shared());
        r.devices = &mut devices;
        let e = r.preflight(&dir.join("missing.json")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidScript);
        std::fs::write(dir.join("bad.json"), "{").unwrap();
        let e = r.preflight(&dir.join("bad.json")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidScript);
        // 床が壊れていれば設定エラー
        std::fs::write(&floor, "version = 1\n[general\n").unwrap();
        let e = r.preflight(&script).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Config);
    }

    #[test]
    fn runner_run_in_dev_shell_exits_9_and_sends_nothing() {
        let dir = unique_temp_dir("runner-dev");
        let floor = floor_file(&dir, ENABLED);
        let env = MapEnv::new(&[(ENV_DEV_FORBID, "1")]);
        let world = World::new().shared();
        let mut devices = MockDevices(world.clone());
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut devices,
            images: &mut images,
            dpi: Ok(()),
            host_pids: vec![],
        };
        let s = Script::from_json(CLICK).unwrap();
        assert_eq!(
            r.run(&s, None).unwrap_err().kind,
            ErrorKind::ForbiddenDevEnv
        );
        assert!(world.borrow().sent.is_empty());
    }

    #[test]
    fn runner_run_uses_saved_layers_only_and_requires_dpi() {
        // 床が実入力を許していなければ、どんな操作列も 4 で止まり、装置に触れない
        let dir = unique_temp_dir("runner-saved");
        let floor = floor_file(&dir, "");
        let env = MapEnv::default();
        let world = World::new().shared();
        let mut devices = DesktopOnly(world.clone());
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut devices,
            images: &mut images,
            dpi: Ok(()),
            host_pids: vec![],
        };
        let s = Script::from_json(CLICK).unwrap();
        assert_eq!(r.run(&s, None).unwrap_err().kind, ErrorKind::PolicyDenied);
        assert!(world.borrow().sent.is_empty());
        r.dpi = Err("test".into());
        assert_eq!(r.run(&s, None).unwrap_err().kind, ErrorKind::OsError);
    }

    #[test]
    fn runner_run_with_mock_devices_reports_and_excludes_self_window() {
        let dir = unique_temp_dir("runner-run");
        let floor = floor_file(&dir, ENABLED);
        let env = MapEnv::default();
        let world = World::new().shared();
        let mut devices = MockDevices(world.clone());
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut devices,
            images: &mut images,
            dpi: Ok(()),
            host_pids: vec![],
        };
        let report = r.run(&Script::from_json(CLICK).unwrap(), None).unwrap();
        assert_eq!(report.executed, 1);
        assert_eq!(world.borrow().sent.len(), 1);
        // 武装の瞬間の前面 (自分の端末) を渡すと、キーは前面に届くので 7 で何も送らない
        let me = window("WindowsTerminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS");
        world.borrow_mut().foreground = Some(me.clone());
        let e = r
            .run(&Script::from_json(KEY).unwrap(), Some(&me))
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(e.hint.as_deref(), Some(SELF_WINDOW_HINT));
        assert_eq!(world.borrow().sent.len(), 1, "増えていない");
        // 前面の取得は読み取りだけで、装置の観測をそのまま返す
        assert_eq!(r.foreground(), Some(me));
    }

    /// TUI からの実行は枠を出すが予告の間を省く (カウントダウン D-043 がある)。枠の見え方を読まず、音も鳴らさず、
    /// 枠を残さない (D-093)
    #[test]
    fn runner_tui_run_shows_overlay_without_lead_or_sound() {
        let dir = unique_temp_dir("runner-tui-overlay");
        let floor = floor_file(&dir, &format!("{ENABLED}[indicator]\nsound = true\n"));
        let env = MapEnv::default();
        let world = World::new().shared();
        // 枠は出ていない: 予告を省かなければ 3 秒待って音を鳴らすはずの状態
        world.borrow_mut().indicator_shown_for = None;
        let mut devices = MockDevices(world.clone());
        let mut images = MemoryImages::default();
        let mut r = RealRunner {
            paths: paths(&floor),
            env: &env,
            devices: &mut devices,
            images: &mut images,
            dpi: Ok(()),
            host_pids: vec![],
        };
        let report = r.run(&Script::from_json(CLICK).unwrap(), None).unwrap();
        assert_eq!(report.executed, 1);
        let w = world.borrow();
        assert_eq!(w.indicator_reads, 0);
        assert_eq!(w.beeps, 0);
        assert_eq!(w.shows.len(), 1);
        assert_eq!(w.releases, vec![0]);
    }

    #[test]
    fn runner_sentinel_paths_combine_policy_and_env() {
        let dir = unique_temp_dir("runner-sentinel");
        let floor = floor_file(
            &dir,
            &format!("{ENABLED}[stop]\nsentinel_file = \"C:\\\\a\\\\STOP\"\n"),
        );
        let layers = crate::config::load_layers(&paths(&floor)).unwrap();
        let policy = Policy::new(layers);
        let env = MapEnv::new(&[(ENV_STOP_FILE, "C:\\b\\STOP")]);
        let files = sentinel_paths(&policy, &env);
        assert_eq!(
            files,
            vec![PathBuf::from("C:\\a\\STOP"), PathBuf::from("C:\\b\\STOP")]
        );
        assert!(sentinel_paths(&policy, &MapEnv::default()).len() == 1);
    }
}
