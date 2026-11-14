//! 操作列の実行器。検証、停止要因の確認、フォアグラウンドガード、送信、押下の後始末を担う。
//! 装置には `InputSink` と `Desktop` の trait 越しにだけ触れる (テストはモック)。

use serde::Serialize;

use crate::device::{
    BackendError, Clock, Desktop, HelperOutcome, Image, Indicator, InputDesktop, InputEvent,
    InputLock, InputSink, StackWindow, TopWindow, WindowInfo, WindowPresence,
};
use crate::elements::{
    ElementQuery, UiElement, element_json, first_text, normalize_newlines, parse_control_type,
};
use crate::error::{AppError, ErrorKind, reason};
use crate::estop::{StopReason, StopSet};
use crate::geometry::{Point, Rect, bounding, visible_parts};
use crate::hint::{Hint, Surface, hint};
use crate::plan::{
    CLOSE_SETTLE_MS, FOCUS_SETTLE_MS, OCCLUDED_GRACE_MS, Step, Target, Until, WAIT_FOR_POLL_MS,
    WAIT_SCREEN_POLL_MS,
};
use crate::policy::Policy;
use crate::script::{Button, ElementFilter, Script, Settle, WindowTarget};
use crate::validate::validate;

/// フォアグラウンドガードの次の手 (マウス): 何も送っていない
pub const MOUSE_GUARD_HINT: &str = "何も送っていない。クリックする点の下に、許可されていないウィンドウ (タスクバーや別のアプリ) がある。先に focus で対象を前面にし (一手ずつなら `ai-cu focus`、操作列なら先頭に focus)、`ai-cu windows` の rect の中の点を使う。focus が 11 なら、人間に前面に出してもらう";
/// フォアグラウンドガードの次の手 (キー): 何も送っていない
pub const KEYBOARD_GUARD_HINT: &str = "何も送っていない。キーと文字は前面のウィンドウに届くが、前面が許可されたウィンドウではない。先に focus で対象を前面にしてから送る (一手ずつなら `ai-cu focus`、操作列なら先頭に focus)。focus が 11 なら、人間に前面に出してもらう";
/// 管理者権限のウィンドウ
pub const ELEVATED_HINT: &str = "何も送っていない。管理者権限のウィンドウ (またはそう判定できないウィンドウ) には送れない。人間に操作してもらう";
/// ai-cu 自身を動かしている端末のウィンドウ (TUI からの実行で除外する。D-045)
pub const SELF_WINDOW_HINT: &str = "何も送っていない。届く先が ai-cu を動かしている端末のウィンドウなので送らない。カウントダウンの間に対象のウィンドウを前面にするか、操作列の先頭に対象への focus を入れる";
/// focus: 対象が見つからない (D-050)
pub const FOCUS_NOT_FOUND_HINT: &str = "何も送っていない。その実行ファイル名 (と題名) の見えているウィンドウが無い。`ai-cu windows` で名前と題名を確かめる。起動した直後なら数秒待って `windows` を見直す";
/// focus: 対象が最小化されている (復元はしない)
pub const FOCUS_MINIMIZED_HINT: &str = "何も送っていない。対象は最小化されている。ai-cu は復元しない (人が別のアプリを使っていても前面を奪えてしまうため)。人間に復元してもらう";
/// focus: Windows が前面の切り替えを断った
pub const FOCUS_REFUSED_HINT: &str = "何も送っていない。Windows が前面の切り替えを断った (人が別のアプリを使っている)。繰り返さず、人間に対象を前面に出してもらう。タスクバーや Alt+Tab、他のツールで前面にしようとしない";
/// focus: 対象が許可されていない
pub const FOCUS_DENIED_HINT: &str = "何も送っていない。前面にしようとしたウィンドウが許可されていない。何を許可してほしいかを人間に伝える";
/// focus: 人が操作中
pub const FOCUS_USER_ACTIVE_HINT: &str = "何も送っていない。人がマウスやキーボードを使っている最中なので、対象を前面にしなかった (ai-cu 自身の run の直後も含む)。数秒待って 1 回だけ再試行し、だめなら人間に確かめる";
/// close: 対象が見つからない、WM_CLOSE を置けなかった (D-062)
pub const CLOSE_NOT_FOUND_HINT: &str = "何も閉じていない。その実行ファイル名と題名の見えているウィンドウが無い (もう閉じている場合を含む)。`ai-cu windows` で名前と題名を確かめる";
/// close: 対象が許可されていない (D-062)
pub const CLOSE_DENIED_HINT: &str = "何も閉じていない。閉じようとしたウィンドウが許可されていない。何を許可してほしいかを人間に伝える";
/// close: 対象がモーダルのダイアログを開いていて無効 (D-062)
pub const CLOSE_DISABLED_HINT: &str = "何も閉じていない。対象のウィンドウは無効 (多くはダイアログ (保存の確認など) を開いている) で、閉じるボタンが押せない状態なので閉じなかった。`ai-cu windows` と撮影でダイアログを確かめ、どう答えるかは人間に聞く";
/// close: 対象が前面で、人が操作中 (D-062)
pub const CLOSE_USER_ACTIVE_HINT: &str = "何も閉じていない。対象のウィンドウが前面にあり、人がマウスやキーボードを使っている最中 (ai-cu 自身の入力の直後も含む) なので閉じなかった。同じ操作列で対象に入力した直後なら、close を別の呼び出しにするか、前に 3 秒以上の wait を置く。そうでなければ数秒待って 1 回だけ再試行し、だめなら人間に確かめる";
/// ai-cu を起動した側 (エージェント、シェル、端末。親プロセスの連鎖) のウィンドウ (D-055)
pub const HOST_WINDOW_HINT: &str = "何も送っていない。届く先が ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウなので、設定にかかわらず送らない。先に focus で対象のアプリを前面にしてから送る";
/// 撮影と要素の読み取りの対象が、ai-cu を起動した側のウィンドウ (D-055)
pub const HOST_CAPTURE_HINT: &str = "何もしていない。対象が ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウなので、設定にかかわらず撮らず、読まない。別のアプリを対象にする";
/// screenshot の window: 対象が他のウィンドウに隠れている (D-053 の追記)
pub const CAPTURE_OCCLUDED_HINT: &str = "何も撮っていない。対象のウィンドウの一部が別のウィンドウに隠れていて、矩形で撮るとそのウィンドウの中身が写り込む。先に focus で対象を前面にしてから撮る (一手ずつなら `ai-cu focus PROCESS --shot PATH --window PROCESS`)";
/// 撮る矩形に、設定で拒否されたウィンドウが見えている (D-077)
pub const CAPTURE_SENSITIVE_HINT: &str = "何も撮っていない。撮る矩形に、設定で拒否されたウィンドウ (error.detail の window。見えている部分は visible) が見えている。`--region` (と全画面) なら、そのウィンドウにかからないよう `--region` を狭めるか、`--window PROCESS` で対象のウィンドウだけを撮る。`--window` で撮っていて対象の手前に重なっているなら、先に focus で対象を前面にしてから撮る (一手ずつなら `ai-cu focus PROCESS --shot PATH --window PROCESS`。focus は 1 回だけ)。そのウィンドウを閉じる・動かすのは人間に頼む。設定を緩めるよう頼まない";
/// 画面の待ち (`wait_stable` / `wait_change`) の時間切れ (15。D-081)。CLI と MCP で同じ文 (コマンド名を含めない)。
/// 15 の既定の文 (`wait_for` 向け。ウィンドウや要素の条件を見直す) とは次の手が違う
pub const SCREEN_WAIT_HINT: &str = "画面が時間内に落ち着かなかった (変わらなかった)。何も送っていない (操作列なら、それより前の操作は済んでいる。executed を見る)。同じ条件で繰り返し待たない。shot で今の画面を見る。動き続ける部分 (動画、スピナー、点滅) があるなら region を狭めるか threshold を上げる。変わるのを待っていたなら、待つ前に既に変わっていなかったか (待ち始めの画像が基準になる) を確かめる";

/// 矩形の中を間引いて見て、対象 (と対象に所有されたウィンドウ) 以外のウィンドウが上にあれば、それを返す (D-053 の追記)。
/// 画面の矩形の撮影は、そこに見えているものを撮る (ウィンドウ自身の中身ではない) ので、隠れている部分には
/// 別のアプリ (deny の端末など) の中身が写り込む。角の丸み (Windows 11) を避けて内側から見る。
/// 見つけた点も返す (隠れの 7 の `error.detail.point`。D-135)
fn occluding_window(
    desktop: &dyn Desktop,
    rect: &Rect,
    target: usize,
) -> Result<Option<(WindowInfo, Point)>, BackendError> {
    const INSET: i64 = 8;
    const STEP: usize = 64;
    let (x0, y0) = (i64::from(rect.x) + INSET, i64::from(rect.y) + INSET);
    let x1 = i64::from(rect.x) + i64::from(rect.w) - 1 - INSET;
    let y1 = i64::from(rect.y) + i64::from(rect.h) - 1 - INSET;
    if x1 < x0 || y1 < y0 {
        return Ok(None);
    }
    let axis = |a: i64, b: i64| -> Vec<i64> {
        let mut v: Vec<i64> = (a..=b).step_by(STEP).collect();
        if v.last() != Some(&b) {
            v.push(b);
        }
        v
    };
    for y in axis(y0, y1) {
        for x in axis(x0, x1) {
            let p = Point::new(x as i32, y as i32);
            if let Some(w) = desktop.window_at(p)?
                && w.hwnd != target
                && desktop.owner_root(w.hwnd) != target
            {
                return Ok(Some((w, p)));
            }
        }
    }
    Ok(None)
}

/// 待ちの途中で停止要因を確かめる間隔
pub const STOP_POLL_MS: u64 = 50;

/// 予告の間の始めのこの長さの入力は、人の入力に数えない (D-093)。端末で Enter を押して CLI を起動したとき、
/// そのキーを離した入力が予告の始めの直後に届いて毎回 10 になるのを避ける (時刻の前後は推定、測っていない)
pub const LEAD_INPUT_GRACE_MS: u64 = 300;

/// 枠を見せる期限の余裕 (D-093)。期限は 予告の間 + `limits.max_duration_ms` + 残す時間 + これ
pub const OVERLAY_DEADLINE_MARGIN_MS: u64 = 2000;

/// 入力の錠 (別の ai-cu との排他) を待つ上限 (ミリ秒。D-113)。これを過ぎても取れなければ、何も送らずに 16 (`input_busy`)。
/// MCP の呼び出しがクライアントの制限時間に収まるよう固定にし、設定のキーは足さない
pub const INPUT_LOCK_WAIT_MS: u64 = 30_000;

/// 撮影した画像の保存先。テストではメモリに溜める。
pub trait ImageWriter {
    /// 保存して、保存先の表示用のパスを返す
    fn write_png(&mut self, path: &str, image: &Image) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotInfo {
    pub action_index: usize,
    pub path: String,
    /// 原点 (物理ピクセルのスクリーン座標)
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// 画像の縮尺 (D-053)。画像の点 (px, py) は画面の (x + px / scale, y + py / scale)
    pub scale: f64,
    /// `window` で指定して撮ったウィンドウ (D-053)。取り違えに気づけるよう結果に出す
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<ShotWindow>,
    /// screenshot の settle (D-081) で、時間内に落ち着いたか (false なら時間切れの最後のフレーム)。settle が無ければ省く
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled: Option<bool>,
    /// screenshot の settle (D-081) で待った時間 (ミリ秒)。settle が無ければ省く
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle_ms: Option<u64>,
}

/// 撮った (D-053)、または `wait_for` で待った (D-068) ウィンドウ
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotWindow {
    pub hwnd: usize,
    pub process: String,
    pub title: String,
}

impl ShotWindow {
    fn of(w: &WindowInfo) -> Self {
        ShotWindow {
            hwnd: w.hwnd,
            process: w.process.clone(),
            title: w.title.clone(),
        }
    }
}

/// `wait_for` が満たされた記録 (D-068)。`window` と `element` は無ければキーごと省く (null は出さない)。
/// `window` は満たされたときに見ていたウィンドウ (`absent` でウィンドウ自身が消えたなら無し)、`element` は
/// 要素か文字列の条件が「ある」で満たされたときの要素 (`center` つき。文字列は入れない)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaitInfo {
    pub action_index: usize,
    /// 待ち始めてから満たされるまで (ミリ秒)
    pub elapsed_ms: u64,
    /// 確かめた回数 (最初の判定を含む)
    pub checks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<ShotWindow>,
    /// 画面の待ち (D-081) で最後に撮った矩形 (`{x, y, w, h}`)。`wait_for` では省く
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    /// `wait_change` で満たしたときの基準との差 (変わった点の割合。小数第 4 位で丸める。D-081)。ほかでは省く
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<serde_json::Value>,
    /// `absent` が最初の判定で満たされたときの注意 (`timeout_ms` 0 は除く。D-070)。最初の確認で既に無かったことと、
    /// 次に確かめる道具 (windows / find / text) を書く。無ければキーごと省く
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// フォーカスを動かしうるキー (Tab と Enter) を押す束か。type の `\t` と改行、key の `tab` / `enter` がこれに当たり、
/// 送った後の次のキーの束でパスワード欄の判定をやり直す (D-090)
fn moves_focus(events: &[InputEvent]) -> bool {
    const VK_TAB: u16 = 0x09;
    const VK_RETURN: u16 = 0x0D;
    events.iter().any(|e| {
        matches!(e, InputEvent::Key { vk, down: true, .. } if *vk == VK_TAB || *vk == VK_RETURN)
    })
}

/// `absent` が最初の判定で満たされたときの `WaitInfo::note` の文 (D-070)。`unmet` は最初の判定で無かった段
/// (`"window"` / `"element"` / それ以外は `"text"`)。UI Automation に要素を出さないアプリ (gpui など) では、
/// 見えている要素も無いと判定されて空振りする (D-070)。コマンドの書き方は CLI と MCP で違うので、道具の名前だけを書く
fn absent_note(unmet: &str) -> &'static str {
    match unmet {
        "window" => {
            "最初の確認で既に無かった。待つ前から無かったか、実行ファイル名か題名が表示と合っていないか、最小化されている (最小化と、許可の外と ai-cu を起動した側の窓は無いと数える)。windows で対象のウィンドウを確かめる"
        }
        "element" => {
            "最初の確認で既に無かった。UI Automation に要素を出さないアプリ (gpui など) では、画面に見えていても無いと判定される。find で条件の要素 (text だけの条件なら文字列を持つ要素) が出るか確かめる"
        }
        _ => {
            "最初の確認で既に無かった。要素の文字列が最初から text を含んでいなかった。text で今の文字列を確かめる"
        }
    }
}

/// 名指しで見つからないときの message の末尾に足す文 (D-139)。hint は変えず (D-056、D-134)、名指しの範囲は
/// この 1 文で伝える
pub const NAMED_WINDOW_SCOPE_NOTE: &str =
    "許可の外と ai-cu を起動した側の窓は探さない (windows に出る窓だけを名指しできる)";

/// 名指し (`find_window`) の候補に入れてよい窓か (D-139)。`windows` の一覧に出る窓と同じ (設定の `check_window`
/// (プロセス、クラス) を通り、ai-cu を起動した側 (`host_pids`。D-055) でない) で、実行器ではさらに TUI の端末
/// (`self_hwnd`。D-045) を除く。通らない窓は無いものとして扱うので、名指しの結果が `windows` に出ない窓の有無と
/// 題名に依らない。`cli::window_allowed` はこれを `self_hwnd` なしで呼ぶ (一覧と名指しの述語を 1 か所に)
pub fn named_window_admitted(
    policy: &Policy,
    w: &WindowInfo,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
) -> bool {
    policy.check_window(w).is_ok() && !host_pids.contains(&w.pid) && self_hwnd != Some(w.hwnd)
}

/// 名指しの前の、名前だけの判定 (D-139)。窓の有無に依らないので何も漏らさない。許可の外の名前は 4 で、
/// 設定のキー (`rule`) と直し方 (`fix`) が付く (操作列の検証 (validate) の `check_process` と同じ扱い)
pub fn check_named_process(policy: &Policy, process: &str) -> Result<(), AppError> {
    policy
        .check_process(process)
        .map_err(|d| d.into_error(ErrorKind::PolicyDenied))
}

/// `find` / `text` と `wait_for` で共有する、読み取りの対象のウィンドウの判定 (D-054、D-068)。
/// 自分の端末 (`self_hwnd`。TUI からの実行) と ai-cu を起動した側 (`host_pids`) のウィンドウは、設定にかかわらず
/// 読まない (7)。設定 (プロセス、クラス) に通らなければ 4。撮影の `resolve_capture_window` は撮らない旨の文なので別
pub fn check_ui_window(
    policy: &Policy,
    w: &TopWindow,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
    shown: &str,
) -> Result<(), AppError> {
    if self_hwnd == Some(w.info.hwnd) {
        return Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "対象のウィンドウ ({shown}, process={:?}, pid={}) は ai-cu を動かしている端末なので読まない",
                w.info.process, w.info.pid
            ),
        )
        .with_hint(HOST_CAPTURE_HINT));
    }
    // ai-cu を起動した側のウィンドウは、設定にかかわらず読まない (D-055)
    if host_pids.contains(&w.info.pid) {
        return Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "対象のウィンドウ ({shown}, process={:?}, pid={}) は ai-cu を起動した側のものなので読まない",
                w.info.process, w.info.pid
            ),
        )
        .with_hint(HOST_CAPTURE_HINT));
    }
    policy
        .check_window(&w.info)
        .map_err(|d| d.into_error(ErrorKind::PolicyDenied))?;
    Ok(())
}

/// 撮る対象のウィンドウの矩形を解く (D-053)。述語 (`named_window_admitted`) を通る窓だけから探す (許可の外、
/// ai-cu を起動した側 (`host_pids`。D-055)、TUI の端末 (`self_hwnd`) の窓は無いものとして 12。D-139)。
/// 見つけた窓をフォアグラウンドガードと同じ判定 (権限など) と撮影の規則 (領域、全画面) に通し、画面からはみ出す
/// 分は切り詰める。見つからない、最小化、画面の外は 12 (何もしていない)。CLI の `shot --window` も使う
/// (`self_hwnd` は None)。次の手の文は `surface` (CLI か MCP か) で選ぶ (D-056)。
/// 隠れは即 7。猶予は操作列の `grab` の側 (D-082)
pub fn resolve_capture_window(
    desktop: &dyn Desktop,
    policy: &Policy,
    screen: Rect,
    target: &WindowTarget,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
    surface: Surface,
) -> Result<(Rect, ShotWindow), AppError> {
    match resolve_capture_window_parts(
        desktop, policy, screen, target, host_pids, self_hwnd, surface,
    )? {
        Resolved::Clear(rect, w) => Ok((rect, ShotWindow::of(&w))),
        Resolved::Occluded { cover, point, rect } => Err(occluded_error(
            desktop, target, &cover, point, &rect, surface, None,
        )),
    }
}

/// `resolve_capture_window_parts` の結果 (D-082)。隠れを、待っても変わらない他の 7 (ホストの窓) や 4 と区別して返す
enum Resolved {
    /// 隠れていない。撮影の矩形の規則 (領域、全画面) も通った。見つけたウィンドウは `WindowInfo` のまま
    /// (画面の待ちの 15 の `error.detail.window` は `window_detail` で作るので、プロセス、クラス、pid が要る。D-081)
    Clear(Rect, WindowInfo),
    /// 対象の矩形の中に別のウィンドウ (`cover`) が上にある。矩形の規則はまだ通していない
    /// (隠れの 7 を矩形の規則より先に出す)。`point` は隠れを見つけた点、`rect` は画面で切り詰めた撮る矩形
    /// (どちらも隠れの 7 の `error.detail` に使う。D-135)
    Occluded {
        cover: WindowInfo,
        point: Point,
        rect: Rect,
    },
}

/// 撮る対象の表示 (`process=..., title=...`)。message に使う
fn capture_target_shown(target: &WindowTarget) -> String {
    match &target.title {
        Some(t) => format!("process={:?}, title={t:?}", target.process),
        None => format!("process={:?}", target.process),
    }
}

/// 対象が別のウィンドウ `cover` に隠れているときの 7 (D-053 の追記)。`waited` は操作列の `grab` が猶予を使い切った
/// ときの待った時間 (D-082)。message の末尾に添えるだけで、kind と hint は同じ。
/// `error.detail` に、覆う窓の `window` (題名なし) と隠れを見つけた `point` を入れる。重なりの一覧
/// (`stack_windows`) をこの失敗のときだけ 1 回読み (deny の有無によらない)、覆う窓が一覧にあれば `see_through`
/// (deny の窓の判定で覆いに数えない窓か。隠れの判定はこの値を見ない) を、見えている部分が残れば
/// その外接矩形の `visible` (機密の 7 と同じ定義) を足す。読めなければ省き、7 のまま (D-135)
fn occluded_error(
    desktop: &dyn Desktop,
    target: &WindowTarget,
    cover: &WindowInfo,
    point: Point,
    rect: &Rect,
    surface: Surface,
    waited: Option<u64>,
) -> AppError {
    let waited = waited
        .map(|ms| format!(" ({ms} ms 待っても隠れたまま)"))
        .unwrap_or_default();
    let mut detail = serde_json::json!({ "window": window_detail(cover), "point": point });
    if let Ok(stack) = desktop.stack_windows()
        && let Some(i) = stack.iter().position(|s| s.info.hwnd == cover.hwnd)
    {
        detail["see_through"] = serde_json::json!(stack[i].see_through);
        if let Some(visible) = visible_in_stack(&stack, i, rect) {
            detail["visible"] = serde_json::json!(visible);
        }
    }
    AppError::new(
        ErrorKind::ForegroundGuard,
        format!(
            "撮る対象のウィンドウ ({}) の一部が別のウィンドウ (process={:?}) に隠れている{waited}",
            capture_target_shown(target),
            cover.process
        ),
    )
    .with_hint(hint(Hint::CaptureOccluded, surface))
    .with_detail(detail)
}

/// `resolve_capture_window` の本体。隠れの判定までを行い、隠れていれば `Resolved::Occluded` を返す (D-082)。
/// 撮影の矩形の規則 (`check_capture_rect` と `check_full_screen`) は、隠れていないときだけ通す
fn resolve_capture_window_parts(
    desktop: &dyn Desktop,
    policy: &Policy,
    screen: Rect,
    target: &WindowTarget,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
    surface: Surface,
) -> Result<Resolved, AppError> {
    let shown = capture_target_shown(target);
    // 述語を通る窓だけから探す (D-139)。通らない窓の有無と題名は、結果に表れない
    let w = desktop
        .find_window(&target.process, target.title.as_deref(), &|w| {
            named_window_admitted(policy, w, host_pids, self_hwnd)
        })
        .map_err(backend_error)?
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::WindowNotFound,
                format!("撮る対象のウィンドウが見つからない ({shown})。{NAMED_WINDOW_SCOPE_NOTE}"),
            )
        })?;
    // 以下の起動した側と許可の判定は、述語を通った窓には当たらない守りの重ね (D-139)。
    // 許可の判定は 1 回だけ評価する。最小化の 12 の detail は許可された窓のときだけ足し、返すエラーの順は
    // 従来のまま (最小化、起動した側、許可) にする (D-135)
    let allowed = policy.check_window(&w.info);
    let found = || serde_json::json!({ "window": window_detail(&w.info) });
    let rect = match (w.minimized, w.rect) {
        (false, Some(r)) => r,
        _ => {
            let err = AppError::new(
                ErrorKind::WindowNotFound,
                format!("撮る対象のウィンドウは最小化されている ({shown})。ai-cu は復元しない"),
            );
            return Err(if allowed.is_ok() {
                err.with_detail(found())
            } else {
                err
            });
        }
    };
    if host_pids.contains(&w.info.pid) {
        return Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "撮る対象のウィンドウ ({shown}, process={:?}, pid={}) は ai-cu を起動した側のものなので撮らない",
                w.info.process, w.info.pid
            ),
        )
        .with_hint(hint(Hint::HostCapture, surface))
        .with_detail(found()));
    }
    allowed.map_err(|d| d.into_error(ErrorKind::PolicyDenied))?;
    let rect = rect.intersection(&screen).ok_or_else(|| {
        AppError::new(
            ErrorKind::WindowNotFound,
            format!("撮る対象のウィンドウ {rect} が画面 {screen} の外にある ({shown})"),
        )
        .with_detail(found())
    })?;
    // 隠れている部分には別のウィンドウの中身が写り込むので、撮らない (前面にしてから撮る)
    if let Some((cover, point)) =
        occluding_window(desktop, &rect, w.info.hwnd).map_err(backend_error)?
    {
        return Ok(Resolved::Occluded { cover, point, rect });
    }
    policy
        .check_capture_rect(&rect)
        .map_err(|d| d.into_error(ErrorKind::PolicyDenied))?;
    policy
        .check_full_screen(Some(&rect), Some(&screen))
        .map_err(|d| d.into_error(ErrorKind::PolicyDenied))?;
    Ok(Resolved::Clear(rect, w.info))
}

/// 撮る矩形 `rect` に見えている、設定で拒否されたウィンドウ (D-077)。`stack` は前面から奥への Z 順。
/// 前面から順に、deny のプロセスかクラス (`check_window_deny`。allow は見ない) に当たり `rect` と重なる窓について、
/// それより前面の窓のうち覆いとみなすもの (`see_through` でない。透けうる、撮影から外れる、枠が不確かな窓と、
/// リージョンが枠をまるごと覆わない窓 (D-112) は除く) の
/// 矩形を覆いとして重なりから引き、残りがあれば
/// (その窓, 残りの外接矩形) を返す。`host_pids` (ai-cu を起動した側) と `self_hwnd` (TUI の端末) の窓は
/// deny に当たっても数えないが、覆いとしては引く。奥の窓と deny の窓自身は覆いにしない。
/// 応答しない (`hung`) deny の窓が数える窓のうちに 1 つでもあれば、ゴーストの窓 (`is_ghost_window`) をすべて
/// その窓の rule の deny の窓として数える (ゴーストは覆いのまま。元の窓とは結ばない。D-115)
pub fn sensitive_window(
    stack: &[StackWindow],
    policy: &Policy,
    rect: &Rect,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
) -> Option<(WindowInfo, Rect)> {
    sensitive_window_hit(stack, policy, rect, host_pids, self_hwnd).map(|h| (h.window, h.visible))
}

/// OS が応答しない窓の代わりに出すゴーストの窓か (D-115)。クラス名が `Ghost` (大文字小文字を区別しない)。
/// この PC の下調べでは、持ち主は dwm.exe、題名は元の題名に「(応答なし)」、矩形は元の窓と同じで、元の窓は
/// 見えたまま (cloaked でもなく) Z 順でその真後ろに残った。持ち主のプロセスは版で違いうるので条件にしない
pub fn is_ghost_window(info: &WindowInfo) -> bool {
    info.class.eq_ignore_ascii_case("Ghost")
}

/// `sensitive_window_hit` が当てた窓
struct SensitiveHit {
    window: WindowInfo,
    visible: Rect,
    /// 当たった `Denied` の `rule` (`deny.processes` か `deny.classes`。D-079)。ゴーストなら応答しない deny の窓のもの
    rule: Option<&'static str>,
    /// 応答しない deny の窓のゴーストとして当たったか (D-115)
    ghost: bool,
}

/// `sensitive_window` の本体。当たった `Denied` の `rule` (`deny.processes` か `deny.classes`。D-079) と、
/// ゴーストで当たったか (D-115) も持ち帰る (判定を 2 回しない)
fn sensitive_window_hit(
    stack: &[StackWindow],
    policy: &Policy,
    rect: &Rect,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
) -> Option<SensitiveHit> {
    let counted =
        |w: &StackWindow| self_hwnd != Some(w.info.hwnd) && !host_pids.contains(&w.info.pid);
    // 応答しない deny の窓の rule (D-115)。外側の Some は「そういう窓がある」
    let ghost_rule: Option<Option<&'static str>> = stack
        .iter()
        .filter(|w| counted(w) && w.hung)
        .find_map(|w| policy.check_window_deny(&w.info).err().map(|d| d.rule));
    for (i, w) in stack.iter().enumerate() {
        if !counted(w) {
            continue;
        }
        let (rule, ghost) = match policy.check_window_deny(&w.info) {
            Err(denied) => (denied.rule, false),
            Ok(()) => match ghost_rule {
                Some(rule) if is_ghost_window(&w.info) => (rule, true),
                _ => continue,
            },
        };
        if let Some(visible) = visible_in_stack(stack, i, rect) {
            return Some(SensitiveHit {
                window: w.info.clone(),
                visible,
                rule,
                ghost,
            });
        }
    }
    None
}

/// `stack[index]` の窓の、撮る矩形 `rect` に見えている部分の外接矩形 (D-077、D-135)。枠と `rect` の重なりから、
/// それより前面の窓のうち覆いとみなすもの (`see_through` でない) の矩形を引く。重ならない、または残らなければ None。
/// 機密の 7 (`sensitive_window_hit`) と隠れの 7 (`occluded_error`) の `visible` が共有する
fn visible_in_stack(stack: &[StackWindow], index: usize, rect: &Rect) -> Option<Rect> {
    let overlap = stack.get(index)?.rect.intersection(rect)?;
    let covers: Vec<Rect> = stack[..index]
        .iter()
        .filter(|c| !c.see_through)
        .map(|c| c.rect)
        .collect();
    bounding(&visible_parts(overlap, &covers))
}

/// 撮る直前の、機密のウィンドウの判定 (D-077)。どの層にも deny のプロセスかクラスが無ければ窓を列挙しない。
/// 見えていれば 7 で断り、`error.detail` に `window` (題名なし) と `visible` (見えている部分の外接矩形) を入れる
pub fn check_sensitive_capture(
    desktop: &dyn Desktop,
    policy: &Policy,
    rect: &Rect,
    host_pids: &[u32],
    self_hwnd: Option<usize>,
    surface: Surface,
) -> Result<(), AppError> {
    if !policy.has_window_deny() {
        return Ok(());
    }
    let stack = desktop.stack_windows().map_err(backend_error)?;
    match sensitive_window_hit(&stack, policy, rect, host_pids, self_hwnd) {
        None => Ok(()),
        // rule だけを写し、fix は付けない (設定を緩めるよう頼ませない。D-077、D-079)
        Some(SensitiveHit {
            window: w,
            visible,
            rule,
            ghost,
        }) => Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "撮る矩形 {rect} に、設定で拒否されたウィンドウ (process={:?}, class={:?}) が見えている{}",
                w.process,
                w.class,
                if ghost {
                    " (応答しない deny のウィンドウの代わりに OS が出したゴースト)"
                } else {
                    ""
                }
            ),
        )
        .with_hint(hint(Hint::CaptureSensitive, surface))
        .with_detail(serde_json::json!({ "window": window_detail(&w), "visible": visible }))
        .with_deny(rule, None)),
    }
}

/// `focus` で前面にしたウィンドウ (D-050)。対象に所有されたダイアログが前面になったときは、そのダイアログ
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FocusInfo {
    pub action_index: usize,
    pub hwnd: usize,
    pub process: String,
    pub title: String,
    /// 補助プロセス (D-057) で前面にしたか。CLI と、直接の呼び出しで足りたときは false
    pub helper: bool,
}

impl FocusInfo {
    fn new(action_index: usize, w: &WindowInfo, helper: bool) -> Self {
        FocusInfo {
            action_index,
            hwnd: w.hwnd,
            process: w.process.clone(),
            title: w.title.clone(),
            helper,
        }
    }
}

/// `close` で `WM_CLOSE` を置いたウィンドウ (D-062)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CloseInfo {
    pub action_index: usize,
    pub hwnd: usize,
    pub process: String,
    pub title: String,
    /// 待つ上限 (`CLOSE_SETTLE_MS`) のうちに見える窓から消えたか (破棄とは限らない。`hidden` を見る)。
    /// false なら保存の確認などが出ていることが多い
    pub gone: bool,
    /// `gone` が真で、窓が破棄されず同じプロセスのまま隠れただけ (トレイに入るアプリ。プロセスは動いている)。D-096
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunReport {
    pub executed: usize,
    pub screenshots: Vec<ShotInfo>,
    /// `focus` で前面にしたウィンドウ (操作の順)
    pub focused: Vec<FocusInfo>,
    /// `close` で閉じようとしたウィンドウ (操作の順)。無ければ出力に出さない
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<CloseInfo>,
    /// `wait_for` が満たされた記録 (操作の順。D-068)。無ければ出力に出さない
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waited: Vec<WaitInfo>,
    pub elapsed_ms: u64,
}

/// 検証の問題を 1 つのエラーにまとめる (問題が無ければ None)。種類は形の誤りを優先し、添字と次の手は最初の問題のもの。
pub fn validation_error(v: &crate::validate::Validation) -> Option<AppError> {
    let all: Vec<String> = v
        .issues
        .iter()
        .map(|i| match i.action_index {
            Some(n) => format!("actions[{n}]: {}", i.message),
            None => i.message.clone(),
        })
        .collect();
    first_issue_error(v, all.join(" / "))
}

/// 検証の問題を、文 `message` の 1 つのエラーにする (問題が無ければ None)。種類は形の誤りを優先し、
/// 添字、次の手、rule と fix (D-079) は最初の問題 (その種類の最初) から写す。`validation_error` と
/// CLI の `validate` / `--dry-run` / `run` の検証の失敗が共有する (写すコードを 1 つにする)
pub fn first_issue_error(v: &crate::validate::Validation, message: String) -> Option<AppError> {
    let kind = v.error_kind()?;
    let first = v.first_issue()?;
    let mut e = AppError::new(kind, message).with_deny(first.rule, first.fix.clone());
    e.action_index = first.action_index;
    e.hint = first.hint.clone();
    Some(e)
}

/// focus の失敗の `error.detail` のうち、補助プロセス (D-057) の部分。使ったか、結果、失敗の理由
/// (`failed` のときだけ。exe が起動できないなど、人が直せる原因を隠さない)
fn helper_detail(helper: Option<&HelperOutcome>) -> serde_json::Value {
    serde_json::json!({
        "helper": helper.is_some(),
        "helper_result": helper.map(HelperOutcome::label),
        "helper_error": match helper {
            Some(HelperOutcome::Failed(why)) => Some(why.as_str()),
            _ => None,
        },
    })
}

/// ガードが断ったウィンドウの `error.detail.window` (D-065)。ガードが既に読んだ値だけで、題名は入れない
fn window_detail(w: &WindowInfo) -> serde_json::Value {
    serde_json::json!({
        "process": w.process,
        "class": w.class,
        "pid": w.pid,
        "hwnd": w.hwnd,
    })
}

/// 1 つの操作のキーの束の途中で、前面が控えた pid と別のプロセスの窓に変わった (7、`target_changed`。D-100)。
/// `now` は変わった後の前面 (ガードは通っている)、`pinned_pid` は最初の束で控えた pid
fn target_changed(now: &WindowInfo, pinned_pid: u32, surface: Surface) -> AppError {
    AppError::new(
        ErrorKind::ForegroundGuard,
        format!(
            "入力の途中で前面が別のプロセスの窓に変わった (process={:?}, pid={}。最初は pid={pinned_pid})。残りは送らない",
            now.process, now.pid
        ),
    )
    .with_reason(reason::TARGET_CHANGED)
    .with_hint(hint(Hint::TargetChanged, surface))
    .with_detail(serde_json::json!({ "window": window_detail(now), "pinned_pid": pinned_pid }))
}

/// ボタンを押したままのマウスの束がガードの 7 で断られたら、次の手をドラッグの途中の文に置き換える (D-132)。
/// 個々の hint の次の手 (focus して rect の中の点を使う、など) は途中の drag に合わない。判定と `reason`、`detail` は変えない
fn mid_drag_hint(e: AppError, mid_drag: bool, target: &Target, surface: Surface) -> AppError {
    if mid_drag && matches!(target, Target::Mouse { .. }) && e.kind == ErrorKind::ForegroundGuard {
        e.with_hint(hint(Hint::DragGuardMidway, surface))
    } else {
        e
    }
}

/// 無操作の不足 (10) の `error.detail` (D-065)
fn idle_detail(idle_ms: u64, min_idle_ms: u64) -> serde_json::Value {
    serde_json::json!({ "idle_ms": idle_ms, "min_idle_ms": min_idle_ms })
}

/// 人が操作中か (D-049): 無操作の下限 `min_idle_ms` が 0 より大きく、最後の入力からの経過 `idle_ms` がそれ未満。
/// `min_idle_ms` が 0 なら無操作を見ない。実入力と撮影の門 (`shot`、`focus`、前面の `close`、MCP の cooldown)、
/// MCP の `status` の `user_active` (D-073)、実行器の予告の間の終わりの人の入力の判定 (D-093) がこの 1 つの述語を共有する
pub fn user_active(idle_ms: u64, min_idle_ms: u64) -> bool {
    min_idle_ms > 0 && idle_ms < min_idle_ms
}

pub fn backend_error(e: BackendError) -> AppError {
    let kind = match &e {
        BackendError::Forbidden(_) => ErrorKind::ForbiddenDevEnv,
        BackendError::OutOfScope(_) => ErrorKind::ForegroundGuard,
        BackendError::Os(_) => ErrorKind::OsError,
        // UIA の読み取りの時間切れも 8。終了コードと出力の形は変えない (D-104)
        BackendError::TimedOut(_) => ErrorKind::OsError,
        // GetCursorPos の 5 (画面のロックか別のデスクトップ)。名前は読んでいないので null (D-066)
        BackendError::DesktopUnavailable(_) => {
            return AppError::new(ErrorKind::DesktopUnavailable, e.to_string())
                .with_reason(reason::OTHER_DESKTOP)
                .with_detail(serde_json::json!({ "input_desktop": null }));
        }
    };
    AppError::new(kind, e.to_string())
}

/// 入力デスクトップの状態を、14 `desktop_unavailable` の失敗に写す (D-066、D-067)。使えるなら None。
/// 実行器と撮影 (`shot`) が共有する。`error.detail` は `{"input_desktop": 名前 か null}`。
/// ここでは進み具合 (D-065) を付けない。操作に入る前 (開始時) の失敗は何も送る前なので進み具合が無く、
/// 途中の撮影と focus のステップの直前の失敗には、実行器が他の途中の失敗と同じく進み具合を付ける
pub(crate) fn input_desktop_error(state: &InputDesktop) -> Option<AppError> {
    let (why, name, message) = match state {
        InputDesktop::Available => return None,
        InputDesktop::ScreenSaver { name } => (
            reason::SCREENSAVER,
            name,
            "スクリーンセーバーが動いているので何も送らない".to_string(),
        ),
        InputDesktop::Locked { name } => (
            reason::LOCKED,
            name,
            "画面がロックされているので何も送らない".to_string(),
        ),
        InputDesktop::Other { name } => (
            reason::OTHER_DESKTOP,
            name,
            format!(
                "入力デスクトップが自分のもの (Default) ではない (名前: {}) ので何も送らない。画面のロックか UAC など",
                name.as_deref().unwrap_or("読めない")
            ),
        ),
    };
    Some(
        AppError::new(ErrorKind::DesktopUnavailable, message)
            .with_reason(why)
            .with_detail(serde_json::json!({ "input_desktop": name })),
    )
}

/// 窓の判定 (`judge_window`) が設定の他に見るもの。実行器は `Ctx` から作り、`__focus-helper` の子は
/// 自分で集めて作る (D-138)
#[derive(Debug, Clone, Copy)]
pub struct WindowGuard<'a> {
    /// 届く先がこのハンドルのウィンドウなら送らない (ai-cu 自身の端末。D-045)
    pub self_hwnd: Option<usize>,
    /// 届く先がこの pid のウィンドウなら送らない (ai-cu を起動した側。D-055)
    pub host_pids: &'a [u32],
    /// ai-cu 自身が管理者権限で動いているか (UIPI の判定)
    pub self_elevated: bool,
    /// 次の手の文を選ぶ呼ばれ方 (D-056)
    pub surface: Surface,
}

/// 届く先 (または前面にする対象) が許可されたウィンドウか。`situation` は設定で拒否されたときの次の手。
/// 順は、自分の端末 (D-045)、起動した側 (D-055)、ゴースト (D-119)、設定の deny / allow、管理者権限。
/// 実行器のフォアグラウンドガードと `__focus-helper` の子が共有する (D-138)。`error.detail.window` は付けない
/// (付けるのは実行器の `check_window`)
pub fn judge_window(
    policy: &Policy,
    w: &WindowInfo,
    guard: &WindowGuard<'_>,
    place: &str,
    situation: Hint,
) -> Result<(), AppError> {
    let describe = || {
        format!(
            "{place}のウィンドウ (process={:?}, class={:?}, pid={})",
            w.process, w.class, w.pid
        )
    };
    // 自分の端末には、設定にかかわらず送らない (D-045)。設定の判定より先に見る
    if guard.self_hwnd == Some(w.hwnd) {
        return Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!("{} は ai-cu を動かしている端末なので送らない", describe()),
        )
        .with_hint(hint(Hint::SelfWindow, guard.surface)));
    }
    // ai-cu を起動した側 (親プロセスの連鎖) にも送らない (D-055)
    if guard.host_pids.contains(&w.pid) {
        return Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "{} は ai-cu を起動した側のウィンドウなので送らない",
                describe()
            ),
        )
        .with_hint(hint(Hint::HostWindow, guard.surface)));
    }
    // 応答しないアプリの代わりに OS が出すゴーストの窓には、設定にかかわらず送らない (D-119)。
    // 持ち主は OS のプロセスなので deny の設定に当たらず、元のアプリは応答しないので届いても意味が無い
    // (ゴーストの閉じるボタンはアプリを強制終了する確認を出す)
    if is_ghost_window(w) {
        return Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "{} は、応答しないアプリの代わりに OS が出したゴーストの窓なので送らない",
                describe()
            ),
        )
        .with_hint(hint(situation, guard.surface)));
    }
    // rule と fix は Denied から写し、message の頭にウィンドウを足す (D-079)
    policy.check_window(w).map_err(|d| {
        let mut e = d.into_error(ErrorKind::ForegroundGuard);
        e.message = format!("{}: {}", describe(), e.message);
        e.with_hint(hint(situation, guard.surface))
    })?;
    match w.elevated {
        Some(true) if !guard.self_elevated => Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!(
                "{} は管理者権限で動いている。入力が届かず失敗も検出できない (UIPI) ので送らない",
                describe()
            ),
        )
        .with_hint(hint(Hint::Elevated, guard.surface))),
        None => Err(AppError::new(
            ErrorKind::ForegroundGuard,
            format!("{} の権限を確かめられないので送らない", describe()),
        )
        .with_hint(hint(Hint::Elevated, guard.surface))),
        _ => Ok(()),
    }
}

/// `focus` の前面化の直前の無操作の確認 (D-049 の判定を focus にも掛ける)。`capture.min_idle_ms` が 0 なら
/// 無操作を読まずに通す。読み取りの失敗は `backend_error`、足りなければ 10 `user_active`。
/// 実行器の `focus` と `__focus-helper` の子が共有する (D-138)
pub fn focus_idle_check(
    policy: &Policy,
    idle_ms: &dyn Fn() -> Result<u64, BackendError>,
    surface: Surface,
) -> Result<(), AppError> {
    let min_idle = policy.capture_min_idle_ms();
    if min_idle == 0 {
        return Ok(());
    }
    let idle = idle_ms().map_err(backend_error)?;
    if user_active(idle, min_idle) {
        return Err(AppError::new(
            ErrorKind::UserActive,
            format!(
                "人が操作中なので前面にしない (最後の入力から {idle} ms。{min_idle} ms 以上の無操作が要る: capture.min_idle_ms)"
            ),
        )
        .with_detail(idle_detail(idle, min_idle))
        .with_hint(hint(Hint::FocusUserActive, surface)));
    }
    Ok(())
}

/// 実行器が操作に入った後の失敗に、その時点の進み具合 (`RunReport` と `partial`) を付け、何か済んでいれば
/// hint の頭に実行済みの一文を足す (D-065)。元の hint は状況の hint か、呼ばれ方の既定の文
/// (`AppError::hint()` は CLI の既定を返すので使わない。MCP の結果に `ai-cu ...` が混ざる)
fn with_progress(mut e: AppError, report: RunReport, partial: bool, surface: Surface) -> AppError {
    let executed = report.executed;
    let mut progress = serde_json::to_value(&report).expect("RunReport は JSON にできる");
    progress["partial"] = serde_json::json!(partial);
    let i = e.action_index.unwrap_or(executed);
    let prefix = crate::hint::done_prefix(executed, partial, i, surface);
    if !prefix.is_empty() {
        let original = e
            .hint
            .clone()
            .unwrap_or_else(|| crate::hint::default_hint(e.kind, surface).to_string());
        e.hint = Some(format!("{prefix}{original}"));
    }
    e.with_progress(progress)
}

/// 押下中のキーとボタン、カーソルの予定位置。
#[derive(Debug, Default)]
struct State {
    held_buttons: Vec<Button>,
    held_keys: Vec<(u16, bool)>,
    held_units: Vec<u16>,
    /// 直前に自分が送った移動の始点と終点 (この線分から離れたら、利用者が動かしたとみなす)
    prev: Option<Point>,
    expected: Option<Point>,
}

impl State {
    fn apply(&mut self, events: &[InputEvent]) {
        for e in events {
            match *e {
                InputEvent::MoveTo { x, y } => {
                    self.prev = self.expected;
                    self.expected = Some(Point::new(x, y));
                }
                InputEvent::Button { button, down: true } => self.held_buttons.push(button),
                InputEvent::Button {
                    button,
                    down: false,
                } => remove_one(&mut self.held_buttons, &button),
                InputEvent::Key {
                    vk,
                    down: true,
                    extended,
                } => self.held_keys.push((vk, extended)),
                InputEvent::Key {
                    vk,
                    down: false,
                    extended,
                } => remove_one(&mut self.held_keys, &(vk, extended)),
                InputEvent::Unicode { unit, down: true } => self.held_units.push(unit),
                InputEvent::Unicode { unit, down: false } => {
                    remove_one(&mut self.held_units, &unit)
                }
                InputEvent::Wheel { .. } => {}
            }
        }
    }

    /// 送信が失敗したバッチの押下を、押したままかもしれないものとして記録する。
    /// `SendInput` は途中までしか挿入しないことがあり、何個入ったかは分からない。余計に離すのは無害。
    fn record_downs(&mut self, events: &[InputEvent]) {
        for e in events {
            match *e {
                InputEvent::Button { button, down: true }
                    if !self.held_buttons.contains(&button) =>
                {
                    self.held_buttons.push(button)
                }
                InputEvent::Key {
                    vk,
                    down: true,
                    extended,
                } if !self.held_keys.contains(&(vk, extended)) => {
                    self.held_keys.push((vk, extended))
                }
                InputEvent::Unicode { unit, down: true } if !self.held_units.contains(&unit) => {
                    self.held_units.push(unit)
                }
                _ => {}
            }
        }
    }

    /// 送信に失敗したバッチを記録する。押下は押したままかもしれないものとして、移動は届いたかもしれない
    /// ものとして扱う。バッチの点はすべてガードを通っているので、離す位置はバッチの最後の移動先にする
    /// (古い点へ戻ってから離すと、頼んでいないドラッグになる)。
    fn record_failed(&mut self, events: &[InputEvent]) {
        if let Some(p) = events.iter().rev().find_map(|e| match *e {
            InputEvent::MoveTo { x, y } => Some(Point::new(x, y)),
            _ => None,
        }) {
            self.prev = self.expected;
            self.expected = Some(p);
        }
        self.record_downs(events);
    }

    /// 押したままを解くためのバッチの列: キーを離す / 最後に確かめた点へ戻る / ボタンを離す。
    /// ボタンを押したまま止まった (ドラッグの途中など) ときは、利用者がカーソルを動かした先で離すと、
    /// 確かめていない場所にドロップしうる。最後にガードを通った点へ戻してから離す。
    /// 各バッチは別々に送り、1 つが拒否されても残りは送る。
    fn release_batches(&self) -> Vec<Vec<InputEvent>> {
        let mut out = Vec::new();
        let mut keys = Vec::new();
        for &(vk, extended) in self.held_keys.iter().rev() {
            keys.push(InputEvent::Key {
                vk,
                down: false,
                extended,
            });
        }
        for &unit in self.held_units.iter().rev() {
            keys.push(InputEvent::Unicode { unit, down: false });
        }
        if !keys.is_empty() {
            out.push(keys);
        }
        if !self.held_buttons.is_empty() {
            if let Some(p) = self.expected {
                out.push(vec![InputEvent::move_to(p)]);
            }
            out.push(
                self.held_buttons
                    .iter()
                    .rev()
                    .map(|&button| InputEvent::Button {
                        button,
                        down: false,
                    })
                    .collect(),
            );
        }
        out
    }
}

fn remove_one<T: PartialEq>(v: &mut Vec<T>, x: &T) {
    if let Some(pos) = v.iter().rposition(|y| y == x) {
        v.remove(pos);
    }
}

pub struct Executor<'a> {
    pub policy: &'a Policy,
    pub sink: &'a mut dyn InputSink,
    pub desktop: &'a dyn Desktop,
    pub clock: &'a mut dyn Clock,
    pub stops: &'a mut StopSet,
    pub images: &'a mut dyn ImageWriter,
    /// 画面の縁の枠と予告の音 (D-093)。枠の装置を持たない経路は `NoIndicator`
    pub indicator: &'a mut dyn Indicator,
    /// プロセスをまたいだ入力の排他 (D-113)。錠の装置を持たない経路は `NoInputLock`
    pub input_lock: &'a mut dyn InputLock,
}

/// 1 回の実行の呼ばれ方 (`execute_on` の引数。D-093 でまとめた)
#[derive(Debug, Clone, Copy)]
pub struct ExecOptions<'w> {
    /// 届く先がこのウィンドウなら送らない (TUI の自分の端末。D-045)
    pub self_window: Option<&'w WindowInfo>,
    /// ai-cu を起動した側のプロセス (D-055)
    pub host_pids: &'w [u32],
    /// 失敗の次の手 (hint) の文を選ぶ呼ばれ方 (D-056)
    pub surface: Surface,
    /// 実行の後に枠を残す時間 (ミリ秒)。止められた実行の後は 0 にする (D-093)
    pub linger_ms: u64,
    /// 予告の間を省く (TUI。枠は出す。D-093)
    pub skip_lead: bool,
}

struct Ctx<'h> {
    screen: Rect,
    start_ms: u64,
    self_elevated: bool,
    /// 届く先がこのハンドルのウィンドウなら送らない (ai-cu 自身の端末。D-045)
    self_hwnd: Option<usize>,
    /// 届く先がこの pid のウィンドウなら送らない (ai-cu を起動した側。D-055)
    host_pids: &'h [u32],
    /// 次の手の文を選ぶ呼ばれ方 (D-056)
    surface: Surface,
    executed: usize,
    /// 今の操作 (plans の 1 要素) の副作用 (入力のバッチ、前面化の依頼、WM_CLOSE) を一部でも起こしたか (D-065)。
    /// 各操作の頭で false にする
    partial: bool,
    shots: Vec<ShotInfo>,
    focused: Vec<FocusInfo>,
    closed: Vec<CloseInfo>,
    waited: Vec<WaitInfo>,
}

impl Ctx<'_> {
    /// その時点の進み具合 (成功の結果と同じ形)
    fn report(&mut self, now_ms: u64) -> RunReport {
        RunReport {
            executed: self.executed,
            screenshots: std::mem::take(&mut self.shots),
            focused: std::mem::take(&mut self.focused),
            closed: std::mem::take(&mut self.closed),
            waited: std::mem::take(&mut self.waited),
            elapsed_ms: now_ms.saturating_sub(self.start_ms),
        }
    }
}

/// `wait_for` の条件 (`Step::WaitFor` のフィールドの借用。D-068)
struct WaitSpec<'a> {
    process: &'a str,
    title: Option<&'a str>,
    element: Option<&'a ElementFilter>,
    text: Option<&'a str>,
    absent: bool,
    timeout_ms: u64,
}

impl WaitSpec<'_> {
    /// 失敗の message に書く条件 (None の項目は省く)
    fn describe(&self) -> String {
        let mut s = format!("process={:?}", self.process);
        if let Some(t) = self.title {
            s.push_str(&format!(", title={t:?}"));
        }
        if let Some(f) = self.element {
            let mut parts = Vec::new();
            if let Some(n) = &f.name {
                parts.push(format!("name={n:?}"));
            }
            if let Some(t) = &f.control_type {
                parts.push(format!("type={t:?}"));
            }
            s.push_str(&format!(", element={{{}}}", parts.join(", ")));
        }
        if let Some(t) = self.text {
            s.push_str(&format!(", text={t:?}"));
        }
        s.push_str(&format!(", absent={}", self.absent));
        s
    }

    /// 対象のウィンドウの表示 (`find` / `text` と同じ形)
    fn shown(&self) -> String {
        match self.title {
            Some(t) => format!("process={:?}, title={t:?}", self.process),
            None => format!("process={:?}", self.process),
        }
    }
}

/// 画面の待ちの条件 (`Step::WaitScreen` のフィールドの借用。D-081)。screenshot の settle も同じ形で使う
struct ScreenSpec<'a> {
    rect: Option<Rect>,
    window: Option<&'a WindowTarget>,
    until: Until,
    threshold: f64,
    timeout_ms: u64,
}

/// `watch_screen` の結果 (D-081)。満たされたか時間切れかと、最後に撮ったもの
struct ScreenWatch {
    met: bool,
    /// 撮った回数 (最初の撮影を含む)
    checks: usize,
    elapsed_ms: u64,
    /// 最後に撮った矩形と、そのときのウィンドウ (`window` で待ったとき)
    rect: Rect,
    window: Option<WindowInfo>,
    /// 最後の比較の差 (stable は直前との差、change は基準との差)。比べる前に終わったなら None
    last_diff: Option<f64>,
    /// 最後に撮った縮める前の画像 (screenshot の settle はこれを保存する。撮り直さない)
    frame: Image,
}

/// screenshot の 1 回分 (`Step::Capture` のフィールドの借用。D-053、D-072、D-081)
struct CaptureSpec<'a> {
    rect: Option<Rect>,
    window: Option<&'a WindowTarget>,
    scale: f64,
    max_edge: Option<u32>,
    path: &'a str,
    settle: Option<Settle>,
}

/// 差の割合を小数第 4 位で丸める (D-081。結果の `changed_ratio` と `last_changed_ratio`)
fn round_ratio(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// `wait_for` の 1 回の判定の結果 (D-068)。`unmet` は、満たされていなければ無かった (absent ならまだあった) 段
struct WaitObservation {
    present: bool,
    /// `"window"` | `"element"` | `"text"`
    unmet: &'static str,
    /// この判定で見たウィンドウ (無ければ None)
    window: Option<TopWindow>,
    /// 「ある」で満たされたときの要素 (要素か文字列の条件のときだけ)
    element: Option<UiElement>,
}

impl Executor<'_> {
    pub fn execute(&mut self, script: &Script) -> Result<RunReport, AppError> {
        self.execute_excluding(script, None)
    }

    /// `self_window` を渡すと、入力が届く先 (キーは前面、マウスは座標の下) がそのウィンドウのときは、
    /// 設定にかかわらずフォアグラウンドガードで拒否する。TUI からの実行で、自分の端末に送らないために使う (D-045)。
    pub fn execute_excluding(
        &mut self,
        script: &Script,
        self_window: Option<&WindowInfo>,
    ) -> Result<RunReport, AppError> {
        self.execute_with(script, self_window, &[])
    }

    /// `execute_excluding` に加えて、`host_pids` (ai-cu を起動した側のプロセス。D-055) のウィンドウも、
    /// 入力の届く先、`focus` の対象、`screenshot` の `window` の対象から除外する。
    pub fn execute_with(
        &mut self,
        script: &Script,
        self_window: Option<&WindowInfo>,
        host_pids: &[u32],
    ) -> Result<RunReport, AppError> {
        self.execute_on(
            script,
            &ExecOptions {
                self_window,
                host_pids,
                surface: Surface::Cli,
                linger_ms: 0,
                skip_lead: false,
            },
        )
    }

    /// `execute_with` に加えて、失敗の次の手 (hint) の文を `opts.surface` (CLI か MCP か) で選び (D-056)、
    /// 画面を変える操作の前に枠と予告の間を入れる (D-093。残す時間と予告の省略は `opts`)。
    /// 判定と送るものは `surface` に依らない。
    /// 画面を変える操作 (入力のバッチ、focus、close) を含む実行は、検証の後で入力の錠 (別の ai-cu との排他) を取ってから
    /// 操作の前の判定に進み、成否によらず終わりに解く (D-113)
    pub fn execute_on(
        &mut self,
        script: &Script,
        opts: &ExecOptions<'_>,
    ) -> Result<RunReport, AppError> {
        let screen = self.desktop.virtual_screen().map_err(backend_error)?;
        let v = validate(script, self.policy, Some(screen));
        if let Some(e) = validation_error(&v) {
            return Err(e);
        }
        let changes_screen = v.plans.iter().flatten().any(|s| {
            matches!(
                s,
                Step::Batch { .. } | Step::Focus { .. } | Step::Close { .. }
            )
        });
        // 内側が枠を出して自分で解いたか (枠の解放を 1 回にする)
        let mut frame_released = false;
        let result = if changes_screen {
            match self.acquire_input_lock(opts.surface) {
                Ok(waited) => {
                    let r =
                        self.execute_checked(screen, &v, opts, true, waited, &mut frame_released);
                    // 押したままのキーとボタンを離し、枠を解いた後に解く (成否によらず)
                    self.input_lock.release();
                    r
                }
                // 錠の待ちの 6、16、8。枠は出していない
                Err(e) => Err(e),
            }
        } else {
            self.execute_checked(screen, &v, opts, false, false, &mut frame_released)
        };
        // 止められた実行の後は枠を残さない。枠を出していない実行 (読み取りの wait_for、画面を変えない run、錠の待ちの 6) でも、
        // 前の呼び出しが残した自分の枠を消す (次の入力が予告と人の入力の判定を省かないように。D-093、D-107、D-113)
        if !frame_released && result.as_ref().is_err_and(AppError::stopped_by_user) {
            self.indicator.release(0);
        }
        result
    }

    /// 入力の錠を取る (D-113)。待たずに試し、取れなければ `STOP_POLL_MS` 眠って停止要因 (目印、取り消し) を見て
    /// 試し直す (止まれば 6)。マウスの移動は見ない (待つ間に動かしているのは前の ai-cu)。始めから
    /// `INPUT_LOCK_WAIT_MS` 以上たっても取れなければ、何も送らずに 16 (`input_busy`)。試みの失敗は `backend_error` (8)。
    /// 戻り値は待ったか (最初の試みで取れなかったか)
    fn acquire_input_lock(&mut self, surface: Surface) -> Result<bool, AppError> {
        let start = self.clock.now_ms();
        let mut waited = false;
        loop {
            if self.input_lock.try_acquire().map_err(backend_error)? {
                return Ok(waited);
            }
            waited = true;
            let elapsed = self.clock.now_ms().saturating_sub(start);
            if elapsed >= INPUT_LOCK_WAIT_MS {
                return Err(AppError::new(
                    ErrorKind::InputBusy,
                    format!(
                        "別の ai-cu の画面の操作が {} 秒たっても終わらないので、何も送らずに止めた",
                        INPUT_LOCK_WAIT_MS / 1000
                    ),
                )
                .with_detail(serde_json::json!({ "waited_ms": elapsed }))
                .with_hint(crate::hint::default_hint(ErrorKind::InputBusy, surface)));
            }
            self.clock.sleep_ms(STOP_POLL_MS);
            if let Some(r) = self.stops.poll() {
                return Err(Self::stopped(r));
            }
        }
    }

    /// `execute_on` の本体 (検証の後、錠を取った後): 操作の前の判定、枠と予告の間、操作の実行、押したままを離す、
    /// 枠を解く。`changes_screen` は画面を変える操作を含むか (`execute_on` が検証の直後に決める)、`waited` は錠を
    /// 待ったか (待った実行は予告の間を縮めない。D-113)。枠を出して自分で解いたら `frame_released` を真にする
    fn execute_checked(
        &mut self,
        screen: Rect,
        v: &crate::validate::Validation,
        opts: &ExecOptions<'_>,
        changes_screen: bool,
        waited: bool,
        frame_released: &mut bool,
    ) -> Result<RunReport, AppError> {
        let ExecOptions {
            self_window,
            host_pids,
            surface,
            linger_ms,
            skip_lead,
        } = *opts;
        // 操作に入る前の読み取りの判定 (D-066)。順は ロック・セーバー (14) → 押されたままのキー (10) → カーソル。
        // ここの失敗は何も送る前なので、進み具合 (D-065) は付けない。入力デスクトップの判定は、途中の撮影と
        // focus のステップの直前にも同じ関数で通す (run_plans。D-067)
        let steps = || v.plans.iter().flatten();
        // 画面の待ち (D-081) は撮影の仲間: ロック中に走らせるとロック画面を撮り続けて落ち着いたと返る
        let needs_desktop = steps().any(|s| {
            matches!(
                s,
                Step::Batch { .. }
                    | Step::Focus { .. }
                    | Step::Capture { .. }
                    | Step::WaitScreen { .. }
            )
        });
        let has_input = steps().any(|s| matches!(s, Step::Batch { .. }));
        if needs_desktop {
            self.check_input_desktop()?;
        }
        if has_input {
            // 実行の最初に 1 回だけ (バッチごとに見ると、drag や key の組み合わせで自分の押下に引っかかる)
            self.check_keys_held(surface)?;
        }
        let mut state = State::default();
        match self.desktop.cursor_pos() {
            Ok(cursor) => {
                state.prev = Some(cursor);
                state.expected = Some(cursor);
            }
            // close、wait、wait_for だけの操作列は、入力デスクトップが使えない (ロック中) ときもマウスの監視なしで進む。
            // 読めるようになったら check_stop が監視を始める (D-066、D-068)
            Err(BackendError::DesktopUnavailable(_)) if !needs_desktop => {}
            Err(e) => return Err(backend_error(e)),
        }
        // 枠と予告の間 (D-093)。画面を変える操作 (入力のバッチ、focus、close) があり、始めのカーソルが読めた
        // (マウスの監視がある) ときだけ。ロック中に監視なしで始まる close だけの操作列では出さない
        let shown = changes_screen && self.policy.indicator_enabled() && state.prev.is_some();
        if shown {
            let lead_ms = self.policy.indicator_lead_ms();
            // 錠を待った実行は、枠の経過でも TUI (skip_lead。カウントダウンは待ちより前に終わっている) でも縮めず、
            // lead_ms をまるごと待つ (D-113)。待っていなければ、TUI は待たず、ほかは枠 (自分のも他の ai-cu のも) が
            // 出てからの経過の分だけ縮める。lead_ms が 0 なら枠を読まない
            let wait_ms = if waited {
                lead_ms
            } else if skip_lead || lead_ms == 0 {
                0
            } else {
                let shown_for = self.indicator.shown_for_ms().unwrap_or(0);
                lead_ms - lead_ms.min(shown_for)
            };
            let deadline = wait_ms
                .saturating_add(self.policy.max_duration_ms())
                .saturating_add(linger_ms)
                .saturating_add(OVERLAY_DEADLINE_MARGIN_MS);
            self.indicator.show(deadline);
            if wait_ms > 0 {
                if self.policy.indicator_sound() {
                    self.indicator.beep();
                }
                // 予告の間の失敗 (6、10) は何も送る前なので、進み具合 (D-065) を付けない。枠は残さない
                if let Err(e) = self.lead(wait_ms, lead_ms, &mut state, surface) {
                    self.indicator
                        .release(if e.stopped_by_user() { 0 } else { linger_ms });
                    *frame_released = true;
                    return Err(e);
                }
            }
        }
        // 時間の上限 (`limits.max_duration_ms`) と結果の `elapsed_ms` は予告の間の後から数える (D-093)
        let mut ctx = Ctx {
            screen,
            start_ms: self.clock.now_ms(),
            self_elevated: self.desktop.self_elevated(),
            self_hwnd: self_window.map(|w| w.hwnd),
            host_pids,
            surface,
            executed: 0,
            partial: false,
            shots: Vec::new(),
            focused: Vec::new(),
            closed: Vec::new(),
            waited: Vec::new(),
        };
        let result = self.run_plans(&v.plans, &mut state, &mut ctx);
        // 失敗や停止の後でも、押したままのキーとボタンを必ず離す
        for batch in state.release_batches() {
            let _ = self.sink.send(&batch);
        }
        let report = ctx.report(self.clock.now_ms());
        // 枠は成否にかかわらず解く。人か停止要因に止められた実行の後は残さない (D-093)。
        // 枠を出していない実行の止められた後の後始末は `execute_on` が 1 か所で行う
        if shown {
            let stopped = result.as_ref().is_err_and(AppError::stopped_by_user);
            self.indicator.release(if stopped { 0 } else { linger_ms });
            *frame_released = true;
        }
        match result {
            Ok(()) => Ok(report),
            Err(e) => Err(with_progress(e, report, ctx.partial, surface)),
        }
    }

    /// 予告の間 (D-093)。`wait_ms` の間、`STOP_POLL_MS` ごとに停止要因 (目印、取り消し) とマウスの移動を見る
    /// (時間の上限は見ない)。終わりに無操作の長さを読み、始めの猶予 (`LEAD_INPUT_GRACE_MS`) より後に人の入力
    /// (キーを含む) があれば 10 (`lead_input`)。無操作が読めなければ判定を飛ばす
    fn lead(
        &mut self,
        wait_ms: u64,
        lead_ms: u64,
        state: &mut State,
        surface: Surface,
    ) -> Result<(), AppError> {
        let mut left = wait_ms;
        while left > 0 {
            let chunk = left.min(STOP_POLL_MS);
            self.clock.sleep_ms(chunk);
            left -= chunk;
            if let Some(r) = self.stops.poll() {
                return Err(Self::stopped(r));
            }
            self.check_user_mouse(state)?;
        }
        if wait_ms > LEAD_INPUT_GRACE_MS
            && let Ok(idle) = self.desktop.idle_ms()
            && user_active(idle, wait_ms - LEAD_INPUT_GRACE_MS)
        {
            return Err(AppError::new(
                ErrorKind::UserActive,
                "予告の間 (枠を出してから動かすまで) に人の入力があったので、何も送らずに止めた",
            )
            .with_reason(reason::LEAD_INPUT)
            .with_detail(serde_json::json!({ "idle_ms": idle, "lead_ms": lead_ms }))
            .with_hint(hint(Hint::LeadUserActive, surface)));
        }
        Ok(())
    }

    fn run_plans(
        &mut self,
        plans: &[Vec<Step>],
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), AppError> {
        for (i, steps) in plans.iter().enumerate() {
            ctx.partial = false;
            // パスワード欄の判定は、操作ごとに最初のキーの束と、フォーカスを動かしうるキー (Tab と Enter) を
            // 送った後の次のキーの束で行う (D-090)
            let mut password_checked = false;
            // キーの束の届く先は、操作の中では最初の束の前面のプロセスに固定する (D-100)
            let mut pinned_pid: Option<u32> = None;
            for step in steps {
                self.check_stop(state, ctx).map_err(|e| e.at(i))?;
                match step {
                    Step::Batch { target, events } => {
                        let mid_drag = !state.held_buttons.is_empty();
                        // 修飾キーつきのクリック (D-137): マウスの束でもキーのイベントを含めば、修飾キーは前面の
                        // 窓に届くため、座標の下の窓より先に前面の窓も確かめる (D-033 の届く先の判定を広げる)。
                        // 前面の窓は捨てる (文字を送らないので、pinned_pid とパスワード欄の判定には使わない)
                        if matches!(target, Target::Mouse { .. })
                            && crate::plan::has_key_events(events)
                        {
                            self.guard(&Target::Keyboard, ctx).map_err(|mut e| {
                                e.message = format!(
                                    "修飾キーつきのクリックは前面の窓にも修飾キーが届くので前面も確かめる: {}",
                                    e.message
                                );
                                mid_drag_hint(e, mid_drag, target, ctx.surface).at(i)
                            })?;
                        }
                        let fg = self
                            .guard(target, ctx)
                            .map_err(|e| mid_drag_hint(e, mid_drag, target, ctx.surface).at(i))?;
                        if let Some(fg) = &fg {
                            match pinned_pid {
                                None => pinned_pid = Some(fg.pid),
                                Some(pid) if pid != fg.pid => {
                                    return Err(target_changed(fg, pid, ctx.surface).at(i));
                                }
                                Some(_) => {}
                            }
                        }
                        if let Some(fg) = fg
                            && !password_checked
                        {
                            password_checked = true;
                            self.recheck_after_password_read(&fg, target, state, ctx)
                                .map_err(|e| e.at(i))?;
                        }
                        if let Err(e) = self.sink.send(events) {
                            if matches!(e, BackendError::Os(_)) {
                                // OS の呼び出しで失敗したときは、一部が挿入された可能性がある
                                state.record_failed(events);
                                ctx.partial = true;
                            }
                            return Err(backend_error(e).at(i));
                        }
                        ctx.partial = true;
                        state.apply(events);
                        if moves_focus(events) {
                            password_checked = false;
                        }
                    }
                    Step::Sleep { ms } => self.sleep(*ms, state, ctx).map_err(|e| e.at(i))?,
                    Step::Capture {
                        rect,
                        window,
                        scale,
                        max_edge,
                        path,
                        settle,
                    } => {
                        let spec = CaptureSpec {
                            rect: *rect,
                            window: window.as_ref(),
                            scale: *scale,
                            max_edge: *max_edge,
                            path,
                            settle: *settle,
                        };
                        self.capture(i, &spec, state, ctx).map_err(|e| e.at(i))?
                    }
                    Step::Focus { process, title } => {
                        // ロックの幕の間の focus が 11 (前面化の失敗) にならないよう、直前にも判定を通す (D-067)
                        self.check_input_desktop().map_err(|e| e.at(i))?;
                        self.focus(i, process, title.as_deref(), state, ctx)
                            .map_err(|e| e.at(i))?
                    }
                    Step::Close { process, title } => self
                        .close(i, process, title, state, ctx)
                        .map_err(|e| e.at(i))?,
                    // 読むだけなので、入力デスクトップの判定は通さない (ロック中も読める。D-068)
                    Step::WaitFor {
                        process,
                        title,
                        element,
                        text,
                        absent,
                        timeout_ms,
                    } => {
                        let spec = WaitSpec {
                            process,
                            title: title.as_deref(),
                            element: element.as_ref(),
                            text: text.as_deref(),
                            absent: *absent,
                            timeout_ms: *timeout_ms,
                        };
                        self.wait_for(i, &spec, state, ctx).map_err(|e| e.at(i))?
                    }
                    // 撮るたびに grab が撮影と同じ判定 (入力デスクトップを含む) を通す (D-081)
                    Step::WaitScreen {
                        rect,
                        window,
                        until,
                        threshold,
                        timeout_ms,
                    } => {
                        let spec = ScreenSpec {
                            rect: *rect,
                            window: window.as_ref(),
                            until: *until,
                            threshold: *threshold,
                            timeout_ms: *timeout_ms,
                        };
                        self.wait_screen(i, &spec, state, ctx)
                            .map_err(|e| e.at(i))?
                    }
                }
            }
            ctx.executed += 1;
        }
        Ok(())
    }

    fn stopped(reason: StopReason) -> AppError {
        AppError::new(ErrorKind::Stopped, reason.to_string())
    }

    /// 入力デスクトップが使えなければ 14 (D-066、D-067)。読み取りの失敗は `backend_error` (多くは 8)。
    /// 操作に入る前の 1 回と、途中の撮影と focus のステップの直前に呼ぶ (入力のバッチと停止の確認では呼ばない)
    fn check_input_desktop(&self) -> Result<(), AppError> {
        let state = self.desktop.input_desktop().map_err(backend_error)?;
        match input_desktop_error(&state) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// 押されたままの修飾キーかマウスボタンがあれば 10 `keys_held` (D-066)。離すことはしない
    fn check_keys_held(&self, surface: Surface) -> Result<(), AppError> {
        let held = self.desktop.keys_held().map_err(backend_error)?;
        if held.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = held.keys.iter().chain(&held.buttons).copied().collect();
        Err(AppError::new(
            ErrorKind::UserActive,
            format!(
                "キーかマウスのボタンが押されたまま ({}) なので、入力に混ざらないよう何も送らない",
                names.join(", ")
            ),
        )
        .with_reason(reason::KEYS_HELD)
        .with_detail(serde_json::json!({ "keys": held.keys, "buttons": held.buttons }))
        .with_hint(hint(Hint::KeysHeld, surface)))
    }

    fn check_stop(&mut self, state: &mut State, ctx: &Ctx<'_>) -> Result<(), AppError> {
        if let Some(r) = self.stops.poll() {
            return Err(Self::stopped(r));
        }
        let limit = self.policy.max_duration_ms();
        if self.clock.now_ms().saturating_sub(ctx.start_ms) > limit {
            return Err(Self::stopped(StopReason::Deadline { limit_ms: limit }));
        }
        self.check_user_mouse(state)
    }

    /// 利用者のマウスの移動 (6)。直前に自分が送った移動の線分から閾値より離れていれば止める。
    /// 停止の確認 (`check_stop`) と予告の間 (D-093) が共有する (判定はここだけ)
    fn check_user_mouse(&mut self, state: &mut State) -> Result<(), AppError> {
        // 監視なしで始めた (ロック中の close、wait、wait_for だけの操作列。D-066、D-068): 読めるようになった時点の点から監視を始める
        if state.prev.is_none() && state.expected.is_none() {
            match self.desktop.cursor_pos() {
                Ok(p) => {
                    state.prev = Some(p);
                    state.expected = Some(p);
                }
                Err(BackendError::DesktopUnavailable(_)) => {}
                Err(e) => return Err(backend_error(e)),
            }
            return Ok(());
        }
        if let (Some(prev), Some(expected)) = (state.prev, state.expected) {
            let actual = self.desktop.cursor_pos().map_err(backend_error)?;
            let d = actual.distance_to_segment(prev, expected);
            if d > f64::from(self.policy.mouse_threshold_px()) {
                // 停止の状況: 読んだカーソルと、直前の移動の線分 (D-065)
                return Err(
                    Self::stopped(StopReason::UserMouse { distance_px: d }).with_detail(
                        serde_json::json!({
                            "cursor": { "x": actual.x, "y": actual.y },
                            "from": { "x": prev.x, "y": prev.y },
                            "expected": { "x": expected.x, "y": expected.y },
                            "distance_px": d.round() as i64,
                        }),
                    ),
                );
            }
            // 到着したら線分を終点に縮める。以後は終点からの距離で見る (長い移動を戻る操作を見逃さない)
            if actual.distance_to_segment(expected, expected) <= 2.0 {
                state.prev = Some(expected);
            }
        }
        Ok(())
    }

    fn sleep(&mut self, ms: u64, state: &mut State, ctx: &Ctx<'_>) -> Result<(), AppError> {
        let mut left = ms;
        while left > 0 {
            let chunk = left.min(STOP_POLL_MS);
            self.clock.sleep_ms(chunk);
            left -= chunk;
            self.check_stop(state, ctx)?;
        }
        Ok(())
    }

    /// パスワード欄への入力の判定 (D-090)。最初のガードが通った後に呼ぶ。読んで通すときは、読む間に止まりうるので
    /// 停止の確認とガードを取り直し、前面の窓が変わっていれば読み直さずに 7 で断る (新しい前面は許可された窓でも
    /// ありうるので、古い判定で送らない)
    fn recheck_after_password_read(
        &mut self,
        fg: &WindowInfo,
        target: &Target,
        state: &mut State,
        ctx: &Ctx<'_>,
    ) -> Result<(), AppError> {
        if !self.check_password_focus(fg, ctx)? {
            return Ok(());
        }
        self.check_stop(state, ctx)?;
        if let Some(now) = self.guard(target, ctx)?
            && now.hwnd != fg.hwnd
        {
            return Err(AppError::new(
                ErrorKind::ForegroundGuard,
                format!(
                    "パスワード欄かを確かめる間に前面のウィンドウが変わった (process={:?}, class={:?}, pid={})。古い判定では送らない",
                    now.process, now.class, now.pid
                ),
            )
            .with_hint(hint(Hint::ForegroundChanged, ctx.surface))
            .with_detail(serde_json::json!({ "window": window_detail(&now) })));
        }
        Ok(())
    }

    /// フォーカスの要素がパスワード欄なら 4 で断る (D-090)。戻り値は UIA を読んだか。全層が
    /// `general.password_input` を許していれば読まない。時間切れ (`TimedOut`。UIA 自身の時間切れ、上限の待ち、
    /// 返らない判定の上限) は何も送らずに 8 で断る (D-104、本人の決定)。他の読めないとき (`Ok(None)` と他の `Err`)
    /// は通す (fail-open)。
    /// `fg` は最初のガードが読んだ前面の窓で、断るときの `error.detail.window` に使う (D-065)
    fn check_password_focus(&self, fg: &WindowInfo, ctx: &Ctx<'_>) -> Result<bool, AppError> {
        let Err(denied) = self.policy.password_input_allowed() else {
            return Ok(false);
        };
        match self.desktop.focused_is_password() {
            Ok(Some(true)) => Err(denied
                .into_error(ErrorKind::PolicyDenied)
                .with_hint(hint(Hint::PasswordField, ctx.surface))
                .with_detail(serde_json::json!({ "window": window_detail(fg) }))),
            // 読み取りを頼んでいない呼び手にも、入力を断った理由が分かる文にする。Tab や Enter の後の 2 回目の
            // 判定では前の束を送り済みなので、「何も送らずに」とは書かない (進み具合は error の partial で分かる)
            Err(e @ BackendError::TimedOut(_)) => Err(AppError::new(
                ErrorKind::OsError,
                format!(
                    "パスワード欄かの判定 (UIA) が時間切れのため、ここから先を送らずに断った: {e}"
                ),
            )),
            _ => Ok(true),
        }
    }

    /// フォアグラウンドガード (D-020、D-033)。送る直前に、届く先のウィンドウを取り直して確かめる。
    /// キーの束なら、確かめた前面の窓を返す (パスワード欄の判定の detail と、前面が変わったかの比較に使う。D-090)。
    /// マウスは `None`
    fn guard(&self, target: &Target, ctx: &Ctx<'_>) -> Result<Option<WindowInfo>, AppError> {
        match target {
            Target::Mouse { points } => {
                let pts = if points.is_empty() {
                    vec![self.desktop.cursor_pos().map_err(backend_error)?]
                } else {
                    points.clone()
                };
                for p in pts {
                    let point = serde_json::json!({ "x": p.x, "y": p.y });
                    // 届く先が無い 7 の状況 (D-065)
                    let no_window = serde_json::json!({ "point": point, "window": null });
                    if !ctx.screen.contains(p) {
                        return Err(AppError::new(
                            ErrorKind::ForegroundGuard,
                            format!("座標 ({}, {}) が画面 {} の外", p.x, p.y, ctx.screen),
                        )
                        .with_detail(no_window));
                    }
                    self.policy
                        .check_point(p)
                        .map_err(|d| d.into_error(ErrorKind::PolicyDenied))?;
                    let w = self
                        .desktop
                        .window_at(p)
                        .map_err(backend_error)?
                        .ok_or_else(|| {
                            AppError::new(
                                ErrorKind::ForegroundGuard,
                                format!("座標 ({}, {}) の下にウィンドウが無い", p.x, p.y),
                            )
                            .with_detail(no_window)
                        })?;
                    // 断ったウィンドウの detail に、届く先を決めた点を足す
                    self.check_window(&w, ctx, "座標の下", Hint::MouseGuard)
                        .map_err(|mut e| {
                            if let Some(d) = e.detail.as_deref_mut() {
                                d["point"] = point;
                            }
                            e
                        })?;
                }
            }
            Target::Keyboard => {
                let w = self
                    .desktop
                    .foreground_window()
                    .map_err(backend_error)?
                    .ok_or_else(|| {
                        AppError::new(ErrorKind::ForegroundGuard, "前面のウィンドウが無い")
                            .with_detail(serde_json::json!({ "window": null }))
                    })?;
                self.check_window(&w, ctx, "前面", Hint::KeyboardGuard)?;
                return Ok(Some(w));
            }
        }
        Ok(None)
    }

    /// 許可されたウィンドウを前面にする (D-050)。`SetForegroundWindow` を 1 回呼ぶだけで、入力は送らず、
    /// 最小化は復元しない。人が別のアプリを使っていれば OS が断るので、それをそのまま終了コード 11 にする (理由は
    /// 前面のプロセスで分けて `reason` に入れる。D-056)。
    /// CLI として呼ぶたびに起動されたときは、AI の端末が前面なら OS は断らない (常駐の MCP サーバーは端末が前面でも
    /// 断られうる。D-056) ので、人の最後の入力からの無操作 (`capture.min_idle_ms`) も求める。
    fn focus(
        &mut self,
        i: usize,
        process: &str,
        title: Option<&str>,
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), AppError> {
        // 名指しの候補は `windows` に出る窓と TUI の端末を除いた窓だけ (D-139)。`check_stop` / `settle` が
        // `&mut self` を取るので、述語が使う値は先に写す (借用を重ねない)
        let (policy, host_pids, self_hwnd) = (self.policy, ctx.host_pids, ctx.self_hwnd);
        let admit = |w: &WindowInfo| named_window_admitted(policy, w, host_pids, self_hwnd);
        let Some(w) = self
            .desktop
            .find_window(process, title, &admit)
            .map_err(backend_error)?
        else {
            return Err(AppError::new(
                ErrorKind::FocusFailed,
                format!(
                    "前面にする対象が見つからない (process={process:?}, title={title:?})。{NAMED_WINDOW_SCOPE_NOTE}"
                ),
            )
            .with_reason(reason::NOT_FOUND)
            .with_hint(hint(Hint::FocusNotFound, ctx.surface)));
        };
        // 前面にしてよい相手か。フォアグラウンドガードと同じ判定 (設定、自分の端末、管理者権限)。設定と自分の端末は
        // 述語を通った窓には当たらない (守りの重ね。D-139)。権限の 7 は残る
        self.check_window(&w.info, ctx, "前面にする対象", Hint::FocusDenied)?;
        let describe = format!(
            "process={:?}, title={:?}, hwnd={:#x}",
            w.info.process, w.info.title, w.info.hwnd
        );
        if w.minimized {
            return Err(AppError::new(
                ErrorKind::FocusFailed,
                format!("対象 ({describe}) は最小化されている。ai-cu は復元しない"),
            )
            .with_reason(reason::MINIMIZED)
            .with_hint(hint(Hint::FocusMinimized, ctx.surface)));
        }
        // 既に前面 (対象そのもの、または対象に所有されたダイアログ) なら何もしない。まだ呼んでいないので拒否は 7 にしない
        // 呼ぶ前の前面の hwnd を控える。呼んだ後に前面が変わったか (partial) をこれと比べる (D-065)
        let mut before: Option<usize> = None;
        if let Some(fg) = self.reached(&w, ctx, false, &mut before)? {
            ctx.focused.push(FocusInfo::new(i, &fg, false));
            return Ok(());
        }
        // 人が操作中なら前面を奪わない (D-049 の無操作の判定を focus にも掛ける。CLI の都度起動では、端末が前面なら
        // OS は断らないので、OS の拒否には頼れない)。判定は `__focus-helper` の子と共有する (D-138)
        focus_idle_check(self.policy, &|| self.desktop.idle_ms(), ctx.surface)?;
        let accepted = self
            .desktop
            .set_foreground(w.info.hwnd)
            .map_err(backend_error)?;
        if let Some(fg) = self.settle(&w, accepted, before, state, ctx)? {
            ctx.focused.push(FocusInfo::new(i, &fg, false));
            return Ok(());
        }
        // 失敗の理由を分ける: 対象が閉じられた (同じウィンドウを取り直せない) / 断られた (前面で 3 つに分ける)。
        // 補助プロセスを使ったときは、その後にもう一度確かめる (補助の間に閉じられることがある)
        // 取り直しも同じ述語で探す (前に出てきた許可の外の窓を「対象が閉じられた」の根拠にしない。D-139)
        let still_there = |d: &dyn Desktop| -> Result<bool, AppError> {
            Ok(d.find_window(process, title, &admit)
                .map_err(backend_error)?
                .is_some_and(|again| {
                    again.info.hwnd == w.info.hwnd && again.info.pid == w.info.pid
                }))
        };
        let surface = ctx.surface;
        let closed = |helper: Option<&HelperOutcome>| {
            let e = AppError::new(
                ErrorKind::FocusFailed,
                format!(
                    "対象 ({describe}) が閉じられたか、題名が変わった (SetForegroundWindow={})",
                    u8::from(accepted)
                ),
            )
            .with_reason(reason::CLOSED)
            .with_hint(hint(Hint::FocusNotFound, surface));
            match helper {
                Some(h) => e.with_detail(helper_detail(Some(h))),
                None => e,
            }
        };
        if !still_there(self.desktop)? {
            return Err(closed(None));
        }
        // 前面のプロセスで理由を分ける (D-056)。「人が別のアプリを使っている」と言い切るのは、前面がホスト以外の
        // アプリのときだけ。前面がホスト (AI の端末など) なのに断られたのは、この ai-cu に前面化の権利が無いため
        // (常駐の MCP サーバー、起動の後の人の入力)。前面が無いか読めなければ断定しない。既に失敗の経路なので、
        // 前面を読めないこと自体は 8 にしない
        let read_fg = |d: &dyn Desktop| d.foreground_window().ok().flatten().filter(|f| f.pid != 0);
        let mut fg = read_fg(self.desktop);
        // 常駐の MCP サーバーは、起動の後の人の入力で前面化の権利を失う。断られ (0)、前面がホストなら、使い捨ての
        // 補助プロセスに 1 回だけ頼む (D-057)。前面が別のアプリなら補助も断られる (前面ロックの抜け道にしない) ので
        // 起動しない。CLI (TUI を含む) は都度起動なので、直接の呼び出しで足りる
        let mut helper: Option<HelperOutcome> = None;
        if ctx.surface == Surface::Mcp
            && !accepted
            && fg.as_ref().is_some_and(|f| ctx.host_pids.contains(&f.pid))
        {
            self.check_stop(state, ctx)?;
            let outcome = self
                .desktop
                .set_foreground_by_helper(w.info.hwnd)
                .map_err(backend_error)?;
            let helper_accepted = outcome == HelperOutcome::Accepted;
            helper = Some(outcome);
            if let Some(reached) = self.settle(&w, helper_accepted, before, state, ctx)? {
                ctx.focused.push(FocusInfo::new(i, &reached, true));
                return Ok(());
            }
            if !still_there(self.desktop)? {
                return Err(closed(helper.as_ref()));
            }
            fg = read_fg(self.desktop);
        }
        let set_foreground = u8::from(accepted);
        let fg_process = fg
            .as_ref()
            .map(|f| f.process.clone())
            .filter(|p| !p.is_empty());
        let is_host = fg.as_ref().is_some_and(|f| ctx.host_pids.contains(&f.pid));
        let mut detail = helper_detail(helper.as_ref());
        detail["set_foreground"] = serde_json::json!(set_foreground);
        detail["foreground_process"] = serde_json::json!(fg_process);
        detail["foreground_is_host"] = serde_json::json!(is_host);
        let now = fg_process.unwrap_or_else(|| "(不明)".into());
        let (mut message, why, situation) = match fg {
            Some(_) if is_host => (
                format!(
                    "対象 ({describe}) を前面にできなかった (SetForegroundWindow={set_foreground}。前面は ai-cu を起動した側 ({now}))。この ai-cu には前面化の権利が無い (常駐している、または起動の後に人が操作した)"
                ),
                reason::REFUSED_HOST_FRONT,
                Hint::FocusHostFront,
            ),
            Some(_) => (
                format!(
                    "対象 ({describe}) を前面にできなかった (SetForegroundWindow={set_foreground}。前面は {now})。人が別のアプリを使っている"
                ),
                reason::REFUSED_OTHER_APP,
                Hint::FocusRefused,
            ),
            None => (
                format!(
                    "対象 ({describe}) を前面にできなかった (SetForegroundWindow={set_foreground}。前面のウィンドウを確かめられない)"
                ),
                reason::REFUSED_UNKNOWN,
                Hint::FocusRefusedUnknown,
            ),
        };
        if let Some(h) = &helper {
            let what = match h {
                HelperOutcome::Failed(why) => format!("failed: {why}"),
                other => other.label().to_string(),
            };
            message.push_str(&format!(
                "。前面化の補助プロセスでも前面にできなかった ({what})"
            ));
        }
        Err(AppError::new(ErrorKind::FocusFailed, message)
            .with_reason(why)
            .with_detail(detail)
            .with_hint(hint(situation, ctx.surface)))
    }

    /// 許可されたウィンドウに `WM_CLOSE` を 1 回置く (D-062)。入力は送らず、前面も変えないので、前面のガードは
    /// 掛けない。対象 (または対象に所有されたダイアログ) が前面で人が操作中なら、人が使っているウィンドウを
    /// 取り上げないよう 10 で断る。置いた後は消えるのを短く (`CLOSE_SETTLE_MS`) 待ち、見える窓から消えたか (`gone`) と、
    /// 破棄されず同じプロセスのまま隠れただけか (`hidden`。トレイに入るアプリ) を `closed` に残す (消えなくても成功。
    /// 保存の確認が出ていることが多い)。壊れたら待たずに抜け、隠れている間は上限まで待ってから決める (D-096)。
    fn close(
        &mut self,
        i: usize,
        process: &str,
        title: &str,
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), AppError> {
        let not_found = |message: String| {
            AppError::new(ErrorKind::WindowNotFound, message)
                .with_hint(hint(Hint::CloseNotFound, ctx.surface))
        };
        // 名指しの候補は `windows` に出る窓と TUI の端末を除いた窓だけ (D-139)
        let (policy, host_pids, self_hwnd) = (self.policy, ctx.host_pids, ctx.self_hwnd);
        let Some(w) = self
            .desktop
            .find_window(process, Some(title), &|w| {
                named_window_admitted(policy, w, host_pids, self_hwnd)
            })
            .map_err(backend_error)?
        else {
            return Err(not_found(format!(
                "閉じる対象が見つからない (process={process:?}, title={title:?})。{NAMED_WINDOW_SCOPE_NOTE}"
            )));
        };
        // 閉じてよい相手か。フォアグラウンドガードと同じ判定 (設定、自分の端末、起動した側、管理者権限)。権限の
        // ほかは述語を通った窓には当たらない (守りの重ね。D-139)
        self.check_window(&w.info, ctx, "閉じる対象", Hint::CloseDenied)?;
        let describe = format!(
            "process={:?}, title={:?}, hwnd={:#x}",
            w.info.process, w.info.title, w.info.hwnd
        );
        // モーダルのダイアログを開いている (無効な) ウィンドウは、閉じるボタンも押せない。click と同じ範囲に留める
        if !self
            .desktop
            .window_enabled(w.info.hwnd)
            .map_err(backend_error)?
        {
            return Err(AppError::new(
                ErrorKind::ForegroundGuard,
                format!("対象 ({describe}) は無効 (多くはダイアログを開いている) なので閉じない"),
            )
            .with_reason(reason::TARGET_DISABLED)
            .with_hint(hint(Hint::CloseDisabled, ctx.surface)));
        }
        let min_idle = self.policy.capture_min_idle_ms();
        if min_idle > 0 {
            let in_front = self
                .desktop
                .foreground_window()
                .map_err(backend_error)?
                .is_some_and(|fg| {
                    fg.pid != 0
                        && fg.pid == w.info.pid
                        && (fg.hwnd == w.info.hwnd
                            || self.desktop.owner_root(fg.hwnd) == w.info.hwnd)
                });
            if in_front {
                let idle = self.desktop.idle_ms().map_err(backend_error)?;
                if user_active(idle, min_idle) {
                    return Err(AppError::new(
                        ErrorKind::UserActive,
                        format!(
                            "対象 ({describe}) が前面にあり、人が操作中なので閉じない (最後の入力から {idle} ms。{min_idle} ms 以上の無操作が要る: capture.min_idle_ms)"
                        ),
                    )
                    .with_detail(idle_detail(idle, min_idle))
                    .with_hint(hint(Hint::CloseUserActive, ctx.surface)));
                }
            }
        }
        if !self
            .desktop
            .post_close(w.info.hwnd)
            .map_err(backend_error)?
        {
            return Err(not_found(format!(
                "対象 ({describe}) に WM_CLOSE を置けなかった (閉じられたか、置けない相手)"
            )));
        }
        // WM_CLOSE を置いた (この後の待ちで止まっても、閉じる依頼は届いている。D-065)
        ctx.partial = true;
        // 壊れたらすぐ抜ける。見える間と隠れた間は上限まで読み直す (壊す途中で一瞬隠れた窓を「隠れただけ」と
        // 取り違えない。D-096)。抜けた時の最後の状態で決める
        let mut waited = 0;
        let last = loop {
            let presence = self
                .desktop
                .window_presence(w.info.hwnd, w.info.pid)
                .map_err(backend_error)?;
            if presence == WindowPresence::Destroyed || waited >= CLOSE_SETTLE_MS {
                break presence;
            }
            self.clock.sleep_ms(STOP_POLL_MS);
            waited += STOP_POLL_MS;
            self.check_stop(state, ctx)?;
        };
        let (gone, hidden) = match last {
            WindowPresence::Destroyed => (true, false),
            WindowPresence::Hidden => (true, true),
            WindowPresence::Visible => (false, false),
        };
        ctx.closed.push(CloseInfo {
            action_index: i,
            hwnd: w.info.hwnd,
            process: w.info.process.clone(),
            title: w.info.title.clone(),
            gone,
            hidden,
        });
        Ok(())
    }

    /// 条件がそろうまで待つ (D-068)。読むだけで何も送らない。最初の判定は待たずに行い、満たされなければ
    /// `WAIT_FOR_POLL_MS` (残りが短ければ残り) だけ `sleep` (50 ms ごとに停止要因と期限) して確かめ直す。
    /// 経過が `timeout_ms` 以上なら 15 (期限の時点で最後に 1 回確かめる)。窓の名指しは `windows` に出る窓 (TUI の
    /// 端末も除く) だけから探すので、許可の外と起動した側の窓は「無い」と数える (`present: true` なら時間切れまで
    /// 待って 15、`absent` なら満たされる。D-139)。名前だけの判定 (4) は検証 (validate) が先に掛ける
    fn wait_for(
        &mut self,
        i: usize,
        spec: &WaitSpec<'_>,
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), AppError> {
        let start = self.clock.now_ms();
        let mut checks = 0usize;
        loop {
            let obs = self.observe_wait(spec, ctx)?;
            checks += 1;
            let now = self.clock.now_ms();
            if obs.present != spec.absent {
                ctx.waited.push(WaitInfo {
                    action_index: i,
                    elapsed_ms: now.saturating_sub(start),
                    checks,
                    window: obs.window.as_ref().map(|w| ShotWindow::of(&w.info)),
                    rect: None,
                    changed_ratio: None,
                    // 要素は「ある」で満たされたときだけ (absent + text が「要素はあるが含まない」で満たされたときは入れない)
                    element: if obs.present {
                        obs.element.as_ref().map(element_json)
                    } else {
                        None
                    },
                    // 「最初から無い」は待つ意味が無かったか、条件がアプリに見えていない合図 (D-070)。
                    // timeout_ms 0 は「1 回だけ確かめる」と明示した使い方なので付けない
                    note: (spec.absent && checks == 1 && spec.timeout_ms > 0)
                        .then(|| absent_note(obs.unmet)),
                });
                return Ok(());
            }
            let elapsed = now.saturating_sub(start);
            if elapsed >= spec.timeout_ms {
                let state_word = if spec.absent {
                    "まだある"
                } else {
                    "無い"
                };
                let mut message = format!(
                    "条件 ({}) が {} ms のうちに満たされなかった (確かめた回数 {checks}。最後の判定で {} が{state_word})",
                    spec.describe(),
                    spec.timeout_ms,
                    obs.unmet
                );
                // 窓の段で満たされなかったときだけ、名指しの範囲を足す (15 の hint は画面の待ちと共有なので変えない。D-139)
                if obs.unmet == "window" {
                    message.push_str(&format!("。{NAMED_WINDOW_SCOPE_NOTE}"));
                }
                return Err(AppError::new(ErrorKind::WaitTimeout, message).with_detail(
                    serde_json::json!({
                        "timeout_ms": spec.timeout_ms,
                        "checks": checks,
                        "unmet": obs.unmet,
                        "window": obs.window.as_ref().map(|w| window_detail(&w.info)),
                    }),
                ));
            }
            self.sleep(WAIT_FOR_POLL_MS.min(spec.timeout_ms - elapsed), state, ctx)?;
        }
    }

    /// `wait_for` の 1 回の判定 (D-068)。ウィンドウを毎回引き直す (待っている間に作り直されることがある)。
    /// 窓は `named_window_admitted` を通る窓 (`windows` に出る窓から TUI の端末を除いたもの) だけから探す (D-139)。
    /// 無い、許可の外か起動した側の窓しか無い、または最小化なら「ウィンドウが無い」。あれば、読んでよい相手か
    /// (`check_ui_window`。述語を通った窓には当たらない守りの重ね)。
    /// 条件が要素なら 1 つ以上あるか、文字列なら最初に文字列を持つ要素の文字列が含むか (要素の文字列と条件の
    /// 文字列の両方を LF にそろえてから比べる)。
    /// 要素の読み取りが失敗したら、ウィンドウを探し直して無ければその回は「無い」(閉じられた直後の競合)、
    /// まだあれば失敗 (多くは 8)
    fn observe_wait(
        &self,
        spec: &WaitSpec<'_>,
        ctx: &Ctx<'_>,
    ) -> Result<WaitObservation, AppError> {
        let no_window = || WaitObservation {
            present: false,
            unmet: "window",
            window: None,
            element: None,
        };
        let (policy, host_pids, self_hwnd) = (self.policy, ctx.host_pids, ctx.self_hwnd);
        let find = |d: &dyn Desktop| -> Result<Option<TopWindow>, AppError> {
            Ok(d.find_window(spec.process, spec.title, &|w| {
                named_window_admitted(policy, w, host_pids, self_hwnd)
            })
            .map_err(backend_error)?
            .filter(|w| !w.minimized))
        };
        let Some(w) = find(self.desktop)? else {
            return Ok(no_window());
        };
        check_ui_window(self.policy, &w, ctx.host_pids, ctx.self_hwnd, &spec.shown())?;
        let (name, control_type) = match spec.element {
            Some(f) => (
                f.name.clone(),
                f.control_type
                    .as_deref()
                    .and_then(parse_control_type)
                    .map(str::to_string),
            ),
            None => (None, None),
        };
        let query = if spec.text.is_some() {
            ElementQuery::for_text(name, control_type)
        } else if spec.element.is_some() {
            ElementQuery {
                name,
                control_type,
                limit: 1,
                automation_id: None,
                focused: false,
                with_text: false,
            }
        } else {
            return Ok(WaitObservation {
                present: true,
                unmet: "window",
                window: Some(w),
                element: None,
            });
        };
        let found = match self.desktop.ui_elements(w.info.hwnd, &query) {
            Ok(f) => f,
            // 読む直前に閉じられた (absent でダイアログが消えるのを待つときの競合) なら、その回は「無い」
            Err(e) => {
                return if find(self.desktop)?.is_none() {
                    Ok(no_window())
                } else {
                    Err(backend_error(e))
                };
            }
        };
        let (present, unmet, element) = match spec.text {
            // 条件の文字列も LF にそろえる (CRLF で書かれた text が決して一致せずに 15 になるのを防ぐ)
            Some(needle) => match first_text(&found.elements) {
                None => (false, "element", None),
                Some((el, s)) => (
                    s.contains(&normalize_newlines(needle)),
                    "text",
                    Some(el.clone()),
                ),
            },
            None => match found.elements.first() {
                Some(el) => (true, "element", Some(el.clone())),
                None => (false, "element", None),
            },
        };
        Ok(WaitObservation {
            present,
            unmet,
            window: Some(w),
            element,
        })
    }

    /// 前面化を頼んだ後の待ち。受け付けられたら、前面になるのを短く (`FOCUS_SETTLE_MS`) 待つ (その間も停止要因は
    /// 見る)。断られたら 1 回だけ確かめて終わり。前面になれば、その前面のウィンドウ。`before` は呼ぶ前の前面の hwnd
    fn settle(
        &mut self,
        w: &TopWindow,
        accepted: bool,
        before: Option<usize>,
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<Option<WindowInfo>, AppError> {
        let mut waited = 0;
        let mut before = before;
        loop {
            if let Some(fg) = self.reached(w, ctx, true, &mut before)? {
                return Ok(Some(fg));
            }
            if !accepted || waited >= FOCUS_SETTLE_MS {
                return Ok(None);
            }
            self.clock.sleep_ms(STOP_POLL_MS);
            waited += STOP_POLL_MS;
            self.check_stop(state, ctx)?;
        }
    }

    /// 対象が前面になったか (なっていれば、その前面のウィンドウ)。前面のウィンドウの hwnd と pid の両方が対象と
    /// 同じなら成功 (hwnd だけの一致は、閉じられて再利用されたハンドルを誤って成功にしうる)。別の hwnd なら、
    /// 対象に所有されたウィンドウ (モーダルダイアログなど。`GA_ROOTOWNER` が対象) のときだけ、続くキーがそこに
    /// 届くので、それをフォアグラウンドガードと同じ判定に通し、通れば成功、通らなければ 7 (`strict`。呼ぶ前の
    /// 判定では「まだ前面にしていない」ので、拒否は不成立として呼びに進む)。同じプロセスでも所有関係の無い
    /// 別のウィンドウ (Chrome の別のウィンドウなど) は成功にしない (断られて何も変わらなかった場合を含む)。
    /// `before` は呼ぶ前の前面の hwnd。呼ぶ前の判定 (`strict` でない) で読んだ値を書き、呼んだ後の判定では
    /// 前面がそれと違うときだけ partial を立てる (前面が実際に変わった。D-065)
    fn reached(
        &self,
        w: &TopWindow,
        ctx: &mut Ctx<'_>,
        strict: bool,
        before: &mut Option<usize>,
    ) -> Result<Option<WindowInfo>, AppError> {
        let fg = self.desktop.foreground_window().map_err(backend_error)?;
        if !strict {
            *before = fg.as_ref().map(|f| f.hwnd);
        }
        let Some(fg) = fg else {
            return Ok(None);
        };
        if fg.pid == 0 || fg.pid != w.info.pid {
            return Ok(None);
        }
        // 呼んだ後に前面が対象 (かそのダイアログ) に変わった: 前面化の副作用は起きている (この後の判定で 7 に
        // なっても)。拒否されたダイアログが呼ぶ前から前面だったなら、何も変わっていないので立てない
        let switched = strict && Some(fg.hwnd) != *before;
        if fg.hwnd == w.info.hwnd {
            if switched {
                ctx.partial = true;
            }
            return Ok(Some(fg));
        }
        if self.desktop.owner_root(fg.hwnd) != w.info.hwnd {
            return Ok(None);
        }
        if switched {
            ctx.partial = true;
        }
        match self.check_window(&fg, ctx, "前面", Hint::FocusDenied) {
            Ok(()) => Ok(Some(fg)),
            Err(e) if strict => Err(e),
            Err(_) => Ok(None),
        }
    }

    /// 届く先 (または前面にする対象) が許可されたウィンドウか。`situation` は設定で拒否されたときの次の手。
    /// 断ったときは、判定に使ったウィンドウを `error.detail.window` に入れる (題名は入れない。D-065)
    fn check_window(
        &self,
        w: &WindowInfo,
        ctx: &Ctx<'_>,
        place: &str,
        situation: Hint,
    ) -> Result<(), AppError> {
        self.judge_window(w, ctx, place, situation)
            .map_err(|e| e.with_detail(serde_json::json!({ "window": window_detail(w) })))
    }

    /// 窓の判定 (自由関数の `judge_window`) に、この実行の `Ctx` から作った材料を渡す
    fn judge_window(
        &self,
        w: &WindowInfo,
        ctx: &Ctx<'_>,
        place: &str,
        situation: Hint,
    ) -> Result<(), AppError> {
        let guard = WindowGuard {
            self_hwnd: ctx.self_hwnd,
            host_pids: ctx.host_pids,
            self_elevated: ctx.self_elevated,
            surface: ctx.surface,
        };
        judge_window(self.policy, w, &guard, place, situation)
    }

    /// screenshot。`settle` があれば、撮る前に `wait_stable` と同じ判定 (`watch_screen`) で落ち着くのを待ち、
    /// 落ち着いたときか時間切れのときの最後のフレームを縮めて保存する (撮り直さない。時間切れも成功。D-081)
    fn capture(
        &mut self,
        i: usize,
        spec: &CaptureSpec<'_>,
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), AppError> {
        let (rect, window, img, settled, settle_ms) = match spec.settle {
            Some(s) => {
                let watch = ScreenSpec {
                    rect: spec.rect,
                    window: spec.window,
                    until: Until::Stable {
                        stable_ms: s.stable_ms,
                    },
                    threshold: s.threshold,
                    timeout_ms: s.timeout_ms,
                };
                let w = self.watch_screen(&watch, state, ctx)?;
                (w.rect, w.window, w.frame, Some(w.met), Some(w.elapsed_ms))
            }
            None => {
                let (rect, window, img, _grace) = self.grab(spec.rect, spec.window, state, ctx)?;
                (rect, window, img, None, None)
            }
        };
        let window = window.as_ref().map(ShotWindow::of);
        // 長辺の上限 (D-072): 矩形が決まってから、scale と max_edge / 長辺 の小さい方にする。結果の scale もこの値
        let scale = crate::image::effective_scale(spec.scale, spec.max_edge, rect.w, rect.h);
        let img = crate::image::downscale(&img, scale);
        let saved = self
            .images
            .write_png(spec.path, &img)
            .map_err(|m| AppError::new(ErrorKind::OsError, format!("画像を保存できない: {m}")))?;
        ctx.shots.push(ShotInfo {
            action_index: i,
            path: saved,
            x: rect.x,
            y: rect.y,
            width: img.width,
            height: img.height,
            scale,
            window,
            settled,
            settle_ms,
        });
        Ok(())
    }

    /// 1 回の撮影 (D-081。screenshot と画面の待ちが共有する)。撮影と同じ判定を順に通す: 入力デスクトップ
    /// (撮影は前面のガードを通らないので、途中でロックやセーバーになった画面を成功として返さない。D-067) →
    /// `window` なら撮る直前に矩形を解く (D-053。撮影の矩形の規則もその中で通す。`window` が隠れていれば
    /// `OCCLUDED_GRACE_MS` まで 50 ms ごとに入力デスクトップの判定から解き直す。D-082)、region と全画面なら撮影の
    /// 矩形の規則 → deny の窓が見えていないか (D-077。window の経路も通す。隠れの確認の間引きが見ない所を拾う) →
    /// 撮る。返すのは撮った矩形、ウィンドウ、縮める前の画像と、使った隠れの猶予の長さ (ms。使わなければ 0)
    fn grab(
        &mut self,
        rect: Option<Rect>,
        window: Option<&WindowTarget>,
        state: &mut State,
        ctx: &Ctx<'_>,
    ) -> Result<(Rect, Option<WindowInfo>, Image, u64), AppError> {
        // 最初に隠れが見えた時刻 (猶予の起点。D-082)
        let mut occluded_since: Option<u64> = None;
        let (rect, window) = loop {
            self.check_input_desktop()?;
            let Some(t) = window else {
                let r = rect.unwrap_or(ctx.screen);
                self.policy
                    .check_capture_rect(&r)
                    .map_err(|d| d.into_error(ErrorKind::PolicyDenied))?;
                break (r, None);
            };
            match resolve_capture_window_parts(
                self.desktop,
                self.policy,
                ctx.screen,
                t,
                ctx.host_pids,
                ctx.self_hwnd,
                ctx.surface,
            )? {
                Resolved::Clear(r, w) => break (r, Some(w)),
                Resolved::Occluded { cover, point, rect } => {
                    let now = self.clock.now_ms();
                    let waited = now.saturating_sub(*occluded_since.get_or_insert(now));
                    if waited >= OCCLUDED_GRACE_MS {
                        // 重なりの一覧は猶予を使い切ったこの 1 回だけ読む (周回ごとには読まない。D-135)
                        return Err(occluded_error(
                            self.desktop,
                            t,
                            &cover,
                            point,
                            &rect,
                            ctx.surface,
                            Some(waited),
                        ));
                    }
                    self.sleep(STOP_POLL_MS.min(OCCLUDED_GRACE_MS - waited), state, ctx)?;
                }
            }
        };
        let grace = occluded_since.map_or(0, |since| self.clock.now_ms().saturating_sub(since));
        check_sensitive_capture(
            self.desktop,
            self.policy,
            &rect,
            ctx.host_pids,
            ctx.self_hwnd,
            ctx.surface,
        )?;
        let img = self.desktop.capture(rect).map_err(backend_error)?;
        Ok((rect, window, img, grace))
    }

    /// 画面が落ち着く (`Until::Stable`) か変わる (`Until::Change`) まで撮って比べる (D-081)。何も送らない。
    /// 最初の撮影は待たずに行い (`window` が隠れて見えたときは猶予だけ待つ。D-082)、以後は `WAIT_SCREEN_POLL_MS`
    /// (残りが短ければ残り) だけ `sleep` (50 ms ごとに
    /// 停止要因と期限) してから撮る。比べるのは縮めた画像 (`thumb`) どうしで、矩形が違えば (窓が動いた、大きさが
    /// 変わった) 差は 1.0。stable は直前のフレームと比べ、差が閾値を超えるたびに静けさの始まりをその時刻へ進め、
    /// 静けさが `stable_ms` 続けば満たす。change は最初のフレーム (基準) と比べ、差が閾値を超えれば満たす。
    /// 満たされたか、経過が `timeout_ms` 以上になったら、最後の矩形と差を返す (時間切れの扱いは呼ぶ側)。
    /// 撮るたびに `grab` の判定を通し、当たれば止める (隠れだけは `grab` が猶予の間解き直す。最初のフレームの
    /// 猶予は起点をずらし、`timeout_ms` を食わない。2 枚目以降の猶予は経過に入り、stable では静けさに数えない。D-082)
    fn watch_screen(
        &mut self,
        spec: &ScreenSpec<'_>,
        state: &mut State,
        ctx: &Ctx<'_>,
    ) -> Result<ScreenWatch, AppError> {
        let mut start = self.clock.now_ms();
        let mut checks = 0usize;
        // stable は直前、change は基準の (矩形, 縮めた画像)
        let mut reference: Option<(Rect, Image)> = None;
        let mut quiet_since = start;
        let mut last_diff = None;
        loop {
            let (rect, window, frame, grace) = self.grab(spec.rect, spec.window, state, ctx)?;
            checks += 1;
            let now = self.clock.now_ms();
            let small = crate::image::thumb(&frame);
            let mut met = false;
            match &reference {
                // 最初のフレーム: 静けさはここから数える。隠れの猶予の分だけ起点を後ろへずらす (D-082)
                None => {
                    start += grace;
                    quiet_since = now;
                }
                Some((prev_rect, prev)) => {
                    // 途中の隠れの猶予の間は見ていないので、静けさに数えない (D-082)
                    if grace > 0 && matches!(spec.until, Until::Stable { .. }) {
                        quiet_since = now;
                    }
                    let diff = if *prev_rect == rect {
                        crate::image::frame_diff(prev, &small)
                    } else {
                        1.0
                    };
                    last_diff = Some(diff);
                    match spec.until {
                        Until::Stable { stable_ms } => {
                            if !crate::image::is_quiet(diff, spec.threshold) {
                                quiet_since = now;
                            } else if now.saturating_sub(quiet_since) >= stable_ms {
                                met = true;
                            }
                        }
                        Until::Change => met = crate::image::is_changed(diff, spec.threshold),
                    }
                }
            }
            if matches!(spec.until, Until::Stable { .. }) || reference.is_none() {
                reference = Some((rect, small));
            }
            let elapsed = now.saturating_sub(start);
            if met || elapsed >= spec.timeout_ms {
                return Ok(ScreenWatch {
                    met,
                    checks,
                    elapsed_ms: elapsed,
                    rect,
                    window,
                    last_diff,
                    frame,
                });
            }
            self.sleep(
                WAIT_SCREEN_POLL_MS.min(spec.timeout_ms - elapsed),
                state,
                ctx,
            )?;
        }
    }

    /// `wait_stable` / `wait_change` (D-081)。満たされたら `waited` に積み、時間切れなら 15 (`unmet` は
    /// `"stable"` / `"change"`、`detail` に最後の矩形と差。窓は題名なし)。hint は画面の待ちの文
    fn wait_screen(
        &mut self,
        i: usize,
        spec: &ScreenSpec<'_>,
        state: &mut State,
        ctx: &mut Ctx<'_>,
    ) -> Result<(), AppError> {
        let w = self.watch_screen(spec, state, ctx)?;
        let change = spec.until == Until::Change;
        if w.met {
            ctx.waited.push(WaitInfo {
                action_index: i,
                elapsed_ms: w.elapsed_ms,
                checks: w.checks,
                window: w.window.as_ref().map(ShotWindow::of),
                rect: Some(w.rect),
                changed_ratio: if change {
                    w.last_diff.map(round_ratio)
                } else {
                    None
                },
                element: None,
                note: None,
            });
            return Ok(());
        }
        let (unmet, what) = if change {
            ("change", "変わらなかった")
        } else {
            ("stable", "落ち着かなかった")
        };
        Err(AppError::new(
            ErrorKind::WaitTimeout,
            format!(
                "画面 (矩形 {}) が {} ms のうちに{what} (撮った回数 {}。最後の差 {})",
                w.rect,
                spec.timeout_ms,
                w.checks,
                w.last_diff
                    .map_or("なし".to_string(), |d| round_ratio(d).to_string())
            ),
        )
        .with_hint(hint(Hint::ScreenWait, ctx.surface))
        .with_detail(serde_json::json!({
            "timeout_ms": spec.timeout_ms,
            "checks": w.checks,
            "unmet": unmet,
            "window": w.window.as_ref().map(window_detail),
            "rect": w.rect,
            "last_changed_ratio": w.last_diff.map(round_ratio),
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::config::{Config, Layer};
    use crate::estop::StopSource;

    use crate::testing::{
        LockEvent, MemoryImages, MockClock, MockDesktop, MockIndicator, MockInputLock, MockSink,
        World, window,
    };

    const ENABLED: &str = "[general]\ninput_enabled = true\ncapture_enabled = true\n";

    /// focus の候補 (一覧のウィンドウ)
    fn top(hwnd: usize, process: &str, title: &str) -> TopWindow {
        let mut info = window(process, "TargetClass");
        info.hwnd = hwnd;
        info.title = title.into();
        TopWindow {
            info,
            rect: Some(Rect::new(100, 100, 800, 600)),
            minimized: false,
        }
    }

    /// 別のプロセスのウィンドウ (モックの `window` は pid が同じなので、別にする)
    fn other(process: &str, hwnd: usize, pid: u32) -> WindowInfo {
        let mut t = window(process, "Chrome_WidgetWin_1");
        t.hwnd = hwnd;
        t.pid = pid;
        t
    }

    fn terminal() -> WindowInfo {
        other("Zed.exe", 0x10, 1)
    }

    fn chrome() -> WindowInfo {
        other("chrome.exe", 0x11, 2)
    }

    fn policy(toml: &str) -> Policy {
        let cfg = Config::parse(&format!("version = 1\n{toml}")).unwrap();
        Policy::new(vec![Layer::from_config(&cfg, "t").unwrap()])
    }

    fn script(actions: &str) -> Script {
        Script::from_json(&format!("{{\"version\":1,\"actions\":[{actions}]}}")).unwrap()
    }

    fn exec(
        world: &Rc<RefCell<World>>,
        policy: &Policy,
        stops: &mut StopSet,
        s: &Script,
    ) -> (Result<RunReport, AppError>, MemoryImages) {
        exec_excluding(world, policy, stops, s, None)
    }

    fn exec_excluding(
        world: &Rc<RefCell<World>>,
        policy: &Policy,
        stops: &mut StopSet,
        s: &Script,
        self_window: Option<&WindowInfo>,
    ) -> (Result<RunReport, AppError>, MemoryImages) {
        exec_with(world, policy, stops, s, self_window, &[])
    }

    fn exec_with(
        world: &Rc<RefCell<World>>,
        policy: &Policy,
        stops: &mut StopSet,
        s: &Script,
        self_window: Option<&WindowInfo>,
        host_pids: &[u32],
    ) -> (Result<RunReport, AppError>, MemoryImages) {
        let mut sink = MockSink(world.clone());
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        let r = Executor {
            policy,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops,
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute_with(s, self_window, host_pids);
        (r, images)
    }

    /// 呼ばれ方 (残す時間 0、予告あり)
    fn opts<'w>(
        self_window: Option<&'w WindowInfo>,
        host_pids: &'w [u32],
        surface: Surface,
    ) -> ExecOptions<'w> {
        ExecOptions {
            self_window,
            host_pids,
            surface,
            linger_ms: 0,
            skip_lead: false,
        }
    }

    /// `opts` で呼ぶ (枠と予告の間のテスト。D-093)。時計と枠はモック
    fn exec_opts(
        world: &Rc<RefCell<World>>,
        policy: &Policy,
        stops: &mut StopSet,
        s: &Script,
        opts: &ExecOptions<'_>,
    ) -> Result<RunReport, AppError> {
        let mut sink = MockSink(world.clone());
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        Executor {
            policy,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops,
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute_on(s, opts)
    }

    struct StopAfterPolls(u32);

    impl StopSource for StopAfterPolls {
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

    fn downs_and_ups(world: &World) -> (usize, usize) {
        let ev = world.all_events();
        let downs = ev
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    InputEvent::Button { down: true, .. } | InputEvent::Key { down: true, .. }
                )
            })
            .count();
        let ups = ev
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    InputEvent::Button { down: false, .. } | InputEvent::Key { down: false, .. }
                )
            })
            .count();
        (downs, ups)
    }

    #[test]
    fn run_executes_all_actions_and_captures() {
        let world = World::new().shared();
        let s = script(
            r#"{"type":"click","x":10,"y":10},{"type":"key","keys":"ctrl+s"},{"type":"wait","ms":120},{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":4,"h":3}}"#,
        );
        let (r, images) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert_eq!(report.executed, 4);
        assert_eq!(report.screenshots[0].width, 4);
        assert_eq!(images.written.len(), 1);
        assert_eq!(world.borrow().sent.len(), 2);
        // click の直後の key の前に待つ (D-059)
        assert_eq!(
            world.borrow().now_ms,
            crate::plan::CLICK_SETTLE_MS + crate::plan::KEY_SETTLE_MS + 120
        );
    }

    #[test]
    fn run_policy_denied_sends_nothing() {
        let world = World::new().shared();
        let s = script(r#"{"type":"click","x":10,"y":10}"#);
        let (r, _) = exec(&world, &Policy::closed(), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::PolicyDenied);
        assert!(world.borrow().sent.is_empty());
    }

    #[test]
    fn estop_stop_source_halts_before_next_batch_and_releases_buttons() {
        let world = World::new().shared();
        // drag の途中で止める。押したボタンは離されていること
        let s = script(
            r#"{"type":"drag","from":{"x":10,"y":10},"to":{"x":200,"y":10},"steps":10,"duration_ms":100}"#,
        );
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(6)));
        let (r, _) = exec(&world, &policy(ENABLED), &mut stops, &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        assert_eq!(e.action_index, Some(0));
        let w = world.borrow();
        assert_eq!(
            downs_and_ups(&w),
            (1, 1),
            "停止後にボタンを離す: {:?}",
            w.sent
        );
        assert!(matches!(
            w.sent.last().unwrap()[..],
            [InputEvent::Button { down: false, .. }]
        ));
    }

    #[test]
    fn estop_user_mouse_movement_stops_run() {
        let mut w = World::new();
        // 最初のバッチの後、利用者がカーソルを大きく動かした
        w.user_moves.push((0, Point::new(1500, 900)));
        let world = w.shared();
        let s = script(r#"{"type":"move","x":10,"y":10},{"type":"move","x":20,"y":20}"#);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        assert!(e.message.contains("マウス"), "{}", e.message);
        assert_eq!(world.borrow().sent.len(), 1);
    }

    #[test]
    fn estop_small_cursor_jitter_does_not_stop() {
        let mut w = World::new();
        w.user_moves.push((0, Point::new(12, 13)));
        let world = w.shared();
        let s = script(r#"{"type":"move","x":10,"y":10},{"type":"move","x":20,"y":20}"#);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert!(r.is_ok(), "{r:?}");
    }

    /// n 回目の poll で、利用者がカーソルを動かしたことにする
    struct UserMovesAtPoll {
        world: Rc<RefCell<World>>,
        at: u32,
        to: Point,
        polls: u32,
    }

    impl StopSource for UserMovesAtPoll {
        fn poll(&mut self) -> Option<StopReason> {
            self.polls += 1;
            if self.polls == self.at {
                self.world.borrow_mut().cursor = self.to;
            }
            None
        }
    }

    #[test]
    fn estop_user_moving_back_along_long_move_is_detected() {
        let world = World::new().shared();
        // 長い移動 (100,100)->(1000,100) の到着を観測した後、利用者が線分の上を始点側へ戻した
        let s = script(
            r#"{"type":"move","x":1000,"y":100},{"type":"wait","ms":100},{"type":"move","x":1000,"y":110}"#,
        );
        let mut stops = StopSet::new();
        stops.push(Box::new(UserMovesAtPoll {
            world: world.clone(),
            at: 3,
            to: Point::new(300, 100),
            polls: 0,
        }));
        let (r, _) = exec(&world, &policy(ENABLED), &mut stops, &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e:?}");
        assert_eq!(world.borrow().sent.len(), 1);
    }

    #[test]
    fn estop_user_mouse_mid_drag_returns_to_last_checked_point_before_release() {
        let mut w = World::new();
        // ボタンを押した直後 (2 回目の send の後) に利用者が別の場所へ動かした
        w.user_moves.push((1, Point::new(2000, 1500)));
        let world = w.shared();
        let s = script(
            r#"{"type":"drag","from":{"x":10,"y":10},"to":{"x":200,"y":10},"steps":4,"duration_ms":40}"#,
        );
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::Stopped);
        let w = world.borrow();
        let n = w.sent.len();
        assert_eq!(
            w.sent[n - 2],
            vec![InputEvent::MoveTo { x: 10, y: 10 }],
            "{:?}",
            w.sent
        );
        assert!(matches!(
            w.sent[n - 1][..],
            [InputEvent::Button { down: false, .. }]
        ));
    }

    struct FailOnce {
        world: Rc<RefCell<World>>,
        fail_at: usize,
        calls: usize,
    }

    impl InputSink for FailOnce {
        fn send(&mut self, events: &[InputEvent]) -> Result<(), BackendError> {
            self.calls += 1;
            if self.calls == self.fail_at {
                return Err(BackendError::Os("partial".into()));
            }
            MockSink(self.world.clone()).send(events)
        }
    }

    #[test]
    fn estop_partial_sendinput_failure_still_releases_keys() {
        let world = World::new().shared();
        let p = policy(ENABLED);
        let mut sink = FailOnce {
            world: world.clone(),
            fail_at: 1,
            calls: 0,
        };
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        let r = Executor {
            policy: &p,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut StopSet::new(),
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute(&script(r#"{"type":"key","keys":"ctrl+a"}"#));
        assert_eq!(r.unwrap_err().kind, ErrorKind::OsError);
        let ev = world.borrow().all_events();
        assert!(
            ev.contains(&InputEvent::Key {
                vk: 0x11,
                down: false,
                extended: false
            }),
            "{ev:?}"
        );
        assert!(
            ev.iter().all(|e| e.is_release()),
            "後始末は離す方向だけ: {ev:?}"
        );
    }

    #[test]
    fn estop_failed_click_releases_at_its_own_point_not_the_previous_one() {
        let world = World::new().shared();
        let p = policy(ENABLED);
        // 1 回目 (A のクリック) は成功、2 回目 (B のクリック) が OS の失敗
        let mut sink = FailOnce {
            world: world.clone(),
            fail_at: 2,
            calls: 0,
        };
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        let r = Executor {
            policy: &p,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut StopSet::new(),
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute(&script(
            r#"{"type":"click","x":10,"y":10},{"type":"click","x":500,"y":400}"#,
        ));
        assert_eq!(r.unwrap_err().kind, ErrorKind::OsError);
        let w = world.borrow();
        let after_failure: Vec<InputEvent> = w.sent[1..].concat();
        assert!(
            !after_failure.contains(&InputEvent::MoveTo { x: 10, y: 10 }),
            "A へ戻らない: {:?}",
            w.sent
        );
        assert!(
            after_failure
                .iter()
                .any(|e| matches!(e, InputEvent::Button { down: false, .. }))
        );
    }

    struct Jump(Rc<RefCell<World>>);

    impl StopSource for Jump {
        fn poll(&mut self) -> Option<StopReason> {
            self.0.borrow_mut().now_ms += 400;
            None
        }
    }

    #[test]
    fn estop_deadline_stops_long_run() {
        let world = World::new().shared();
        let s = script(r#"{"type":"wait","ms":300},{"type":"click","x":1,"y":1}"#);
        // 検証は待ちの合計 300 ms + 余裕 1000 ms <= 2000 で通るが、実行中に時計を進めて期限を超えさせる
        let p = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 2000\n"));
        let mut stops = StopSet::new();
        stops.push(Box::new(Jump(world.clone())));
        let (r, _) = exec(&world, &p, &mut stops, &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        assert!(e.message.contains("最大実行時間"), "{}", e.message);
        assert!(world.borrow().sent.is_empty());
    }

    #[test]
    fn estop_sentinel_present_before_start_sends_nothing() {
        let f = crate::testing::unique_temp_dir("estop-run").join("STOP");
        std::fs::write(&f, b"").unwrap();
        let world = World::new().shared();
        let mut stops = StopSet::new();
        stops.push(Box::new(crate::estop::SentinelStop::new(vec![f])));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut stops,
            &script(r#"{"type":"click","x":1,"y":1}"#),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::Stopped);
        assert!(world.borrow().sent.is_empty());
    }

    #[test]
    fn foreground_guard_rejects_keys_when_foreground_not_allowed() {
        let mut w = World::new();
        w.foreground = Some(window("cmd.exe", "ConsoleWindowClass"));
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"target\"]\n"));
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::ForegroundGuard);
        assert!(
            world.borrow().sent.is_empty(),
            "前面が対象でなければキーを 1 つも送らない"
        );
    }

    /// 修飾キーつきのクリックの場: 座標の下は許可の窓 (target)、前面は許可の外の窓 (cmd) (D-137)
    fn modifier_click_world(foreground_allowed: bool) -> (Rc<RefCell<World>>, Policy) {
        let mut w = World::new();
        if !foreground_allowed {
            w.foreground = Some(other("cmd.exe", 0x20, 99));
        }
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"target\"]\n"));
        (w.shared(), p)
    }

    /// 前面が許可の外なら、修飾キーつきのクリックは何も送らずに 7。文に修飾キーの理由、detail は前面の窓 (D-137)
    #[test]
    fn modifier_click_refuses_when_foreground_is_not_allowed() {
        for json in [
            r#"{"type":"click","x":10,"y":10,"modifiers":["ctrl"]}"#,
            r#"{"type":"double_click","x":10,"y":10,"modifiers":["shift"]}"#,
        ] {
            let (world, p) = modifier_click_world(false);
            let (r, _) = exec(&world, &p, &mut StopSet::new(), &script(json));
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
            assert!(e.message.contains("修飾キー"), "{}", e.message);
            assert_eq!(e.action_index, Some(0));
            let detail = e.detail.as_deref().expect("detail");
            assert_eq!(detail["window"]["process"], "cmd.exe", "{detail}");
            assert!(detail.get("point").is_none(), "{detail}");
            assert!(world.borrow().sent.is_empty(), "{:?}", world.borrow().sent);
        }
    }

    /// 同じ場で修飾なしのクリックは前面を見ずに通る (マウスは座標の下の窓に届く。D-033)
    #[test]
    fn plain_click_does_not_check_foreground() {
        let (world, p) = modifier_click_world(false);
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10}"#),
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(world.borrow().sent.len(), 1);
    }

    /// 前面も許可の窓なら、修飾キーつきのクリックは計画のイベント列をそのまま 1 つの束で送る (D-137)
    #[test]
    fn modifier_click_passes_with_allowed_foreground() {
        let json = r#"{"type":"click","x":10,"y":10,"modifiers":["shift","ctrl"]}"#;
        let (world, p) = modifier_click_world(true);
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &script(json));
        assert!(r.is_ok(), "{r:?}");
        let s = script(json);
        let planned: Vec<Vec<InputEvent>> = crate::plan::plan(&s.actions[0])
            .unwrap()
            .into_iter()
            .filter_map(|st| match st {
                Step::Batch { events, .. } => Some(events),
                _ => None,
            })
            .collect();
        assert_eq!(planned.len(), 1);
        assert!(crate::plan::has_key_events(&planned[0]));
        assert_eq!(world.borrow().sent, planned);
    }

    /// 修飾キーつきのクリックの束の送信が OS の誤りで失敗しても、後始末は修飾キーを離す。後始末は離す方向だけで、
    /// 押下 (抑えの 0xE8 も) を送らない (D-019、D-137)
    #[test]
    fn modifier_click_releases_modifiers_after_send_failure() {
        let world = World::new().shared();
        let p = policy(ENABLED);
        let mut sink = FailOnce {
            world: world.clone(),
            fail_at: 1,
            calls: 0,
        };
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        let r = Executor {
            policy: &p,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut StopSet::new(),
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute(&script(
            r#"{"type":"click","x":30,"y":40,"modifiers":["alt","shift"]}"#,
        ));
        assert_eq!(r.unwrap_err().kind, ErrorKind::OsError);
        let w = world.borrow();
        let key_up = |vk: u16| InputEvent::Key {
            vk,
            down: false,
            extended: false,
        };
        assert_eq!(w.sent.len(), 3, "{:?}", w.sent);
        // (1) キーを離す束 (離す方向だけ。抑えの 0xE8 の離しが入るのは許す)
        assert!(w.sent[0].iter().all(|e| e.is_release()), "{:?}", w.sent[0]);
        assert!(w.sent[0].contains(&key_up(0x12)), "{:?}", w.sent[0]);
        assert!(w.sent[0].contains(&key_up(0x10)), "{:?}", w.sent[0]);
        // (2) クリックの点への移動 (3) ボタンを離す
        assert_eq!(w.sent[1], vec![InputEvent::MoveTo { x: 30, y: 40 }]);
        assert!(
            matches!(w.sent[2][..], [InputEvent::Button { down: false, .. }]),
            "{:?}",
            w.sent[2]
        );
        assert!(
            !w.all_events()
                .iter()
                .any(|e| matches!(e, InputEvent::Key { down: true, .. })),
            "後始末に押下は無い: {:?}",
            w.sent
        );
    }

    #[test]
    fn foreground_guard_checks_window_under_each_mouse_point() {
        let mut w = World::new();
        w.windows_at.push((
            Rect::new(500, 0, 100, 100),
            window("explorer.exe", "Shell_TrayWnd"),
        ));
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[deny]\nprocesses=[\"explorer\"]\n"));
        let s = script(r#"{"type":"click","x":10,"y":10},{"type":"click","x":550,"y":50}"#);
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(e.action_index, Some(1));
        assert_eq!(world.borrow().sent.len(), 1);
    }

    #[test]
    fn foreground_guard_rejects_elevated_or_unknown_targets() {
        for elevated in [Some(true), None] {
            let mut w = World::new();
            let mut win = window("taskmgr.exe", "TaskManagerWindow");
            win.elevated = elevated;
            w.foreground = Some(win);
            let world = w.shared();
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(r#"{"type":"type","text":"x"}"#),
            );
            assert_eq!(
                r.unwrap_err().kind,
                ErrorKind::ForegroundGuard,
                "{elevated:?}"
            );
            assert!(world.borrow().sent.is_empty());
        }
    }

    #[test]
    fn foreground_guard_rejects_when_no_foreground_window() {
        let mut w = World::new();
        w.foreground = None;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::ForegroundGuard);
    }

    #[test]
    fn foreground_guard_rejects_self_window_for_keys_and_mouse_without_sending() {
        // 自分の端末が前面 (キー) でも座標の下 (マウス) でも、組み込みで拒否して何も送らない (D-045)
        let mut w = World::new();
        let me = window("WindowsTerminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS");
        w.foreground = Some(me.clone());
        w.default_window = Some(me.clone());
        let world = w.shared();
        let p = policy(ENABLED);
        for s in [
            r#"{"type":"key","keys":"a"}"#,
            r#"{"type":"click","x":10,"y":10}"#,
        ] {
            let (r, _) = exec_excluding(&world, &p, &mut StopSet::new(), &script(s), Some(&me));
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{s}");
            assert_eq!(e.hint.as_deref(), Some(SELF_WINDOW_HINT), "{s}");
            assert!(world.borrow().sent.is_empty(), "{s}");
        }
        // 除外が無ければ、許可リストが空なので従来どおり通る (CLI の動作は変わらない)
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        assert!(r.is_ok(), "{r:?}");
        // 別のウィンドウを除外しても、対象には届く
        let mut other = me.clone();
        other.hwnd = 0x2000;
        let (r, _) = exec_excluding(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10}"#),
            Some(&other),
        );
        assert!(r.is_ok(), "{r:?}");
    }

    /// ホストの除外 (D-055): ai-cu を起動した側のプロセスのウィンドウには、キーも、座標の下でも、focus でも、
    /// screenshot の window でも、設定にかかわらず触れない。同じ実行ファイル名でも pid が違えば届く。
    /// focus と screenshot の window の名指しでは、起動した側の窓は無いものとして 11 の not_found / 12 (D-139)
    #[test]
    fn foreground_guard_host_pid_rejects_keys_mouse_focus_and_capture_target_without_sending() {
        let mut w = World::new();
        let host = other("WindowsTerminal.exe", 0x30, 77);
        w.foreground = Some(host.clone());
        w.default_window = Some(host.clone());
        w.top = vec![TopWindow {
            info: host.clone(),
            rect: Some(Rect::new(0, 0, 800, 600)),
            minimized: false,
        }];
        let world = w.shared();
        let p = policy(ENABLED);
        // focus の名指しでは、起動した側の窓は候補に入らない (11 の not_found。D-139)
        let s = r#"{"type":"focus","process":"WindowsTerminal.exe"}"#;
        let (r, _) = exec_with(&world, &p, &mut StopSet::new(), &script(s), None, &[77]);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed, "{e}");
        assert_eq!(e.reason, Some(reason::NOT_FOUND));
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
        assert!(world.borrow().focus_calls.is_empty());
        for (s, hint) in [
            (r#"{"type":"key","keys":"a"}"#, HOST_WINDOW_HINT),
            (r#"{"type":"click","x":10,"y":10}"#, HOST_WINDOW_HINT),
        ] {
            let (r, images) = exec_with(&world, &p, &mut StopSet::new(), &script(s), None, &[77]);
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{s}");
            assert_eq!(e.hint.as_deref(), Some(hint), "{s}");
            assert!(e.message.contains("起動した側"), "{s}: {}", e.message);
            assert!(world.borrow().sent.is_empty(), "{s}");
            assert!(world.borrow().focus_calls.is_empty(), "{s}");
            assert!(images.written.is_empty(), "{s}");
        }
        // screenshot の window の名指しでは、起動した側の窓は無いものとして 12 (D-139)
        let s =
            r#"{"type":"screenshot","path":"a.png","window":{"process":"WindowsTerminal.exe"}}"#;
        let (r, images) = exec_with(&world, &p, &mut StopSet::new(), &script(s), None, &[77]);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
        assert!(e.detail.is_none(), "{:?}", e.detail);
        assert!(world.borrow().sent.is_empty());
        assert_eq!(world.borrow().capture_calls, 0);
        assert!(images.written.is_empty());
        // 同じ実行ファイル名でも pid が違えば届く (除外は pid で見る)
        let (r, _) = exec_with(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
            None,
            &[78],
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(world.borrow().sent.len(), 1);
    }

    /// 自分の端末 (hwnd) とホスト (pid) の両方に当たるウィンドウでは、TUI 向けの次の手 (SELF_WINDOW_HINT) が勝つ
    #[test]
    fn foreground_guard_self_window_hint_wins_when_window_is_also_host_pid() {
        let mut w = World::new();
        let me = window("WindowsTerminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS");
        w.foreground = Some(me.clone());
        let world = w.shared();
        let (r, _) = exec_with(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
            Some(&me),
            &[me.pid],
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(e.hint.as_deref(), Some(SELF_WINDOW_HINT));
        assert!(world.borrow().sent.is_empty());
    }

    /// focus (D-050): 端末が前面でも、隠れた許可ウィンドウを前面にしてからキーを送れる。focus 自体は何も送らない
    #[test]
    fn focus_brings_hidden_allowed_window_to_front_then_keys_reach_it() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "Notepad.exe", "a.txt - メモ帳")];
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        // focus 無しなら、前面が端末なのでキーは 7 で止まる
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::ForegroundGuard);
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"a"}"#),
        );
        let report = r.unwrap();
        assert_eq!(report.executed, 2);
        assert_eq!(
            report.focused,
            vec![FocusInfo {
                action_index: 0,
                hwnd: 0x20,
                process: "Notepad.exe".into(),
                title: "a.txt - メモ帳".into(),
                helper: false,
            }],
            "前面にしたウィンドウを結果に出す"
        );
        let w = world.borrow();
        assert_eq!(w.focus_calls, vec![0x20]);
        assert_eq!(w.foreground.as_ref().unwrap().hwnd, 0x20);
        assert_eq!(w.sent.len(), 1, "focus 自体は何も送らない: {:?}", w.sent);
    }

    /// 門と MCP の status が共有する述語 (D-049、D-073): min_idle_ms が 0 なら見ない。境界 (idle_ms == min_idle_ms) は操作中でない
    #[test]
    fn user_active_needs_positive_min_idle_and_short_idle() {
        assert!(!user_active(0, 0));
        assert!(!user_active(100, 0));
        assert!(user_active(100, 3000));
        assert!(!user_active(3000, 3000));
        assert!(!user_active(5000, 3000));
    }

    /// focus の無操作の確認 (実行器と `__focus-helper` の子が共有。D-138): min_idle_ms が 0 なら無操作を読まない
    #[test]
    fn focus_idle_check_skips_read_when_min_idle_is_zero() {
        let off = policy(&format!("{ENABLED}[capture]\nmin_idle_ms = 0\n"));
        let r = focus_idle_check(
            &off,
            &|| panic!("min_idle_ms = 0 なら無操作を読まない"),
            Surface::Cli,
        );
        assert!(r.is_ok(), "{r:?}");
    }

    /// 足りなければ 10 `user_active` (状況と次の手つき)、読み取りの失敗は `backend_error` の 8 (`os_error`)。足りれば通る
    #[test]
    fn focus_idle_check_reports_user_active_and_read_error() {
        let p = policy(&format!("{ENABLED}[capture]\nmin_idle_ms = 1000\n"));
        let e = focus_idle_check(&p, &|| Ok(10), Surface::Cli).unwrap_err();
        assert_eq!(e.kind, ErrorKind::UserActive);
        assert_eq!(
            e.detail.as_deref(),
            Some(&serde_json::json!({"idle_ms": 10, "min_idle_ms": 1000}))
        );
        assert_eq!(e.hint.as_deref(), Some(FOCUS_USER_ACTIVE_HINT));
        let e = focus_idle_check(
            &p,
            &|| Err(BackendError::Os("読めない".into())),
            Surface::Cli,
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::OsError, "{e:?}");
        assert!(focus_idle_check(&p, &|| Ok(1000), Surface::Cli).is_ok());
    }

    /// 人が操作中 (最後の入力から capture.min_idle_ms 未満) なら前面にせず 10 (D-049 の判定を focus にも掛ける)。
    /// 既に前面なら判定しない。min_idle_ms が 0 なら見ない
    #[test]
    fn focus_refuses_while_user_is_active_unless_already_foreground() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"a"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.idle_ms = 1000;
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::UserActive);
        assert_eq!(e.kind.exit_code(), 10);
        assert!(
            e.message.contains("1000 ms") && e.message.contains("3000 ms"),
            "{}",
            e.message
        );
        assert_eq!(e.hint.as_deref(), Some(FOCUS_USER_ACTIVE_HINT));
        // 状況 (D-065)
        assert_eq!(
            e.detail.as_deref(),
            Some(&serde_json::json!({"idle_ms": 1000, "min_idle_ms": 3000}))
        );
        assert!(world.borrow().focus_calls.is_empty(), "呼ばない");
        assert!(world.borrow().sent.is_empty());
        // 閾値ちょうどは通る。0 なら見ない
        world.borrow_mut().idle_ms = 3000;
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert!(r.is_ok(), "{r:?}");
        world.borrow_mut().foreground = Some(terminal());
        world.borrow_mut().idle_ms = 0;
        let off = policy(&format!("{ENABLED}[capture]\nmin_idle_ms = 0\n"));
        let (r, _) = exec(&world, &off, &mut StopSet::new(), &s);
        assert!(r.is_ok(), "{r:?}");
        // 既に前面なら、操作中でも何もせずに通る
        world.borrow_mut().idle_ms = 0;
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(
            world.borrow().focus_calls.len(),
            2,
            "最初の成功と min_idle_ms = 0 の 2 回だけ"
        );
    }

    /// 前面になるのを待つ間も停止要因を見る (マウス、目印、期限)。待ちの上限は FOCUS_SETTLE_MS
    #[test]
    fn focus_wait_is_bounded_and_honors_stop_sources() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let mut w = World::new();
        w.foreground = Some(chrome());
        w.top = vec![top(0x20, "notepad.exe", "a")];
        w.focus_no_effect = true;
        let world = w.shared();
        // 1 回目の poll (操作の前) は通し、待ちの中の 2 回目で止める
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(1)));
        let (r, _) = exec(&world, &policy(ENABLED), &mut stops, &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.action_index, Some(0));
        let w = world.borrow();
        assert_eq!(w.focus_calls, vec![0x20]);
        assert_eq!(w.now_ms, STOP_POLL_MS, "最初の 50 ms の待ちの直後に止まる");
    }

    #[test]
    fn focus_does_nothing_when_target_is_already_foreground() {
        let mut w = World::new();
        let np = top(0x20, "notepad.exe", "x");
        w.foreground = Some(np.info.clone());
        w.top = vec![np];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"focus","process":"notepad"}"#),
        );
        assert_eq!(r.unwrap().executed, 1);
        assert!(
            world.borrow().focus_calls.is_empty(),
            "既に前面なら呼ばない"
        );
    }

    /// 見つからない / 最小化 / Windows が断った、のどれも終了コード 11 で、何も送らず、繰り返さない
    #[test]
    fn focus_fails_with_11_and_sends_nothing_when_missing_minimized_or_refused() {
        let p = policy(ENABLED);
        let s = script(
            r#"{"type":"focus","process":"notepad.exe","title":"a.txt"},{"type":"key","keys":"a"}"#,
        );
        // 見つからない (題名が違う)
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "b.txt")];
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert_eq!(e.kind.exit_code(), 11);
        assert_eq!(e.action_index, Some(0));
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        assert!(world.borrow().focus_calls.is_empty());
        assert!(world.borrow().sent.is_empty());
        // 最小化: 復元せず、SetForegroundWindow も呼ばない
        let mut w = World::new();
        w.foreground = Some(terminal());
        let mut np = top(0x20, "notepad.exe", "a.txt");
        np.minimized = true;
        np.rect = None;
        w.top = vec![np];
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert!(e.message.contains("最小化"), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(FOCUS_MINIMIZED_HINT));
        assert!(world.borrow().focus_calls.is_empty());
        assert!(world.borrow().sent.is_empty());
        // Windows が断る (人が別のアプリを使っている): 1 回だけ呼び、前面は変わらない
        let mut w = World::new();
        w.foreground = Some(chrome());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.focus_accepts = false;
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert!(e.message.contains("chrome.exe"), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(FOCUS_REFUSED_HINT));
        let w = world.borrow();
        assert_eq!(w.focus_calls, vec![0x20], "繰り返さない");
        assert_eq!(w.foreground.as_ref().unwrap().process, "chrome.exe");
        assert!(w.sent.is_empty());
        drop(w);
        // 呼んだ直後に対象が閉じられた: 「閉じられた」と分かる文で 11
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.focus_closes = true;
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert!(e.message.contains("閉じられた"), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        assert!(world.borrow().sent.is_empty());
    }

    /// 対象のアプリがモーダルダイアログを出していると、前面になるのは対象に所有された別の hwnd。それが許可された
    /// ウィンドウなら成功 (続くキーはそこに届く)、許可されていなければ 7 で何も送らない。所有関係の無い同じ
    /// プロセスのウィンドウや、pid の違う同じ hwnd の値 (再利用) は成功にしない
    #[test]
    fn focus_accepts_allowed_dialog_of_target_process_as_foreground() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        let mut dialog = window("notepad.exe", "#32770");
        dialog.hwnd = 0x21;
        dialog.title = "名前を付けて保存".into();
        w.focus_redirect = Some(dialog);
        w.owners = vec![(0x21, 0x20)];
        let world = w.shared();
        let s = script(r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"esc"}"#);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert_eq!(report.executed, 2);
        assert_eq!(
            report.focused[0].hwnd, 0x21,
            "前面になったのはダイアログ: {:?}",
            report.focused
        );
        assert_eq!(world.borrow().foreground.as_ref().unwrap().hwnd, 0x21);
        assert_eq!(world.borrow().sent.len(), 1);
        // ダイアログのクラスが拒否されていれば 7 (何も送らない)
        world.borrow_mut().foreground = Some(terminal());
        let p = policy(&format!("{ENABLED}[deny]\nclasses=[\"#32770\"]\n"));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        // 呼んだ後に前面が変わっているので、頭に実行済みの一文 (partial) が付き、元の hint で終わる (D-065)
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], true);
        let h = e.hint.as_deref().unwrap();
        assert!(
            h.starts_with("actions[0] も途中まで行った (partial=true)。"),
            "{h}"
        );
        assert!(h.ends_with(FOCUS_DENIED_HINT), "{h}");
        assert_eq!(world.borrow().sent.len(), 1, "2 回目は何も送らない");
        // 拒否されたダイアログが既に前面でも、呼ぶ前の判定では 7 にせず、呼んでから 7 にする
        let dialog = world.borrow().focus_redirect.clone().unwrap();
        world.borrow_mut().foreground = Some(dialog);
        world.borrow_mut().focus_calls.clear();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(world.borrow().focus_calls, vec![0x20], "呼んでから判定する");
        // 前面は呼ぶ前と同じダイアログのまま (変わっていない) なので partial ではなく、hint も元のまま (D-065)
        assert_eq!(progress(&e)["partial"], false);
        assert_eq!(e.hint.as_deref(), Some(FOCUS_DENIED_HINT));
        // 許可されていれば、既に前面のダイアログで成功 (呼ばない)
        world.borrow_mut().focus_calls.clear();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap().focused[0].hwnd, 0x21);
        assert!(world.borrow().focus_calls.is_empty());
        // 別のプロセスが同じ hwnd の値を持っても (ハンドルの再利用)、pid が違えば成功にしない
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        let mut reused = chrome();
        reused.hwnd = 0x20;
        w.focus_redirect = Some(reused);
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::FocusFailed);
        assert!(world.borrow().sent.is_empty());
        // 同じプロセスでも、対象に所有されていないウィンドウが前面になっただけなら成功にしない
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.focus_redirect = Some(top(0x22, "notepad.exe", "b.txt").info);
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::FocusFailed);
        assert!(world.borrow().sent.is_empty());
    }

    /// 複数ウィンドウのアプリ: 同じプロセスの別のウィンドウが既に前面でも、title で指した対象を前面にしようとする。
    /// 断られれば 11 (前面のままの別ウィンドウを成功とみなさない)、受け付けられれば対象が前面になる
    #[test]
    fn focus_targets_titled_window_even_when_sibling_of_same_process_is_foreground() {
        let s = script(r#"{"type":"focus","process":"chrome.exe","title":"B"}"#);
        let mut w = World::new();
        let a = top(0x30, "chrome.exe", "A - Google Chrome");
        let b = top(0x31, "chrome.exe", "B - Google Chrome");
        w.foreground = Some(a.info.clone());
        w.top = vec![a, b];
        w.focus_accepts = false;
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed, "{e}");
        assert_eq!(e.hint.as_deref(), Some(FOCUS_REFUSED_HINT));
        assert_eq!(world.borrow().focus_calls, vec![0x31]);
        world.borrow_mut().focus_accepts = true;
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap().executed, 1);
        assert_eq!(world.borrow().foreground.as_ref().unwrap().hwnd, 0x31);
    }

    /// 受け付けたと返っても前面にならなければ、短く待ってから 11 (呼ぶのは 1 回)
    #[test]
    fn focus_waits_briefly_then_fails_when_os_accepts_but_window_stays_behind() {
        let mut w = World::new();
        w.foreground = Some(chrome());
        w.top = vec![top(0x20, "notepad.exe", "a")];
        w.focus_no_effect = true;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"focus","process":"notepad"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert!(e.message.contains("SetForegroundWindow=1"), "{}", e.message);
        let w = world.borrow();
        assert_eq!(w.focus_calls, vec![0x20]);
        assert!(w.now_ms >= FOCUS_SETTLE_MS, "{}", w.now_ms);
        assert!(
            w.now_ms <= FOCUS_SETTLE_MS + STOP_POLL_MS,
            "上限を超えて待たない: {}",
            w.now_ms
        );
    }

    /// 前面にしてよい相手かは、呼ぶ前に見る。名前の拒否は検証で 4。deny のクラスと自分の端末の窓は名指しの候補に
    /// 入らず、無いときと同じ 11 の `not_found` (D-139。以前は 7)。管理者権限は見つけた窓の判定で 7
    #[test]
    fn focus_rejects_denied_self_and_elevated_targets_before_calling() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a")];
        let world = w.shared();
        // deny.processes は検証で止まる (4)
        let p = policy(&format!("{ENABLED}[deny]\nprocesses=[\"notepad\"]\n"));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::PolicyDenied);
        // deny.classes の窓は探す候補に入らない (11 の not_found)
        let p = policy(&format!("{ENABLED}[deny]\nclasses=[\"TargetClass\"]\n"));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed, "{e}");
        assert_eq!(e.reason, Some(reason::NOT_FOUND));
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        // 自分の端末 (D-045) も候補に入らない
        let me = world.borrow().top[0].info.clone();
        let (r, _) = exec_excluding(&world, &policy(ENABLED), &mut StopSet::new(), &s, Some(&me));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed, "{e}");
        assert_eq!(e.reason, Some(reason::NOT_FOUND));
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        // 管理者権限
        world.borrow_mut().top[0].info.elevated = Some(true);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(e.hint.as_deref(), Some(ELEVATED_HINT));
        assert!(
            world.borrow().focus_calls.is_empty(),
            "どの拒否でも SetForegroundWindow は呼ばない"
        );
    }

    /// close (D-062): 端末が前面のままでも、題名で選んだ許可ウィンドウに WM_CLOSE を置く。前面は変えず、
    /// 入力も送らない。結果の closed に、閉じようとしたウィンドウと消えたかが入る (無ければ出力に出ない)
    #[test]
    fn close_posts_to_titled_window_without_focus_or_input_while_terminal_is_front() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![
            top(0x20, "Notepad.exe", "a.txt - メモ帳"),
            top(0x21, "Notepad.exe", "b.txt - メモ帳"),
        ];
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"close","process":"notepad.exe","title":"B.TXT"}"#),
        );
        let report = r.unwrap();
        assert_eq!(
            report.closed,
            vec![CloseInfo {
                action_index: 0,
                hwnd: 0x21,
                process: "Notepad.exe".into(),
                title: "b.txt - メモ帳".into(),
                gone: true,
                hidden: false,
            }]
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["closed"][0]["gone"], true);
        // hidden は false でも省かずに出す (D-096)
        assert_eq!(
            json["closed"][0].get("hidden"),
            Some(&serde_json::json!(false))
        );
        let w = world.borrow();
        assert_eq!(
            w.close_calls,
            vec![0x21],
            "題名で選ぶ (前面から見て最初の 0x20 ではない)"
        );
        assert!(w.focus_calls.is_empty(), "前面にしない");
        assert!(w.sent.is_empty(), "入力を送らない: {:?}", w.sent);
        assert_eq!(
            w.foreground.as_ref().unwrap().hwnd,
            0x10,
            "前面は端末のまま"
        );
        drop(w);
        // close の無い操作列の出力には closed を出さない (今までの出力を変えない)
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"wait","ms":1}"#),
        );
        assert!(
            serde_json::to_value(r.unwrap())
                .unwrap()
                .get("closed")
                .is_none()
        );
    }

    /// close: 見つからない・許可されていない (deny のクラス)・自分の端末・起動した側なら 12 (後の 3 つは名指しの
    /// 候補に入らない。D-139。以前は 7)、無効と管理者権限なら 7。どれも WM_CLOSE を置かない
    #[test]
    fn close_refuses_missing_denied_self_host_and_elevated_targets_without_posting() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"close","process":"notepad.exe","title":"b.txt"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound);
        assert_eq!(e.kind.exit_code(), 12);
        assert_eq!(e.hint.as_deref(), Some(CLOSE_NOT_FOUND_HINT));
        let p = policy(&format!("{ENABLED}[deny]\nclasses=[\"TargetClass\"]\n"));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CLOSE_NOT_FOUND_HINT));
        let me = world.borrow().top[0].info.clone();
        let (r, _) = exec_excluding(&world, &policy(ENABLED), &mut StopSet::new(), &s, Some(&me));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CLOSE_NOT_FOUND_HINT));
        let (r, _) = exec_with(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &s,
            None,
            &[me.pid],
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CLOSE_NOT_FOUND_HINT));
        // ダイアログを開いていて無効なウィンドウは、閉じるボタンも押せないので閉じない
        world.borrow_mut().disabled = vec![0x20];
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(e.reason, Some(reason::TARGET_DISABLED));
        assert_eq!(e.hint.as_deref(), Some(CLOSE_DISABLED_HINT));
        world.borrow_mut().disabled.clear();
        world.borrow_mut().top[0].info.elevated = Some(true);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().hint.as_deref(), Some(ELEVATED_HINT));
        assert!(
            world.borrow().close_calls.is_empty(),
            "どの拒否でも置かない"
        );
    }

    /// close: 対象 (または対象に所有されたダイアログ) が前面で人が操作中なら 10。前面でなければ操作中でも閉じる
    #[test]
    fn close_refuses_front_target_while_user_is_active_but_closes_background_one() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let mut w = World::new();
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.foreground = Some(w.top[0].info.clone());
        w.idle_ms = 1000;
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::UserActive);
        assert_eq!(e.hint.as_deref(), Some(CLOSE_USER_ACTIVE_HINT));
        // 状況 (D-065)
        assert_eq!(
            e.detail.as_deref(),
            Some(&serde_json::json!({"idle_ms": 1000, "min_idle_ms": 3000}))
        );
        // 対象に所有されたダイアログが前面でも同じ
        let mut dialog = world.borrow().top[0].info.clone();
        dialog.hwnd = 0x30;
        world.borrow_mut().owners = vec![(0x30, 0x20)];
        world.borrow_mut().foreground = Some(dialog);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::UserActive);
        assert!(world.borrow().close_calls.is_empty());
        // 前面が端末なら、人が打った直後でも閉じる (この操作の目的)
        world.borrow_mut().foreground = Some(terminal());
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let closed = &r.unwrap().closed[0];
        assert!(closed.gone);
        assert!(!closed.hidden, "破棄なら hidden: false");
    }

    /// close: 置けても消えなければ (保存の確認など) 上限まで待って成功のまま gone: false。置けなければ 12
    #[test]
    fn close_reports_not_gone_after_bounded_wait_and_fails_when_post_fails() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.close_kept = true;
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert!(!report.closed[0].gone);
        assert!(!report.closed[0].hidden, "残れば hidden: false");
        assert_eq!(
            serde_json::to_value(&report).unwrap()["closed"][0].get("hidden"),
            Some(&serde_json::json!(false))
        );
        let waited = world.borrow().now_ms;
        assert!(
            (CLOSE_SETTLE_MS..CLOSE_SETTLE_MS + 200).contains(&waited),
            "{waited}"
        );
        world.borrow_mut().close_fails = true;
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound);
        assert_eq!(e.hint.as_deref(), Some(CLOSE_NOT_FOUND_HINT));
    }

    /// close: 消えるのを待つ間も停止要因を見る。残る相手でも、隠れただけの相手 (D-096) でも同じ
    #[test]
    fn close_wait_honors_stop_sources() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        for hides in [false, true] {
            let mut w = World::new();
            w.foreground = Some(terminal());
            w.top = vec![top(0x20, "notepad.exe", "a.txt")];
            w.close_kept = !hides;
            w.close_hides = hides;
            let world = w.shared();
            let mut stops = StopSet::new();
            stops.push(Box::new(StopAfterPolls(3)));
            let (r, _) = exec(&world, &policy(ENABLED), &mut stops, &s);
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::Stopped, "hides={hides}");
            assert_eq!(
                world.borrow().close_calls,
                vec![0x20],
                "置いた後の待ちで止まった (hides={hides})"
            );
            // 閉じる依頼は届いているので partial (D-065)
            assert_eq!(progress(&e)["partial"], true);
        }
    }

    /// close (D-096): 窓が壊れず同じ pid のまま隠れただけ (トレイに入るアプリ) なら、上限まで待ってから
    /// gone: true, hidden: true
    #[test]
    fn close_reports_hidden_when_window_is_only_hidden() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.close_hides = true;
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert!(report.closed[0].gone);
        assert!(report.closed[0].hidden);
        assert_eq!(
            serde_json::to_value(&report).unwrap()["closed"][0]["hidden"],
            true
        );
        let waited = world.borrow().now_ms;
        assert!(
            (CLOSE_SETTLE_MS..CLOSE_SETTLE_MS + 200).contains(&waited),
            "隠れた瞬間では決めず上限まで待つ: {waited}"
        );
    }

    /// close (D-096): 隠れた後に壊れる相手 (壊す途中で一瞬隠れる窓) は、壊れた時に抜けて hidden: false
    #[test]
    fn close_hidden_then_destroyed_reports_not_hidden() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.close_hides = true;
        w.hidden_then_destroyed_after_ms = Some(200);
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert!(report.closed[0].gone);
        assert!(!report.closed[0].hidden);
        let waited = world.borrow().now_ms;
        assert!(
            (200..CLOSE_SETTLE_MS).contains(&waited),
            "壊れたら上限を待たずに抜ける: {waited}"
        );
    }

    /// close (D-096): 隠れた後にまた見えた相手は、最後の状態 (見える) で gone: false, hidden: false
    #[test]
    fn close_hidden_then_visible_reports_not_gone() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.close_hides = true;
        w.hidden_then_visible_after_ms = Some(200);
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert!(!report.closed[0].gone);
        assert!(!report.closed[0].hidden);
    }

    fn exec_on(
        world: &Rc<RefCell<World>>,
        stops: &mut StopSet,
        s: &Script,
        host_pids: &[u32],
        surface: Surface,
    ) -> Result<RunReport, AppError> {
        let p = policy(ENABLED);
        let mut sink = MockSink(world.clone());
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        Executor {
            policy: &p,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops,
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute_on(s, &opts(None, host_pids, surface))
    }

    /// focus が断られた world (対象はメモ帳、前面は `fg`)
    fn refused_world(fg: Option<WindowInfo>) -> Rc<RefCell<World>> {
        let mut w = World::new();
        w.foreground = fg;
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.focus_accepts = false;
        w.shared()
    }

    /// 前面が ai-cu を起動した側 (ホスト) なのに断られた (D-056): 人が別のアプリを使っているとは言わず、
    /// reason と detail で分かるようにする。呼ぶのは 1 回、何も送らない。次の手の文は呼ばれ方で選ぶ
    #[test]
    fn focus_refused_with_host_in_front_reports_refused_host_front() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"a"}"#);
        for surface in [Surface::Cli, Surface::Mcp] {
            let world = refused_world(Some(terminal()));
            let e = exec_on(&world, &mut StopSet::new(), &s, &[1], surface).unwrap_err();
            assert_eq!(e.kind, ErrorKind::FocusFailed);
            assert_eq!(e.kind.exit_code(), 11);
            assert_eq!(e.action_index, Some(0));
            assert_eq!(e.reason, Some(reason::REFUSED_HOST_FRONT));
            let d = e.detail.clone().expect("detail");
            assert_eq!(d["set_foreground"], 0);
            assert_eq!(d["foreground_process"], "Zed.exe");
            assert_eq!(d["foreground_is_host"], true);
            assert!(d.get("title").is_none(), "題名は入れない: {d}");
            assert!(!e.message.contains("人が別のアプリ"), "{}", e.message);
            assert!(e.message.contains("起動した側"), "{}", e.message);
            assert!(e.message.contains("SetForegroundWindow=0"), "{}", e.message);
            assert_eq!(
                e.hint.as_deref(),
                Some(hint(Hint::FocusHostFront, surface)),
                "{surface:?}"
            );
            let w = world.borrow();
            assert_eq!(w.focus_calls, vec![0x20], "繰り返さない");
            assert!(w.sent.is_empty());
        }
    }

    /// 前面がホスト以外のアプリなら、今までどおり「人が別のアプリを使っている」と FOCUS_REFUSED_HINT (CLI)
    #[test]
    fn focus_refused_with_other_app_keeps_focus_refused_hint() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let world = refused_world(Some(chrome()));
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Cli).unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert_eq!(e.reason, Some(reason::REFUSED_OTHER_APP));
        assert_eq!(e.hint.as_deref(), Some(FOCUS_REFUSED_HINT));
        assert!(
            e.message.contains("chrome.exe") && e.message.contains("人が別のアプリを使っている"),
            "{}",
            e.message
        );
        let d = e.detail.clone().expect("detail");
        assert_eq!(d["set_foreground"], 0);
        assert_eq!(d["foreground_process"], "chrome.exe");
        assert_eq!(d["foreground_is_host"], false);
        // MCP では道具の名前で書いた文
        let world = refused_world(Some(chrome()));
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.reason, Some(reason::REFUSED_OTHER_APP));
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::FocusRefused, Surface::Mcp))
        );
    }

    /// 前面が無い (または pid が読めない) なら refused_unknown。人のせいと断定しない
    #[test]
    fn focus_refused_without_foreground_reports_refused_unknown() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let mut unreadable = chrome();
        unreadable.pid = 0;
        unreadable.process = String::new();
        for fg in [None, Some(unreadable)] {
            let world = refused_world(fg.clone());
            let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Cli).unwrap_err();
            assert_eq!(e.kind, ErrorKind::FocusFailed, "{fg:?}");
            assert_eq!(e.reason, Some(reason::REFUSED_UNKNOWN), "{fg:?}");
            assert_eq!(
                e.hint.as_deref(),
                Some(hint(Hint::FocusRefusedUnknown, Surface::Cli))
            );
            assert!(!e.message.contains("人が別のアプリ"), "{}", e.message);
            let d = e.detail.clone().expect("detail");
            assert_eq!(d["set_foreground"], 0);
            assert!(d["foreground_process"].is_null(), "{d}");
            assert_eq!(d["foreground_is_host"], false);
            assert_eq!(world.borrow().focus_calls, vec![0x20]);
        }
    }

    /// 断られた後に前面を読めなくても 8 (OS のエラー) にせず、11 の `refused_unknown` にする (D-056。既に失敗の経路)
    #[test]
    fn focus_refused_with_unreadable_foreground_reports_refused_unknown() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let world = refused_world(Some(terminal()));
        // 断られた直後の確かめ (reached) は読め、その後の理由の分類で読めない (間に前面が消えた、など)
        world.borrow_mut().foreground_reads_after_focus = Some(1);
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert_eq!(e.reason, Some(reason::REFUSED_UNKNOWN));
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::FocusRefusedUnknown, Surface::Mcp))
        );
        let d = e.detail.clone().expect("detail");
        assert!(d["foreground_process"].is_null(), "{d}");
        assert_eq!(d["foreground_is_host"], false);
        assert_eq!(world.borrow().focus_calls, vec![0x20]);
    }

    /// 前面がホストで断られた MCP の focus は、補助プロセスに 1 回だけ頼み (D-057)、前面になれば成功。続くキーも届く。
    /// 自分の SetForegroundWindow も 1 回だけ
    #[test]
    fn focus_helper_brings_target_front_when_refused_with_host_in_front_on_mcp() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"a"}"#);
        let world = refused_world(Some(terminal()));
        world.borrow_mut().helper = HelperOutcome::Accepted;
        let report = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap();
        assert_eq!(report.executed, 2);
        assert_eq!(report.focused.len(), 1);
        assert_eq!(report.focused[0].hwnd, 0x20);
        assert!(report.focused[0].helper, "補助で前面にしたことを結果に出す");
        let v = serde_json::to_value(&report.focused[0]).unwrap();
        assert_eq!(v["helper"], true, "出力の JSON にも出す: {v}");
        let w = world.borrow();
        assert_eq!(w.focus_calls, vec![0x20], "自分では 1 回だけ");
        assert_eq!(w.helper_calls, vec![0x20], "補助も 1 回だけ");
        assert_eq!(w.foreground.as_ref().unwrap().hwnd, 0x20);
        assert_eq!(w.sent.len(), 1, "focus 自体は何も送らない: {:?}", w.sent);
    }

    /// 補助も断られた (0)、時間切れ、起動に失敗: 今の分類のまま 11 の refused_host_front。detail に補助の結果を足す。
    /// 補助を繰り返さず、何も送らない
    #[test]
    fn focus_helper_failure_keeps_refused_host_front_with_helper_detail() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"a"}"#);
        for (outcome, label) in [
            (HelperOutcome::Refused, "refused"),
            (HelperOutcome::TimedOut, "timeout"),
            (HelperOutcome::Failed("起動できない".into()), "failed"),
        ] {
            let world = refused_world(Some(terminal()));
            world.borrow_mut().helper = outcome;
            let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
            assert_eq!(e.kind, ErrorKind::FocusFailed, "{label}");
            assert_eq!(e.action_index, Some(0));
            assert_eq!(e.reason, Some(reason::REFUSED_HOST_FRONT), "{label}");
            assert_eq!(
                e.hint.as_deref(),
                Some(hint(Hint::FocusHostFront, Surface::Mcp))
            );
            let d = e.detail.clone().expect("detail");
            assert_eq!(d["set_foreground"], 0);
            assert_eq!(d["foreground_is_host"], true);
            assert_eq!(d["helper"], true, "{d}");
            assert_eq!(d["helper_result"], label, "{d}");
            // 補助が起動できないなどの理由は捨てない (failed のときだけ)
            if label == "failed" {
                assert_eq!(d["helper_error"], "起動できない", "{d}");
                assert!(e.message.contains("failed: 起動できない"), "{}", e.message);
            } else {
                assert!(d["helper_error"].is_null(), "{d}");
            }
            let w = world.borrow();
            assert_eq!(w.focus_calls, vec![0x20]);
            assert_eq!(w.helper_calls, vec![0x20], "{label}");
            assert!(w.sent.is_empty());
        }
    }

    /// 補助の間に対象が閉じられたら、断られた扱いにせず closed にする (人に「前面に出して」と頼ませない)
    #[test]
    fn focus_target_closed_during_helper_reports_closed() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        for outcome in [
            HelperOutcome::Accepted,
            HelperOutcome::Failed("exit 2".into()),
        ] {
            let world = refused_world(Some(terminal()));
            {
                let mut w = world.borrow_mut();
                w.helper = outcome.clone();
                w.helper_closes = true;
            }
            let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
            assert_eq!(e.kind, ErrorKind::FocusFailed, "{outcome:?}");
            assert_eq!(e.reason, Some(reason::CLOSED), "{outcome:?}");
            assert_eq!(
                e.hint.as_deref(),
                Some(hint(Hint::FocusNotFound, Surface::Mcp))
            );
            let d = e.detail.clone().expect("detail");
            assert_eq!(d["helper"], true, "{d}");
            assert_eq!(d["helper_result"], outcome.label(), "{d}");
            assert_eq!(world.borrow().helper_calls, vec![0x20]);
        }
    }

    /// 補助が 1 を返しても前面にならなければ (点滅だけ)、短く待ってから 11。補助を使ったことは detail で分かる
    #[test]
    fn focus_helper_accepted_but_not_reached_fails_after_bounded_wait() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let world = refused_world(Some(terminal()));
        {
            let mut w = world.borrow_mut();
            w.helper = HelperOutcome::Accepted;
            w.helper_no_effect = true;
        }
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.reason, Some(reason::REFUSED_HOST_FRONT));
        let d = e.detail.clone().expect("detail");
        assert_eq!(d["helper"], true);
        assert_eq!(d["helper_result"], "accepted");
        let w = world.borrow();
        assert_eq!(w.helper_calls, vec![0x20]);
        assert!(w.now_ms >= FOCUS_SETTLE_MS, "{}", w.now_ms);
        assert!(
            w.now_ms <= FOCUS_SETTLE_MS + STOP_POLL_MS,
            "上限を超えて待たない: {}",
            w.now_ms
        );
    }

    /// 補助を使うのは、MCP で、自分の呼び出しが断られ (0)、前面がホストのときだけ。前面が別のアプリ、前面が
    /// 読めない、CLI (TUI も同じ)、自分の呼び出しが 1 を返したときは起動しない。使わなかったことも detail で分かる
    #[test]
    fn focus_helper_not_spawned_unless_mcp_refused_with_host_in_front() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        // 前面が別のアプリ (人が使っている): 補助は前面ロックの抜け道にしない
        let world = refused_world(Some(chrome()));
        world.borrow_mut().helper = HelperOutcome::Accepted;
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.reason, Some(reason::REFUSED_OTHER_APP));
        let d = e.detail.clone().expect("detail");
        assert_eq!(d["helper"], false, "{d}");
        assert!(d["helper_result"].is_null(), "{d}");
        assert!(world.borrow().helper_calls.is_empty());
        // 前面が無い
        let world = refused_world(None);
        world.borrow_mut().helper = HelperOutcome::Accepted;
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.reason, Some(reason::REFUSED_UNKNOWN));
        assert!(world.borrow().helper_calls.is_empty());
        // CLI は都度起動なので、直接の呼び出しで足りる (補助を使わない)
        let world = refused_world(Some(terminal()));
        world.borrow_mut().helper = HelperOutcome::Accepted;
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Cli).unwrap_err();
        assert_eq!(e.reason, Some(reason::REFUSED_HOST_FRONT));
        assert_eq!(e.detail.clone().expect("detail")["helper"], false);
        assert!(world.borrow().helper_calls.is_empty());
        // 自分の呼び出しが 1 を返した (前面にならなかった) ときも使わない
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.focus_no_effect = true;
        w.helper = HelperOutcome::Accepted;
        let world = w.shared();
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert!(world.borrow().helper_calls.is_empty());
    }

    /// 補助を起動する前にも停止要因 (取り消し、マウス、目印、期限) を見る。止まっていれば起動しない
    #[test]
    fn focus_helper_not_spawned_after_stop() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let world = refused_world(Some(terminal()));
        world.borrow_mut().helper = HelperOutcome::Accepted;
        // 1 回目の poll (操作の前) は通し、補助の前の 2 回目で止める
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(1)));
        let e = exec_on(&world, &mut stops, &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.action_index, Some(0));
        assert!(world.borrow().helper_calls.is_empty());
    }

    /// 隠れたウィンドウの撮影 (7) とクリックの点の下のガード (7) も、MCP では道具の名前の文になる。判定は変わらない
    #[test]
    fn capture_and_mouse_guard_hints_follow_surface() {
        let np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let mut w = World::new();
        w.default_window = Some(np.info.clone());
        w.top = vec![np];
        w.windows_at = vec![(Rect::new(100, 100, 300, 200), chrome())];
        w.foreground = Some(chrome());
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let shot =
            script(r#"{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe"}}"#);
        let click = script(r#"{"type":"click","x":150,"y":150}"#);
        for surface in [Surface::Cli, Surface::Mcp] {
            for (s, h) in [(&shot, Hint::CaptureOccluded), (&click, Hint::MouseGuard)] {
                let mut sink = MockSink(world.clone());
                let desktop = MockDesktop(world.clone());
                let mut clock = MockClock(world.clone());
                let mut images = MemoryImages::default();
                let e = Executor {
                    policy: &p,
                    sink: &mut sink,
                    desktop: &desktop,
                    clock: &mut clock,
                    stops: &mut StopSet::new(),
                    images: &mut images,
                    indicator: &mut MockIndicator(world.clone()),
                    input_lock: &mut MockInputLock(world.clone()),
                }
                .execute_on(s, &opts(None, &[1], surface))
                .unwrap_err();
                assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{h:?} {surface:?}");
                assert_eq!(
                    e.hint.as_deref(),
                    Some(hint(h, surface)),
                    "{h:?} {surface:?}"
                );
                assert!(images.written.is_empty());
            }
        }
        assert!(world.borrow().sent.is_empty());
    }

    /// 見つからない、最小化、閉じられた、にも reason が付く。終了コードと次の手 (CLI) は変わらない
    #[test]
    fn focus_existing_paths_carry_reason() {
        let s = script(r#"{"type":"focus","process":"notepad.exe","title":"a.txt"}"#);
        // 見つからない
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "b.txt")];
        let e = exec_on(&w.shared(), &mut StopSet::new(), &s, &[1], Surface::Cli).unwrap_err();
        assert_eq!(
            (e.kind.exit_code(), e.reason),
            (11, Some(reason::NOT_FOUND))
        );
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        // 最小化
        let mut w = World::new();
        w.foreground = Some(terminal());
        let mut np = top(0x20, "notepad.exe", "a.txt");
        np.minimized = true;
        np.rect = None;
        w.top = vec![np];
        let e = exec_on(&w.shared(), &mut StopSet::new(), &s, &[1], Surface::Cli).unwrap_err();
        assert_eq!(
            (e.kind.exit_code(), e.reason),
            (11, Some(reason::MINIMIZED))
        );
        assert_eq!(e.hint.as_deref(), Some(FOCUS_MINIMIZED_HINT));
        // 閉じられた
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.focus_closes = true;
        let world = w.shared();
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Cli).unwrap_err();
        assert_eq!((e.kind.exit_code(), e.reason), (11, Some(reason::CLOSED)));
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        assert!(world.borrow().sent.is_empty());
        // MCP では、見つからないときの次の手に CLI の書き方を出さない
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "b.txt")];
        let e = exec_on(&w.shared(), &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.reason, Some(reason::NOT_FOUND));
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::FocusNotFound, Surface::Mcp))
        );
    }

    /// 受け付けられても前面にならず、前面はホストのまま: 待ちの上限と停止要因はそのままで、refused_host_front
    #[test]
    fn focus_accepted_but_not_reached_with_host_front_waits_then_host_hint() {
        let s = script(r#"{"type":"focus","process":"notepad.exe"}"#);
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a")];
        w.focus_no_effect = true;
        let world = w.shared();
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed);
        assert_eq!(e.reason, Some(reason::REFUSED_HOST_FRONT));
        assert!(e.message.contains("SetForegroundWindow=1"), "{}", e.message);
        assert_eq!(e.detail.as_ref().unwrap()["set_foreground"], 1);
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::FocusHostFront, Surface::Mcp))
        );
        {
            let w = world.borrow();
            assert_eq!(w.focus_calls, vec![0x20]);
            assert!(w.now_ms >= FOCUS_SETTLE_MS, "{}", w.now_ms);
            assert!(w.now_ms <= FOCUS_SETTLE_MS + STOP_POLL_MS, "{}", w.now_ms);
        }
        // 待ちの間の停止要因も今までどおり
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(1)));
        let e = exec_on(&world, &mut stops, &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
    }

    /// フォアグラウンドガードの次の手も呼ばれ方で選ぶ。判定と送るものは変わらない
    #[test]
    fn guard_hints_follow_surface_without_changing_decisions() {
        let mut w = World::new();
        w.foreground = Some(chrome());
        let world = w.shared();
        let s = script(r#"{"type":"key","keys":"a"}"#);
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        for (surface, expected) in [
            (Surface::Cli, KEYBOARD_GUARD_HINT),
            (Surface::Mcp, hint(Hint::KeyboardGuard, Surface::Mcp)),
        ] {
            let mut sink = MockSink(world.clone());
            let desktop = MockDesktop(world.clone());
            let mut clock = MockClock(world.clone());
            let mut images = MemoryImages::default();
            let e = Executor {
                policy: &p,
                sink: &mut sink,
                desktop: &desktop,
                clock: &mut clock,
                stops: &mut StopSet::new(),
                images: &mut images,
                indicator: &mut MockIndicator(world.clone()),
                input_lock: &mut MockInputLock(world.clone()),
            }
            .execute_on(&s, &opts(None, &[], surface))
            .unwrap_err();
            assert_eq!(e.kind, ErrorKind::ForegroundGuard);
            assert_eq!(e.hint.as_deref(), Some(expected), "{surface:?}");
            assert_eq!(e.reason, None);
        }
        assert!(world.borrow().sent.is_empty());
        // ホストのウィンドウ
        world.borrow_mut().foreground = Some(terminal());
        let e = exec_on(&world, &mut StopSet::new(), &s, &[1], Surface::Mcp).unwrap_err();
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::HostWindow, Surface::Mcp))
        );
        assert!(world.borrow().sent.is_empty());
    }

    #[test]
    fn forbid_real_input_backend_refusal_maps_to_exit_9() {
        let mut w = World::new();
        w.refuse = Some(BackendError::Forbidden("dev".into()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":1,"y":1}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForbiddenDevEnv);
        assert_eq!(e.kind.exit_code(), 9);
    }

    #[test]
    fn run_rejects_points_outside_screen_before_sending() {
        let world = World::new().shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":-5,"y":1}"#),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidScript);
        assert!(world.borrow().sent.is_empty());
    }

    /// screenshot の window (D-053): 撮る直前に find_window で矩形を解き、結果に window を出す。
    /// 見つからない / 最小化 / 画面と交わらないは 12 で何も撮らない。許可されないクラスは 4。はみ出す分は切り詰める
    #[test]
    fn screenshot_window_resolves_rect_at_capture_time() {
        let np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let mut w = World::new();
        // 矩形の中はどこも対象自身 (隠れていない)
        w.default_window = Some(np.info.clone());
        w.top = vec![np];
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let s =
            script(r#"{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe"}}"#);
        let (r, images) = exec(&world, &p, &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert_eq!(report.executed, 1);
        let shot = &report.screenshots[0];
        assert_eq!(
            (shot.x, shot.y, shot.width, shot.height),
            (100, 100, 800, 600)
        );
        assert_eq!(
            shot.window
                .as_ref()
                .map(|w| (w.hwnd, w.process.as_str(), w.title.as_str())),
            Some((0x20, "Notepad.exe", "a.txt - メモ帳"))
        );
        assert_eq!(images.written.len(), 1);
        // 許可されていないプロセスは検証で 4 (allow.processes)。許可されたプロセスで一致する窓が無ければ 12
        let other =
            script(r#"{"type":"screenshot","path":"a.png","window":{"process":"chrome.exe"}}"#);
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &other);
        assert_eq!(r.unwrap_err().kind, ErrorKind::PolicyDenied);
        let missing = script(
            r#"{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe","title":"zzz"}}"#,
        );
        let (r, images) = exec(&world, &p, &mut StopSet::new(), &missing);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound);
        assert_eq!(e.action_index, Some(0));
        assert!(e.message.contains("zzz"), "{}", e.message);
        assert!(images.written.is_empty(), "何も撮らない");
        {
            let mut w = world.borrow_mut();
            w.top[0].minimized = true;
            w.top[0].rect = None;
        }
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound);
        assert!(e.message.contains("最小化"), "{}", e.message);
        {
            let mut w = world.borrow_mut();
            w.top[0].minimized = false;
            w.top[0].rect = Some(Rect::new(3800, 2100, 800, 600));
        }
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let report = r.unwrap();
        let shot = &report.screenshots[0];
        assert_eq!(
            (shot.x, shot.y, shot.width, shot.height),
            (3800, 2100, 40, 60),
            "画面からはみ出す分は切り詰める"
        );
        world.borrow_mut().top[0].rect = Some(Rect::new(-900, 0, 800, 600));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        assert_eq!(
            r.unwrap_err().kind,
            ErrorKind::WindowNotFound,
            "画面と交わらない"
        );
        world.borrow_mut().top[0].rect = Some(Rect::new(100, 100, 800, 600));
        let denied_class = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nclasses=[\"TargetClass\"]\n"
        ));
        // クラスで断られる窓は名指しの候補に入らない (D-139。無いときと同じ 12)
        let (r, images) = exec(&world, &denied_class, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
        assert!(images.written.is_empty());
    }

    /// 対象が他のウィンドウに隠れていれば撮らない (D-053 の追記): 矩形の撮影には上に重なるウィンドウの中身が
    /// 写り込む (deny の端末など)。7 で「先に focus」。対象に所有されたダイアログが重なっているのは対象の一部なので撮る
    #[test]
    fn screenshot_window_refuses_when_covered_by_another_window() {
        let np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let mut w = World::new();
        w.default_window = Some(np.info.clone());
        w.top = vec![np];
        // 左上の一角に Chrome が重なっている
        w.windows_at = vec![(Rect::new(100, 100, 300, 200), chrome())];
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let s =
            script(r#"{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe"}}"#);
        let (r, images) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert!(e.message.contains("chrome.exe"), "{}", e.message);
        assert!(e.message.contains("隠れている"), "{}", e.message);
        assert!(e.hint().contains("focus"), "{}", e.hint());
        assert!(images.written.is_empty(), "何も撮らない");
        let mut dialog = window("Notepad.exe", "#32770");
        dialog.hwnd = 0x21;
        {
            let mut w = world.borrow_mut();
            w.windows_at = vec![(Rect::new(300, 300, 200, 100), dialog)];
            w.owners = vec![(0x21, 0x20)];
        }
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        assert!(r.is_ok(), "対象に所有されたダイアログは対象の一部: {r:?}");
    }

    // --- 操作列の撮影の隠れの猶予 (D-082) ---

    const NOTEPAD_RECT: Rect = Rect::new(100, 100, 800, 600);

    /// メモ帳 (0x20) の矩形全体に Chrome が重なっている世界。重なりの時刻の範囲は呼ぶ側が決める
    fn covered_notepad_world() -> World {
        let np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let mut w = World::new();
        w.default_window = Some(np.info.clone());
        w.top = vec![np];
        w.windows_at = vec![(NOTEPAD_RECT, chrome())];
        w
    }

    fn notepad_policy() -> Policy {
        policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"))
    }

    const NOTEPAD_SHOT: &str =
        r#"{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe"}}"#;

    /// 隠れが猶予のうちに消えれば撮る: 50 ms ごとに解き直し、t = 150 で前面に描かれて 1 回だけ撮る
    #[test]
    fn screenshot_window_waits_out_transient_cover() {
        let mut w = covered_notepad_world();
        w.windows_at_until_ms = Some(120);
        let world = w.shared();
        let (r, images) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &script(NOTEPAD_SHOT),
        );
        let report = r.unwrap();
        assert_eq!(report.screenshots.len(), 1);
        assert_eq!(images.written.len(), 1);
        let w = world.borrow();
        assert_eq!(w.capture_calls, 1);
        assert!((120..OCCLUDED_GRACE_MS).contains(&w.now_ms), "{}", w.now_ms);
        // 隠れが解けて撮れたときは、deny の無い設定で重なりの一覧を読まない (D-077、D-135)
        assert_eq!(w.stack_calls, 0);
    }

    /// 猶予の間に窓が消えれば (閉じかけの窓に次の窓が重なっていた)、隠れの 7 ではなく 12。撮らない
    #[test]
    fn screenshot_window_closing_under_cover_is_12() {
        let mut w = covered_notepad_world();
        // 3 回目の find_window から一覧に無い
        w.top_after_finds = Some((2, Vec::new()));
        let world = w.shared();
        let (r, images) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &script(NOTEPAD_SHOT),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(images.written.is_empty());
        let w = world.borrow();
        assert_eq!(w.capture_calls, 0);
        assert_eq!(w.find_window_calls, 3);
        assert_eq!(w.now_ms, 2 * STOP_POLL_MS);
    }

    /// 猶予を使い切っても隠れたままなら 7。message に待った時間を添え、hint は今までの隠れの文。撮らない
    #[test]
    fn screenshot_window_cover_outlasting_grace_is_7() {
        let world = covered_notepad_world().shared();
        let (r, images) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &script(NOTEPAD_SHOT),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(e.message.contains("隠れている"), "{}", e.message);
        assert!(e.message.contains("ms 待っても隠れたまま"), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(CAPTURE_OCCLUDED_HINT));
        // 隠れを見つけた点 (メモ帳の矩形の内側 8 の左上。D-135)
        assert_eq!(
            detail(&e)["point"],
            serde_json::json!({ "x": 108, "y": 108 })
        );
        assert_eq!(detail(&e)["window"]["process"], "chrome.exe");
        assert!(images.written.is_empty());
        let w = world.borrow();
        assert_eq!(w.capture_calls, 0);
        assert!(
            (OCCLUDED_GRACE_MS..=OCCLUDED_GRACE_MS + STOP_POLL_MS).contains(&w.now_ms),
            "{}",
            w.now_ms
        );
        // 重なりの一覧は猶予を使い切った 1 回だけ読む (周回ごとには読まない。D-135)
        assert!(w.find_window_calls > 1, "{}", w.find_window_calls);
        assert_eq!(w.stack_calls, 1);
    }

    /// 隠れ以外の 12 / 7 は待たずに返す (解き直さない): 見つからない、許可の外れ (deny のクラス。D-139 で 12)、
    /// ai-cu を起動した側の窓 (D-139 で 12)、対象の枠の内側 8 ピクセルの帯にある小さい deny の窓 (7)。
    /// 最初の 3 つは対象を覆いで隠しておき、覆いがあってもこれらが隠れより先に返ることも確かめる
    #[test]
    fn screenshot_window_other_errors_do_not_wait() {
        let np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let base = || {
            let mut w = World::new();
            w.default_window = Some(np.info.clone());
            w.top = vec![np.clone()];
            w.windows_at = vec![(NOTEPAD_RECT, chrome())];
            w
        };
        let s = script(NOTEPAD_SHOT);
        let check = |world: &Rc<RefCell<World>>, r: Result<RunReport, AppError>, kind, why| {
            let e = r.unwrap_err();
            assert_eq!(e.kind, kind, "{why}: {e}");
            let w = world.borrow();
            assert_eq!(w.now_ms, 0, "{why}");
            assert_eq!(w.find_window_calls, 1, "{why}");
            assert_eq!(w.capture_calls, 0, "{why}");
            e
        };
        // 見つからない (12)
        let mut w = base();
        w.top.clear();
        let world = w.shared();
        let (r, _) = exec(&world, &notepad_policy(), &mut StopSet::new(), &s);
        check(&world, r, ErrorKind::WindowNotFound, "見つからない");
        // 許可の外れ (4)
        let world = base().shared();
        let denied_class = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nclasses=[\"TargetClass\"]\n"
        ));
        let (r, _) = exec(&world, &denied_class, &mut StopSet::new(), &s);
        let e = check(&world, r, ErrorKind::WindowNotFound, "許可の外れ");
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
        // ai-cu を起動した側の窓 (12。D-139)
        let world = base().shared();
        let (r, _) = exec_with(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &s,
            None,
            &[np.info.pid],
        );
        let e = check(&world, r, ErrorKind::WindowNotFound, "ホストの窓");
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
        // 帯の中の小さい deny の窓が見えている (7)
        let small = Rect::new(102, 102, 5, 5);
        let mut w = base();
        w.windows_at = vec![(small, terminal())];
        w.stack = vec![
            stacked(terminal(), small, false),
            stacked(np.info.clone(), NOTEPAD_RECT, false),
        ];
        let world = w.shared();
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nprocesses=[\"zed.exe\"]\n"
        ));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = check(&world, r, ErrorKind::ForegroundGuard, "deny の窓");
        assert_eq!(e.hint.as_deref(), Some(CAPTURE_SENSITIVE_HINT));
    }

    /// 間引きの点に掛かる deny の窓の覆いは隠れとして猶予を使い、使い切ってから隠れの 7 (D-077 の判定は変えない)
    #[test]
    fn screenshot_window_denied_cover_on_sample_point_uses_grace() {
        let mut w = covered_notepad_world();
        w.windows_at = vec![(NOTEPAD_RECT, terminal())];
        w.stack = vec![
            stacked(terminal(), NOTEPAD_RECT, false),
            stacked(w.top[0].info.clone(), NOTEPAD_RECT, false),
        ];
        let world = w.shared();
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nprocesses=[\"zed.exe\"]\n"
        ));
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &script(NOTEPAD_SHOT));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CAPTURE_OCCLUDED_HINT));
        assert!(e.message.contains("ms 待っても隠れたまま"), "{}", e.message);
        let w = world.borrow();
        assert_eq!(w.capture_calls, 0);
        assert!(w.now_ms >= OCCLUDED_GRACE_MS, "{}", w.now_ms);
    }

    /// 猶予の間も停止要因を見る: poll 1 は操作の前、poll 2 は猶予の 1 回目の待ち (通す)、poll 3 は 2 回目の待ちで
    /// 止まる。2 回解いた (解き直してから止まった) 後で、撮らない
    #[test]
    fn occluded_grace_honors_stop_sources() {
        let world = covered_notepad_world().shared();
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(2)));
        let (r, images) = exec(&world, &notepad_policy(), &mut stops, &script(NOTEPAD_SHOT));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.action_index, Some(0));
        assert!(images.written.is_empty());
        let w = world.borrow();
        assert_eq!(w.capture_calls, 0);
        assert_eq!(w.find_window_calls, 2);
        assert_eq!(w.now_ms, 2 * STOP_POLL_MS);
    }

    // --- 隠れの 7 の error.detail (D-135) ---

    /// 撮る矩形 (メモ帳の枠)
    const OCC_RECT: Rect = Rect::new(100, 50, 640, 480);
    /// 覆う窓 (chrome、0x11) の枠
    const OCC_COVER: Rect = Rect::new(500, 300, 400, 400);

    /// 撮る矩形 `OCC_RECT` のメモ帳 (0x20) の右下を、枠 `OCC_COVER` の chrome (0x11) が覆う世界。重なりの一覧は `stack`
    fn occluded_world(stack: Vec<StackWindow>) -> World {
        let mut np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        np.rect = Some(OCC_RECT);
        let mut w = World::new();
        w.default_window = Some(np.info.clone());
        w.top = vec![np];
        w.windows_at = vec![(OCC_COVER, chrome())];
        w.stack = stack;
        w
    }

    /// `resolve_capture_window` を直接呼び、隠れの 7 を返す (kind も確かめる)
    fn resolve_occluded(world: &Rc<RefCell<World>>) -> AppError {
        let desktop = MockDesktop(world.clone());
        let screen = world.borrow().screen;
        let target = WindowTarget {
            process: "notepad.exe".into(),
            title: None,
        };
        let e = resolve_capture_window(
            &desktop,
            &notepad_policy(),
            screen,
            &target,
            &[],
            None,
            Surface::Cli,
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CAPTURE_OCCLUDED_HINT));
        e
    }

    /// 撮る矩形の外の別の窓 (一覧の先頭に置き、hwnd の照合を確かめる)
    fn elsewhere(rect: Rect) -> StackWindow {
        stacked(other("explorer.exe", 0x30, 5), rect, false)
    }

    /// 隠れの 7 の detail: 覆う窓 (題名なし)、見つけた点、見えている部分の外接矩形、see_through。
    /// 点は間引きの内側 8、間隔 64 で最初に覆いに当たる (108 + 64 * 7, 58 + 64 * 4)
    #[test]
    fn occluded_detail_names_cover_point_visible_and_see_through() {
        let world = occluded_world(vec![
            elsewhere(Rect::new(2000, 2000, 100, 100)),
            stacked(chrome(), OCC_COVER, true),
        ])
        .shared();
        let e = resolve_occluded(&world);
        let d = detail(&e);
        assert_eq!(d["window"]["process"], "chrome.exe");
        assert_eq!(d["window"]["class"], "Chrome_WidgetWin_1");
        assert_eq!(d["window"]["pid"], 2);
        assert_eq!(d["window"]["hwnd"], 0x11);
        assert!(d["window"].get("title").is_none(), "{d}");
        assert_no_title(&e);
        assert_eq!(d["point"], serde_json::json!({ "x": 556, "y": 314 }));
        assert_eq!(
            d["visible"],
            serde_json::json!({ "x": 500, "y": 300, "w": 240, "h": 230 })
        );
        assert_eq!(d["see_through"], true);
        assert_eq!(world.borrow().stack_calls, 1);
    }

    /// 覆う窓の手前の不透明な窓は visible から引く (機密の 7 と同じ定義)。see_through は偽で入る
    #[test]
    fn occluded_detail_visible_subtracts_opaque_windows_in_front() {
        let world = occluded_world(vec![
            elsewhere(Rect::new(700, 300, 200, 400)),
            stacked(chrome(), OCC_COVER, false),
        ])
        .shared();
        let e = resolve_occluded(&world);
        let d = detail(&e);
        assert_eq!(
            d["visible"],
            serde_json::json!({ "x": 500, "y": 300, "w": 200, "h": 230 })
        );
        assert_eq!(d["see_through"], false);
    }

    /// 覆う窓が一覧に無ければ visible と see_through を省く (window と point はある。7 のまま)
    #[test]
    fn occluded_detail_omits_visible_when_cover_is_not_in_stack() {
        let world = occluded_world(vec![elsewhere(OCC_COVER)]).shared();
        let e = resolve_occluded(&world);
        let d = detail(&e);
        assert_eq!(d["window"]["hwnd"], 0x11);
        assert_eq!(d["point"], serde_json::json!({ "x": 556, "y": 314 }));
        assert!(d.get("visible").is_none(), "{d}");
        assert!(d.get("see_through").is_none(), "{d}");
    }

    /// 一覧が読めなくても 7 のまま (8 にしない)。visible と see_through を省く
    #[test]
    fn occluded_detail_omits_visible_when_stack_read_fails() {
        let mut w = occluded_world(vec![stacked(chrome(), OCC_COVER, false)]);
        w.stack_err = true;
        let world = w.shared();
        let e = resolve_occluded(&world);
        let d = detail(&e);
        assert_eq!(d["window"]["process"], "chrome.exe");
        assert_eq!(d["point"], serde_json::json!({ "x": 556, "y": 314 }));
        assert!(d.get("visible").is_none(), "{d}");
        assert!(d.get("see_through").is_none(), "{d}");
        assert_eq!(world.borrow().stack_calls, 1);
    }

    /// 覆う窓の枠が撮る矩形と交わらなければ visible だけを省く (see_through はある)
    #[test]
    fn occluded_detail_omits_only_visible_when_frame_does_not_meet_rect() {
        let world = occluded_world(vec![stacked(
            chrome(),
            Rect::new(2000, 2000, 100, 100),
            false,
        )])
        .shared();
        let e = resolve_occluded(&world);
        let d = detail(&e);
        assert!(d.get("visible").is_none(), "{d}");
        assert_eq!(d["see_through"], false);
    }

    // --- 撮影の対象の 7 と 12 の error.detail.window (D-135) ---

    /// メモ帳 (0x20、pid 4242) だけがある世界。`edit` で窓を変える
    fn capture_world(edit: impl FnOnce(&mut TopWindow)) -> Rc<RefCell<World>> {
        let mut np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        edit(&mut np);
        let mut w = World::new();
        w.top = vec![np];
        w.shared()
    }

    /// `resolve_capture_window` を直接呼び、失敗を返す
    fn resolve_capture_err(
        world: &Rc<RefCell<World>>,
        policy: &Policy,
        process: &str,
        host_pids: &[u32],
    ) -> AppError {
        let desktop = MockDesktop(world.clone());
        let screen = world.borrow().screen;
        let target = WindowTarget {
            process: process.into(),
            title: None,
        };
        resolve_capture_window(
            &desktop,
            policy,
            screen,
            &target,
            host_pids,
            None,
            Surface::Cli,
        )
        .unwrap_err()
    }

    /// 見つけた窓の window (題名なし)
    fn assert_found_window(e: &AppError) {
        let d = detail(e);
        assert_eq!(d["window"]["process"], "Notepad.exe");
        assert_eq!(d["window"]["class"], "TargetClass");
        assert_eq!(d["window"]["pid"], 4242);
        assert_eq!(d["window"]["hwnd"], 0x20);
        assert!(d["window"].get("title").is_none(), "{d}");
        assert_no_title(e);
    }

    /// 名指しの失敗 `e` が、同じ名指しで窓が 1 つも無いときの失敗と同じ (kind、message、hint、detail。D-139)
    fn assert_capture_like_absent(e: &AppError, policy: &Policy, process: &str) {
        let mut w = World::new();
        w.top.clear();
        let absent = resolve_capture_err(&w.shared(), policy, process, &[]);
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert_like_absent(e, &absent);
        assert!(e.detail.is_none(), "{:?}", e.detail);
    }

    /// 起動した側の窓は名指しの候補に入らない。無いときと同じ 12 で、window を持たない (D-139。以前は 7)
    #[test]
    fn capture_host_window_is_not_found_like_absent() {
        let world = capture_world(|_| {});
        let p = notepad_policy();
        let e = resolve_capture_err(&world, &p, "notepad.exe", &[4242]);
        assert_capture_like_absent(&e, &p, "notepad.exe");
    }

    /// 起動した側で deny の窓も、無いときと同じ 12 (D-139。以前は 7)
    #[test]
    fn capture_host_denied_window_is_not_found_like_absent() {
        let world = capture_world(|_| {});
        let p = policy(&format!("{ENABLED}[deny]\nprocesses=[\"notepad\"]\n"));
        let e = resolve_capture_err(&world, &p, "notepad.exe", &[4242]);
        assert_capture_like_absent(&e, &p, "notepad.exe");
    }

    /// 許可された窓が最小化なら 12 で、その窓の window を持つ
    #[test]
    fn capture_minimized_detail_has_window() {
        let world = capture_world(|np| {
            np.minimized = true;
            np.rect = None;
        });
        let e = resolve_capture_err(&world, &notepad_policy(), "notepad.exe", &[]);
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(e.message.contains("最小化"), "{}", e.message);
        assert_found_window(&e);
    }

    /// 画面の外の 12 は、その窓の window を持つ
    #[test]
    fn capture_offscreen_detail_has_window() {
        let world = capture_world(|np| np.rect = Some(Rect::new(-900, 0, 800, 600)));
        let e = resolve_capture_err(&world, &notepad_policy(), "notepad.exe", &[]);
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(e.message.contains("の外にある"), "{}", e.message);
        assert_found_window(&e);
    }

    /// deny の窓が最小化なら、無いときと同じ 12 で、「最小化」の文も detail も出さない (許可の外の窓の有無を
    /// 漏らさない。D-139)
    #[test]
    fn capture_minimized_denied_window_has_no_detail() {
        let world = capture_world(|np| {
            np.minimized = true;
            np.rect = None;
        });
        let p = policy(&format!("{ENABLED}[deny]\nprocesses=[\"notepad\"]\n"));
        let e = resolve_capture_err(&world, &p, "notepad.exe", &[]);
        assert!(!e.message.contains("最小化"), "{}", e.message);
        assert_capture_like_absent(&e, &p, "notepad.exe");
    }

    /// 実行器の screenshot の window (D-139): TUI の端末 (self_hwnd) の窓は名指しの候補に入らず 12 で、撮らない。
    /// 同じ実行ファイル名の別の窓が後ろにあれば、そちらを撮る
    #[test]
    fn grab_window_skips_tui_terminal() {
        let me = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let mut w = World::new();
        w.default_window = Some(me.info.clone());
        w.top = vec![me.clone()];
        let world = w.shared();
        let s = script(NOTEPAD_SHOT);
        let p = notepad_policy();
        let (r, images) = exec_excluding(&world, &p, &mut StopSet::new(), &s, Some(&me.info));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
        assert!(e.detail.is_none(), "{:?}", e.detail);
        assert_eq!(world.borrow().capture_calls, 0);
        assert!(images.written.is_empty());
        // 後ろに同じ実行ファイル名の別の窓があれば、そちらが見つかる
        let mut behind = top(0x21, "Notepad.exe", "b.txt - メモ帳");
        behind.info.pid = 4243;
        // 隠れの判定 (座標の下の窓) が後ろの窓自身を返すようにする
        world.borrow_mut().default_window = Some(behind.info.clone());
        world.borrow_mut().top.push(behind);
        let (r, _) = exec_excluding(&world, &p, &mut StopSet::new(), &s, Some(&me.info));
        let report = r.unwrap();
        assert_eq!(report.screenshots[0].window.as_ref().unwrap().hwnd, 0x21);
    }

    /// 見つからない 12 は detail を持たない (足す情報が無い)
    #[test]
    fn capture_not_found_has_no_detail() {
        let world = capture_world(|_| {});
        let e = resolve_capture_err(&world, &notepad_policy(), "absent.exe", &[]);
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert!(e.message.contains("見つからない"), "{}", e.message);
        assert!(e.detail.is_none(), "{:?}", e.detail);
    }

    /// Z 順の窓 1 つ (D-077)。`stack_windows` と同じく題名は空にする
    fn stacked(info: WindowInfo, rect: Rect, see_through: bool) -> StackWindow {
        let mut info = info;
        info.title = String::new();
        StackWindow {
            info,
            rect,
            see_through,
            hung: false,
        }
    }

    /// 応答しない (`IsHungAppWindow` が真の) 不透明な窓 (D-115)
    fn hung_stacked(info: WindowInfo, rect: Rect) -> StackWindow {
        StackWindow {
            hung: true,
            ..stacked(info, rect, false)
        }
    }

    /// OS が出すゴーストの窓 (D-115)。実際の形に合わせて process は空 (ai-cu の読み方では dwm.exe の名前が
    /// 読めない見込み)、pid と hwnd は応答しない窓とも `terminal()` とも別 (ゴースト自身が除外で外れないように)
    fn ghost(class: &str, rect: Rect) -> StackWindow {
        let mut info = other("", 0x50, 2024);
        info.class = class.into();
        stacked(info, rect, false)
    }

    fn notepad_cover(rect: Rect) -> StackWindow {
        stacked(other("notepad.exe", 0x20, 3), rect, false)
    }

    fn deny_zed() -> Policy {
        policy(&format!("{ENABLED}[deny]\nprocesses=[\"zed.exe\"]\n"))
    }

    const WHOLE: Rect = Rect::new(0, 0, 1000, 1000);

    /// 前面の不透明な窓に完全に覆われた deny の窓は通り、1 ピクセルでも残れば (その窓, 残りの外接矩形) (D-077)。
    /// 覆いが撮る矩形の外まで伸びていても同じ
    #[test]
    fn sensitive_window_passes_covered_and_refuses_one_pixel_visible() {
        let p = deny_zed();
        let zed = stacked(terminal(), Rect::new(100, 100, 200, 200), false);
        let covered = [notepad_cover(Rect::new(0, 0, 500, 500)), zed.clone()];
        assert_eq!(sensitive_window(&covered, &p, &WHOLE, &[], None), None);
        let beyond = [
            notepad_cover(Rect::new(-100, -100, 3000, 3000)),
            zed.clone(),
        ];
        assert_eq!(sensitive_window(&beyond, &p, &WHOLE, &[], None), None);
        // 右端の 1 列だけ残る
        let strip = [notepad_cover(Rect::new(0, 0, 299, 500)), zed.clone()];
        let (w, visible) = sensitive_window(&strip, &p, &WHOLE, &[], None).unwrap();
        assert_eq!(w.hwnd, 0x10);
        assert_eq!(w.process, "Zed.exe");
        assert_eq!(visible, Rect::new(299, 100, 1, 200));
        // 覆いが無ければ、撮る矩形との重なり全体
        let (_, visible) =
            sensitive_window(&[zed], &p, &Rect::new(150, 0, 1000, 250), &[], None).unwrap();
        assert_eq!(visible, Rect::new(150, 100, 150, 150));
    }

    /// 透けうる窓 (see_through) は覆いにしない。deny の窓より奥の窓は、どれだけ大きく不透明でも覆いにしない (D-077)
    #[test]
    fn sensitive_window_ignores_see_through_covers_and_windows_behind() {
        let p = deny_zed();
        let zed = stacked(terminal(), Rect::new(100, 100, 200, 200), false);
        let glass = stacked(chrome(), Rect::new(0, 0, 1000, 1000), true);
        let (w, visible) =
            sensitive_window(&[glass.clone(), zed.clone()], &p, &WHOLE, &[], None).unwrap();
        assert_eq!((w.hwnd, visible), (0x10, Rect::new(100, 100, 200, 200)));
        // 奥の大きな不透明の窓
        let behind = [zed.clone(), notepad_cover(Rect::new(0, 0, 1000, 1000))];
        let (w, visible) = sensitive_window(&behind, &p, &WHOLE, &[], None).unwrap();
        assert_eq!((w.hwnd, visible), (0x10, Rect::new(100, 100, 200, 200)));
        // 透ける窓と不透明な窓が両方前面にあれば、不透明な方だけが引かれる
        let mixed = [
            glass,
            notepad_cover(Rect::new(100, 100, 100, 200)),
            zed.clone(),
        ];
        let (_, visible) = sensitive_window(&mixed, &p, &WHOLE, &[], None).unwrap();
        assert_eq!(visible, Rect::new(200, 100, 100, 200));
    }

    /// 撮る矩形の外の deny の窓は見ない。allow に無いだけの窓は数えない。起動した側 (host_pids) と TUI の端末
    /// (self_hwnd) の窓は deny でも数えないが、覆いとしては引く (本人の決定、D-077)
    #[test]
    fn sensitive_window_skips_rects_outside_allowed_and_host_windows() {
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad.exe\"]\n[deny]\nprocesses=[\"zed.exe\",\"keepass.exe\"]\n"
        ));
        let zed = stacked(terminal(), Rect::new(100, 100, 200, 200), false);
        assert_eq!(
            sensitive_window(
                std::slice::from_ref(&zed),
                &p,
                &Rect::new(300, 100, 50, 50),
                &[],
                None
            ),
            None,
            "右端のすぐ外に接する矩形"
        );
        let only_not_allowed = [stacked(chrome(), Rect::new(0, 0, 1000, 1000), false)];
        assert_eq!(
            sensitive_window(&only_not_allowed, &p, &WHOLE, &[], None),
            None
        );
        let keepass = stacked(
            other("KeePass.exe", 0x30, 5),
            Rect::new(150, 150, 50, 50),
            false,
        );
        let stack = [zed.clone(), keepass.clone()];
        assert_eq!(sensitive_window(&stack, &p, &WHOLE, &[1], None), None);
        assert_eq!(sensitive_window(&stack, &p, &WHOLE, &[], Some(0x10)), None);
        let (w, _) = sensitive_window(&stack, &p, &WHOLE, &[], None).unwrap();
        assert_eq!(w.hwnd, 0x10, "起動した側でなければ前面の deny の窓");
        // 起動した側の窓が奥の deny の窓を一部だけ覆うなら、残りで断る
        let partial = [
            stacked(terminal(), Rect::new(100, 100, 60, 200), false),
            keepass,
        ];
        let (w, visible) = sensitive_window(&partial, &p, &WHOLE, &[1], None).unwrap();
        assert_eq!((w.hwnd, visible), (0x30, Rect::new(160, 150, 40, 50)));
    }

    /// プロセス名が読めない窓 (昇格したプロセスなど) は、クラスが deny に当たるときだけ数える (D-077)
    #[test]
    fn sensitive_window_judges_unnamed_window_by_class() {
        let mut unnamed = other("", 0x40, 6);
        unnamed.class = "SecretClass".into();
        let stack = [stacked(unnamed, Rect::new(0, 0, 10, 10), false)];
        assert_eq!(
            sensitive_window(&stack, &deny_zed(), &WHOLE, &[], None),
            None
        );
        let by_class = policy(&format!("{ENABLED}[deny]\nclasses=[\"secretclass\"]\n"));
        let (w, _) = sensitive_window(&stack, &by_class, &WHOLE, &[], None).unwrap();
        assert_eq!(w.hwnd, 0x40);
    }

    /// 操作列の screenshot (region): deny の窓が見えていれば撮らずに 7。detail に window (題名なし) と visible、
    /// hint は CAPTURE_SENSITIVE_HINT (前の操作が済んでいれば実行済みの一文が頭に付く) (D-077)
    #[test]
    fn run_screenshot_region_refuses_visible_denied_window_with_detail() {
        let mut w = World::new();
        w.stack = vec![
            notepad_cover(Rect::new(0, 0, 150, 400)),
            stacked(terminal(), Rect::new(100, 100, 200, 200), false),
        ];
        let world = w.shared();
        let s = script(
            r#"{"type":"wait","ms":10},{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":400,"h":400}}"#,
        );
        let (r, images) = exec(&world, &deny_zed(), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.action_index, Some(1));
        assert_eq!(progress(&e)["executed"], 1);
        assert_eq!(
            e.message,
            "撮る矩形 0,0,400,400 に、設定で拒否されたウィンドウ (process=\"Zed.exe\", class=\"Chrome_WidgetWin_1\") が見えている"
        );
        let h = e.hint.as_deref().unwrap();
        assert!(h.starts_with("actions[0] は実行済み (executed=1)。"), "{h}");
        assert!(h.ends_with(CAPTURE_SENSITIVE_HINT), "{h}");
        let d = e.detail.as_deref().unwrap();
        assert_eq!(
            d["window"],
            serde_json::json!({ "process": "Zed.exe", "class": "Chrome_WidgetWin_1", "pid": 1, "hwnd": 0x10 })
        );
        assert_eq!(
            d["visible"],
            serde_json::json!({ "x": 150, "y": 100, "w": 150, "h": 200 })
        );
        assert!(images.written.is_empty(), "何も撮らない");
        assert_eq!(world.borrow().capture_calls, 0);
        assert_eq!(world.borrow().stack_calls, 1);
        assert!(!e.message.contains("ゴースト"), "{}", e.message);
    }

    const GHOST_AREA: Rect = Rect::new(100, 100, 200, 200);

    /// 応答しない deny の窓があれば、その真前のゴーストの窓 (クラス Ghost) を deny の窓として数える。
    /// ゴーストが元の窓と重ならない位置に動かされても当たる (元の窓と結ばない。D-115)
    #[test]
    fn sensitive_window_counts_ghost_when_denied_window_hangs() {
        let p = deny_zed();
        let stack = [
            ghost("Ghost", GHOST_AREA),
            hung_stacked(terminal(), GHOST_AREA),
        ];
        let (w, visible) = sensitive_window(&stack, &p, &Rect::new(150, 150, 10, 10), &[], None)
            .expect("ゴーストの真後ろの応答しない deny の窓");
        assert_eq!((w.class.as_str(), w.hwnd), ("Ghost", 0x50));
        assert_eq!(visible, Rect::new(150, 150, 10, 10));
        // 動かしたゴースト
        let moved_area = Rect::new(500, 500, 100, 100);
        let moved = [
            ghost("Ghost", moved_area),
            hung_stacked(terminal(), GHOST_AREA),
        ];
        let (w, visible) = sensitive_window(&moved, &p, &Rect::new(520, 520, 10, 10), &[], None)
            .expect("動かしたゴースト");
        assert_eq!(w.class, "Ghost", "動かしたゴースト");
        assert_eq!(visible, Rect::new(520, 520, 10, 10), "動かしたゴースト");
    }

    /// 応答しない deny の窓が無ければ、ゴーストは数えない (deny の窓が応答している、または応答しないのが
    /// deny でない窓)。ゴーストに覆われた元の窓も見えない。応答しない deny の窓があっても、クラスが Ghost で
    /// ない窓は数えない (D-115)
    #[test]
    fn sensitive_window_ignores_ghost_without_hung_denied_window() {
        let p = deny_zed();
        let answering = [
            ghost("Ghost", GHOST_AREA),
            stacked(terminal(), GHOST_AREA, false),
        ];
        assert_eq!(
            sensitive_window(&answering, &p, &WHOLE, &[], None),
            None,
            "zed.exe が応答している"
        );
        let notepad_hangs = [
            ghost("Ghost", GHOST_AREA),
            hung_stacked(other("notepad.exe", 0x20, 3), GHOST_AREA),
        ];
        assert_eq!(
            sensitive_window(&notepad_hangs, &p, &WHOLE, &[], None),
            None,
            "応答しないのは deny でない notepad.exe"
        );
        // 応答しない deny の窓があっても、その前にあるゴーストでない窓は数えない (覆いとして引くだけ)
        let non_ghost_cover = [
            notepad_cover(GHOST_AREA),
            hung_stacked(terminal(), GHOST_AREA),
        ];
        assert_eq!(
            sensitive_window(
                &non_ghost_cover,
                &p,
                &Rect::new(150, 150, 10, 10),
                &[],
                None
            ),
            None,
            "ゴーストでない notepad.exe の窓"
        );
    }

    /// 起動した側 (host_pids) と TUI の端末 (self_hwnd) の応答しない窓は、ゴーストの元に数えない (D-115)
    #[test]
    fn sensitive_window_ghost_rule_skips_host_windows() {
        let p = deny_zed();
        let stack = [
            ghost("Ghost", GHOST_AREA),
            hung_stacked(terminal(), GHOST_AREA),
        ];
        assert_eq!(
            sensitive_window(&stack, &p, &WHOLE, &[1], None),
            None,
            "host_pids"
        );
        assert_eq!(
            sensitive_window(&stack, &p, &WHOLE, &[], Some(0x10)),
            None,
            "self_hwnd"
        );
    }

    /// ゴーストのクラス名は大文字小文字を区別しない (D-115)
    #[test]
    fn sensitive_window_ghost_class_is_case_insensitive() {
        let stack = [
            ghost("ghost", GHOST_AREA),
            hung_stacked(terminal(), GHOST_AREA),
        ];
        let (w, _) = sensitive_window(&stack, &deny_zed(), &WHOLE, &[], None).expect("ghost");
        assert_eq!(w.hwnd, 0x50);
        assert!(is_ghost_window(&w));
    }

    /// 操作列の screenshot (region): 応答しない deny の窓のゴーストが見えていれば 7。message にゴーストと添え、
    /// detail の window はゴースト、rule は応答しない窓に当たった設定のキーで、fix は無い (D-077、D-079、D-115)
    #[test]
    fn run_screenshot_region_refuses_ghost_of_hung_denied_window() {
        let refuse = |p: &Policy| {
            let mut w = World::new();
            w.stack = vec![
                ghost("Ghost", GHOST_AREA),
                hung_stacked(terminal(), GHOST_AREA),
            ];
            let world = w.shared();
            let s = script(
                r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":400,"h":400}}"#,
            );
            let (r, images) = exec(&world, p, &mut StopSet::new(), &s);
            assert!(images.written.is_empty(), "何も撮らない");
            assert_eq!(world.borrow().capture_calls, 0);
            r.unwrap_err()
        };
        let e = refuse(&deny_zed());
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(
            e.message,
            "撮る矩形 0,0,400,400 に、設定で拒否されたウィンドウ (process=\"\", class=\"Ghost\") が見えている (応答しない deny のウィンドウの代わりに OS が出したゴースト)"
        );
        let v = e.to_json();
        assert_eq!(
            v["error"]["detail"]["window"],
            serde_json::json!({ "process": "", "class": "Ghost", "pid": 2024, "hwnd": 0x50 }),
            "{v}"
        );
        assert_eq!(
            v["error"]["detail"]["visible"],
            serde_json::json!({ "x": 100, "y": 100, "w": 200, "h": 200 }),
            "{v}"
        );
        assert_eq!(v["error"]["rule"], "deny.processes", "deny.processes: {v}");
        assert!(v["error"].get("fix").is_none(), "{v}");
        // 応答しない窓をクラスで当てたなら、rule は deny.classes
        let by_class = policy(&format!(
            "{ENABLED}[deny]\nclasses=[\"Chrome_WidgetWin_1\"]\n"
        ));
        let e = refuse(&by_class);
        assert!(e.message.contains("ゴースト"), "{}", e.message);
        let v = e.to_json();
        assert_eq!(v["error"]["rule"], "deny.classes", "deny.classes: {v}");
        assert!(v["error"].get("fix").is_none(), "{v}");
    }

    /// 撮る矩形に deny の窓が見えている 7 は、当たった設定のキーの rule だけを付け、fix は付けない
    /// (設定を緩めるよう頼ませない。D-077、D-079)
    #[test]
    fn sensitive_capture_refusal_has_rule_but_no_fix() {
        let mut w = World::new();
        w.stack = vec![stacked(terminal(), Rect::new(100, 100, 200, 200), false)];
        let world = w.shared();
        let s = script(
            r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":400,"h":400}}"#,
        );
        let (r, _) = exec(&world, &deny_zed(), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        let v = e.to_json();
        assert_eq!(v["error"]["rule"], "deny.processes", "{v}");
        assert!(v["error"].get("fix").is_none(), "{v}");
        assert_eq!(e.fix(), None);
    }

    /// 名指しの候補の述語 (D-139) は `windows` の一覧と同じ: 許可された窓は通り、deny のクラス、起動した側
    /// (host_pids)、TUI の端末 (self_hwnd) は通らない
    #[test]
    fn named_window_admitted_matches_windows_listing() {
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad.exe\"]\n[deny]\nclasses=[\"DeniedClass\"]\n"
        ));
        let ok = top(0x20, "notepad.exe", "a").info;
        assert!(named_window_admitted(&p, &ok, &[], None), "許可");
        let mut denied_class = ok.clone();
        denied_class.class = "DeniedClass".into();
        assert!(
            !named_window_admitted(&p, &denied_class, &[], None),
            "deny のクラス"
        );
        assert!(
            !named_window_admitted(&p, &ok, &[ok.pid], None),
            "起動した側"
        );
        assert!(
            !named_window_admitted(&p, &ok, &[], Some(ok.hwnd)),
            "TUI の端末"
        );
        assert!(
            named_window_admitted(&p, &ok, &[ok.pid + 1], Some(ok.hwnd + 1)),
            "別の pid と別の窓なら通る"
        );
        // 一覧の述語 (`cli::window_allowed`) と同じ答え
        for w in [&ok, &denied_class] {
            for host in [&[][..], &[ok.pid][..]] {
                assert_eq!(
                    crate::cli::window_allowed(&p, w, host),
                    named_window_admitted(&p, w, host, None)
                );
            }
        }
    }

    /// 名指しの前の名前だけの判定 (D-139): allow.processes の外の名前は 4 で、rule と fix が付く
    #[test]
    fn check_named_process_denies_by_name_with_fix() {
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"calc.exe\"]\n"));
        assert!(check_named_process(&p, "calc.exe").is_ok());
        let e = check_named_process(&p, "notepad.exe").unwrap_err();
        assert_eq!(e.kind, ErrorKind::PolicyDenied, "{e}");
        assert_eq!(e.kind.exit_code(), 4);
        let v = e.to_json();
        assert_eq!(v["error"]["rule"], "allow.processes", "{v}");
        let fix = e.fix().expect("fix");
        assert!(
            fix.contains("allow.processes に \"notepad.exe\" を足す"),
            "{fix}"
        );
    }

    /// 検証の失敗のエラーは、最初の問題 (添字と同じもの) の rule と fix を写す (D-079)
    #[test]
    fn validation_error_copies_first_issue_rule_and_fix() {
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"calc.exe\"]\n[deny]\nactions=[\"close\"]\n"
        ));
        let s = script(
            r#"{"type":"wait","ms":1},{"type":"focus","process":"notepad.exe"},{"type":"close","process":"calc.exe","title":"x"}"#,
        );
        let e = validation_error(&crate::validate::validate(&s, &p, None)).unwrap();
        assert_eq!(e.action_index, Some(1));
        let v = e.to_json();
        assert_eq!(v["error"]["rule"], "allow.processes", "{v}");
        let fix = v["error"]["fix"].as_str().unwrap();
        assert!(
            fix.contains("allow.processes に \"notepad.exe\" を足す"),
            "{fix}"
        );
        // 組み込みのキーが最初なら rule は builtin で fix が無い
        let s = script(r#"{"type":"key","keys":"win+r"},{"type":"focus","process":"notepad.exe"}"#);
        let e = validation_error(&crate::validate::validate(&s, &p, None)).unwrap();
        assert_eq!(e.action_index, Some(0));
        assert_eq!(
            e.deny.as_ref().map(|d| d.rule),
            Some(crate::policy::RULE_BUILTIN)
        );
        assert_eq!(e.fix(), None);
        // 形の誤りが勝つときは、その問題 (rule なし) を写す
        let s = script(
            r#"{"type":"focus","process":"notepad.exe"},{"type":"key","keys":"ctrl+bogus"}"#,
        );
        let e = validation_error(&crate::validate::validate(&s, &p, None)).unwrap();
        assert_eq!(e.kind, ErrorKind::InvalidScript);
        assert!(e.deny.is_none());
    }

    /// どの層にも deny のプロセスかクラスが無ければ窓を列挙せず、今と同じに撮る (allow の一覧だけでも同じ。D-077)
    #[test]
    fn run_screenshot_without_deny_lists_does_not_enumerate() {
        for toml in [
            ENABLED.to_string(),
            format!("{ENABLED}[allow]\nprocesses=[\"notepad.exe\"]\n"),
        ] {
            let mut w = World::new();
            w.stack = vec![stacked(terminal(), Rect::new(0, 0, 100, 100), false)];
            let world = w.shared();
            let s = script(
                r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":50,"h":50}}"#,
            );
            let (r, images) = exec(&world, &policy(&toml), &mut StopSet::new(), &s);
            assert!(r.is_ok(), "{toml}: {r:?}");
            assert_eq!(images.written.len(), 1);
            assert_eq!(world.borrow().stack_calls, 0, "{toml}");
        }
    }

    /// window で撮るとき、対象の枠の内側 8 ピクセルの帯 (隠れの確認の間引きが見ない所) に小さい deny の窓が
    /// あれば、隠れの確認ではなく機密の判定が 7 で断る。対象が deny の窓の前面で覆っていれば通る (D-077)
    #[test]
    fn run_screenshot_window_refuses_small_denied_window_over_target() {
        let np = top(0x20, "Notepad.exe", "a.txt - メモ帳");
        let small = Rect::new(102, 102, 5, 5);
        let mut w = World::new();
        w.default_window = Some(np.info.clone());
        w.windows_at = vec![(small, terminal())];
        w.stack = vec![
            stacked(terminal(), small, false),
            stacked(np.info.clone(), Rect::new(100, 100, 800, 600), false),
        ];
        w.top = vec![np.clone()];
        let world = w.shared();
        let p = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nprocesses=[\"zed.exe\"]\n"
        ));
        let s =
            script(r#"{"type":"screenshot","path":"a.png","window":{"process":"notepad.exe"}}"#);
        let (r, images) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(
            e.message.contains("設定で拒否されたウィンドウ"),
            "{}",
            e.message
        );
        assert_eq!(e.hint.as_deref(), Some(CAPTURE_SENSITIVE_HINT));
        assert_ne!(e.hint.as_deref(), Some(CAPTURE_OCCLUDED_HINT));
        assert_eq!(
            e.detail.as_deref().unwrap()["visible"],
            serde_json::json!({ "x": 102, "y": 102, "w": 5, "h": 5 })
        );
        assert!(images.written.is_empty());
        assert_eq!(world.borrow().capture_calls, 0);
        // 対象が前面で deny の窓を覆っていれば撮る
        world.borrow_mut().stack = vec![
            stacked(np.info.clone(), Rect::new(100, 100, 800, 600), false),
            stacked(terminal(), small, false),
        ];
        world.borrow_mut().windows_at.clear();
        let (r, images) = exec(&world, &p, &mut StopSet::new(), &s);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(images.written.len(), 1);
    }

    /// TUI からの実行 (self_hwnd): ai-cu を動かしている端末の窓は deny に当たっても数えない (D-077)
    #[test]
    fn run_screenshot_skips_self_window_in_tui() {
        let mut w = World::new();
        w.stack = vec![stacked(terminal(), Rect::new(0, 0, 100, 100), false)];
        let world = w.shared();
        let s =
            script(r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":50,"h":50}}"#);
        let me = terminal();
        let (r, images) = exec_excluding(&world, &deny_zed(), &mut StopSet::new(), &s, Some(&me));
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(images.written.len(), 1);
        let (r, _) = exec(&world, &deny_zed(), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap_err().kind, ErrorKind::ForegroundGuard);
    }

    /// 縮尺 (D-053): 保存する画像を縮め、結果の scale と大きさに反映する。原点は変わらない
    #[test]
    fn screenshot_scale_shrinks_saved_image() {
        let world = World::new().shared();
        let s = script(
            r#"{"type":"screenshot","path":"a.png","region":{"x":10,"y":20,"w":40,"h":20},"scale":0.5}"#,
        );
        let (r, images) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        let shot = &report.screenshots[0];
        assert_eq!((shot.x, shot.y, shot.width, shot.height), (10, 20, 20, 10));
        assert_eq!(shot.scale, 0.5);
        let saved = &images.written[0].1;
        assert_eq!((saved.width, saved.height), (20, 10));
    }

    /// 長辺の上限 (D-072): 縮尺は scale と max_edge / 長辺 の小さい方。結果の scale に実際の値が入る。原点は変わらない
    #[test]
    fn screenshot_max_edge_shrinks_long_edge_and_reports_effective_scale() {
        let region = r#""region":{"x":10,"y":20,"w":40,"h":20}"#;
        for (extra, size, scale) in [
            (r#""max_edge":10"#, (10, 5), 0.25),
            (r#""scale":0.1,"max_edge":10"#, (4, 2), 0.1),
            (r#""max_edge":100"#, (40, 20), 1.0),
        ] {
            let world = World::new().shared();
            let s = script(&format!(
                r#"{{"type":"screenshot","path":"a.png",{region},{extra}}}"#
            ));
            let (r, images) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
            let report = r.unwrap();
            let shot = &report.screenshots[0];
            assert_eq!((shot.x, shot.y), (10, 20), "{extra}");
            assert_eq!((shot.width, shot.height), size, "{extra}");
            assert_eq!(shot.scale, scale, "{extra}");
            let saved = &images.written[0].1;
            assert_eq!((saved.width, saved.height), size, "{extra}");
        }
    }

    /// MCP の既定 1568 (D-072) は操作列に漏れない。矩形は 1568 より大きくする (D-072)
    #[test]
    fn screenshot_without_max_edge_keeps_full_size_above_mcp_default() {
        let world = World::new().shared();
        let s = script(
            r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":2000,"h":1000}}"#,
        );
        let (r, images) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        let shot = &report.screenshots[0];
        assert_eq!((shot.width, shot.height), (2000, 1000));
        assert_eq!(shot.scale, 1.0);
        let saved = &images.written[0].1;
        assert_eq!((saved.width, saved.height), (2000, 1000));
    }

    /// 失敗の進み具合 (D-065) を読む
    fn progress(e: &AppError) -> &serde_json::Value {
        e.progress.as_deref().expect("progress")
    }

    /// 座標の下が許可されたメモ帳、(500,0)-(600,100) だけ許可外の chrome の world と、メモ帳だけ許可する設定
    fn notepad_with_chrome_corner() -> (Rc<RefCell<World>>, Policy) {
        let mut w = World::new();
        w.default_window = Some(top(0x20, "notepad.exe", "a.txt").info);
        w.windows_at = vec![(Rect::new(500, 0, 100, 100), chrome())];
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        (w.shared(), p)
    }

    /// 2 つ済んでから 3 つ目が実行時のガードで 7 (D-065): 進み具合と、hint の頭の実行済みの一文
    #[test]
    fn failure_after_click_reports_executed_and_done_prefix() {
        let (world, p) = notepad_with_chrome_corner();
        let s = script(
            r#"{"type":"click","x":10,"y":10},{"type":"wait","ms":10},{"type":"click","x":550,"y":50}"#,
        );
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.action_index, Some(2));
        let pr = progress(&e);
        assert_eq!(pr["executed"], 2);
        assert_eq!(pr["executed"], e.action_index.unwrap());
        assert_eq!(pr["partial"], false);
        let h = e.hint.as_deref().unwrap();
        assert!(
            h.starts_with("actions[0]〜actions[1] は実行済み (executed=2)。操作ごと打ち直さない。残りは、以下の hint に従い、ai-cu windows"),
            "{h}"
        );
        assert!(
            h.ends_with(&format!(
                "以下は失敗した actions[2] について: {MOUSE_GUARD_HINT}"
            )),
            "{h}"
        );
        assert_eq!(world.borrow().sent.len(), 1, "1 つ目の click だけ送った");
        // 出力の JSON の最上位にも出る
        let v = e.to_json();
        assert_eq!(v["executed"], 2);
        assert_eq!(v["ok"], false);
    }

    /// 先頭の操作で失敗 (何も済んでいない): hint は元のまま、executed は 0
    #[test]
    fn failure_at_first_action_keeps_hint_and_reports_zero() {
        let (world, p) = notepad_with_chrome_corner();
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":550,"y":50}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], false);
        assert_eq!(e.hint.as_deref(), Some(MOUSE_GUARD_HINT));
    }

    /// key のバッチを送った後の待ちの途中で止まる: 失敗した操作が途中まで届いている
    #[test]
    fn stop_during_key_settle_marks_partial() {
        let world = World::new().shared();
        let mut stops = StopSet::new();
        // 1 回目 (束の前) と 2 回目 (パスワード欄かを読んだ後。D-090) は通し、待ちの中の 3 回目で止める
        stops.push(Box::new(StopAfterPolls(2)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut stops,
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], true);
        let h = e.hint.as_deref().unwrap();
        assert!(
            h.starts_with("actions[0] も途中まで行った (partial=true)。操作ごと打ち直さない。"),
            "{h}"
        );
        assert!(
            h.ends_with(&format!(
                "以下は actions[0] の止まった所から先について (それより前に起こしたことはそのまま): {}",
                ErrorKind::Stopped.hint()
            )),
            "{h}"
        );
        assert!(!world.borrow().sent.is_empty(), "キーは送られている");
    }

    /// 入力デスクトップが使えない (GetCursorPos の 5) は 14 `other_desktop`、名前は null。OS の失敗は 8 のまま (D-066)
    #[test]
    fn backend_error_maps_desktop_unavailable_to_14_other_desktop() {
        let e = backend_error(BackendError::DesktopUnavailable(
            "GetCursorPos が失敗 (GetLastError=5)".into(),
        ));
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable);
        assert_eq!(e.kind.exit_code(), 14);
        assert_eq!(e.reason, Some("other_desktop"));
        let v = e.to_json();
        assert_eq!(v["error"]["code"], "desktop_unavailable");
        assert_eq!(v["error"]["reason"], "other_desktop");
        assert!(v["error"]["detail"]["input_desktop"].is_null(), "{v}");
        assert!(
            v["error"]["detail"]
                .as_object()
                .unwrap()
                .contains_key("input_desktop"),
            "{v}"
        );
        assert!(
            e.message.contains("入力デスクトップが使えない"),
            "{}",
            e.message
        );

        let os = backend_error(BackendError::Os("GetCursorPos".into()));
        assert_eq!(os.kind, ErrorKind::OsError);
        assert_eq!(os.reason, None);
        assert_eq!(os.detail, None);
    }

    /// UIA の時間切れ (D-104) は 8 の os_error。reason も detail も付けない
    #[test]
    fn backend_error_maps_timed_out_to_os_error() {
        let e = backend_error(BackendError::TimedOut(
            "UIA の読み取りが 30 秒で時間切れ".into(),
        ));
        assert_eq!(e.kind, ErrorKind::OsError);
        assert_eq!(e.kind.exit_code(), 8);
        assert!(e.message.contains("時間切れ"), "{}", e.message);
        assert_eq!(e.reason, None);
        assert_eq!(e.detail, None);
    }

    /// OS の失敗 (一部が挿入されたかもしれない) は partial。先頭のバッチの失敗でも立てる
    #[test]
    fn os_failure_in_batch_is_partial() {
        let world = World::new().shared();
        let p = policy(ENABLED);
        let mut sink = FailOnce {
            world: world.clone(),
            fail_at: 1,
            calls: 0,
        };
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        let e = Executor {
            policy: &p,
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut StopSet::new(),
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute(&script(r#"{"type":"type","text":"ab"}"#))
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::OsError);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], true);
        assert!(
            e.hint
                .as_deref()
                .unwrap()
                .starts_with("actions[0] も途中まで行った (partial=true)。"),
            "{:?}",
            e.hint
        );
        // OS の失敗でも、前置きは「断った」ではなく中立の「止まった所から先について」で結ぶ
        assert!(
            e.hint
                .as_deref()
                .unwrap()
                .contains("以下は actions[0] の止まった所から先について (それより前に起こしたことはそのまま): "),
            "{:?}",
            e.hint
        );
    }

    /// 最下層が断ったバッチ (9) は何も挿入していないので partial ではなく、hint も付けない
    #[test]
    fn backend_refusal_is_not_partial() {
        let mut w = World::new();
        w.refuse = Some(BackendError::Forbidden("dev".into()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":1,"y":1}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForbiddenDevEnv);
        assert_eq!(progress(&e)["partial"], false);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(e.hint, None);
    }

    /// 撮影が済んでから次の操作で失敗: 撮った画像の記録を捨てない
    #[test]
    fn failure_keeps_earlier_screenshots() {
        let (world, p) = notepad_with_chrome_corner();
        let s = script(
            r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":4,"h":3}},{"type":"click","x":550,"y":50}"#,
        );
        let (r, images) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.action_index, Some(1));
        let shots = progress(&e)["screenshots"].as_array().unwrap();
        assert_eq!(shots.len(), 1);
        assert_eq!(shots[0]["action_index"], 0);
        assert_eq!(images.written.len(), 1);
    }

    /// 実行器が操作に入る前の失敗 (検証) には進み具合を付けない
    #[test]
    fn pre_run_failure_has_no_progress() {
        let world = World::new().shared();
        let (r, _) = exec(
            &world,
            &Policy::closed(),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::PolicyDenied);
        assert!(e.progress.is_none());
        assert!(e.to_json().get("executed").is_none());
    }

    fn detail(e: &AppError) -> &serde_json::Value {
        e.detail.as_deref().expect("detail")
    }

    /// detail に題名を入れない (D-065)
    fn assert_no_title(e: &AppError) {
        let s = serde_json::to_string(detail(e)).unwrap();
        assert!(!s.contains("title"), "{s}");
    }

    /// マウスのガードの 7: 届く先のウィンドウと点 (D-065)
    #[test]
    fn mouse_guard_detail_has_window_and_point() {
        let (world, p) = notepad_with_chrome_corner();
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":550,"y":50}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        let d = detail(&e);
        assert_eq!(d["window"]["process"], "chrome.exe");
        assert_eq!(d["window"]["class"], "Chrome_WidgetWin_1");
        assert_eq!(d["window"]["pid"], 2);
        assert_eq!(d["window"]["hwnd"], 0x11);
        assert_eq!(d["point"], serde_json::json!({"x": 550, "y": 50}));
        assert_no_title(&e);
        // 出力の JSON の error.detail に出る
        assert_eq!(e.to_json()["error"]["detail"]["point"]["x"], 550);
    }

    /// OS のゴーストの窓 (持ち主の読みは空、権限は読めたことにする) と、deny だけの設定 (ゴーストは deny に当たらない)
    fn ghost_under_point_world() -> (Rc<RefCell<World>>, Policy) {
        let mut ghost = window("", "Ghost");
        ghost.hwnd = 0x50;
        ghost.pid = 2024;
        let mut w = World::new();
        w.default_window = Some(top(0x20, "notepad.exe", "a.txt").info);
        w.windows_at = vec![(Rect::new(500, 0, 100, 100), ghost.clone())];
        w.foreground = Some(ghost);
        let p = policy(&format!("{ENABLED}[deny]\nprocesses=[\"zed.exe\"]\n"));
        (w.shared(), p)
    }

    /// ゴーストの窓の上の点には、権限が読めて deny に当たらなくても送らない (D-119)
    #[test]
    fn mouse_guard_refuses_ghost_window_even_when_not_denied() {
        let (world, p) = ghost_under_point_world();
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":550,"y":50}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(e.message.contains("ゴーストの窓なので送らない"), "{e}");
        assert_eq!(detail(&e)["window"]["class"], "Ghost");
        assert!(world.borrow().sent.is_empty(), "何も送らない");
        // 対照: 同じ点の窓がゴーストでなければ (deny に当たらず、権限も読める) 通る
        let mut plain = window("", "Notepad");
        plain.hwnd = 0x50;
        plain.pid = 2024;
        world.borrow_mut().windows_at = vec![(Rect::new(500, 0, 100, 100), plain)];
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":550,"y":50}"#),
        );
        assert!(r.is_ok(), "{r:?}");
    }

    /// 前面がゴーストの窓なら、キーと文字も送らない (クラス名の大文字小文字は区別しない。D-119)
    #[test]
    fn keyboard_guard_refuses_ghost_foreground() {
        let (world, p) = ghost_under_point_world();
        world.borrow_mut().foreground.as_mut().unwrap().class = "GHOST".into();
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(e.message.contains("ゴーストの窓なので送らない"), "{e}");
        assert!(world.borrow().sent.is_empty(), "何も送らない");
    }

    /// キーのガードの 7: 前面のウィンドウ (点は無い)
    #[test]
    fn keyboard_guard_detail_has_foreground_window() {
        let (world, p) = notepad_with_chrome_corner();
        world.borrow_mut().foreground = Some(chrome());
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(detail(&e)["window"]["process"], "chrome.exe");
        assert!(detail(&e).get("point").is_none());
        assert_no_title(&e);
    }

    /// 届く先が無い 7: 画面の外の点、下にウィンドウが無い点、前面が無いキー
    #[test]
    fn guard_detail_window_null_cases() {
        // 点の無い scroll はカーソルの位置に届く。カーソルが画面の外
        let mut w = World::new();
        w.cursor = Point::new(5000, 100);
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"scroll","clicks":1}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(e.message.contains("の外"), "{}", e.message);
        assert_eq!(
            *detail(&e),
            serde_json::json!({"point": {"x": 5000, "y": 100}, "window": null})
        );
        // 下にウィンドウが無い
        let mut w = World::new();
        w.default_window = None;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":20}"#),
        );
        let e = r.unwrap_err();
        assert!(e.message.contains("ウィンドウが無い"), "{}", e.message);
        assert_eq!(
            *detail(&e),
            serde_json::json!({"point": {"x": 10, "y": 20}, "window": null})
        );
        // 前面が無いキー
        let mut w = World::new();
        w.foreground = None;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(*detail(&e), serde_json::json!({"window": null}));
    }

    /// focus の対象が断られた (7): 前面にする対象のウィンドウ。設定の外の窓は名指しの候補に入らない (D-139) ので、
    /// 見つけた窓で断られる管理者権限で組む
    #[test]
    fn focus_denied_detail_has_window() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let mut target = top(0x20, "notepad.exe", "a");
        target.info.elevated = Some(true);
        w.top = vec![target];
        let world = w.shared();
        let p = policy(ENABLED);
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"focus","process":"notepad.exe"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(detail(&e)["window"]["process"], "notepad.exe");
        assert_eq!(detail(&e)["window"]["hwnd"], 0x20);
        assert_no_title(&e);
    }

    /// close の対象が断られた (7): 閉じる対象のウィンドウ。設定の外の窓は名指しの候補に入らない (D-139) ので、
    /// 見つけた窓で断られる管理者権限で組む
    #[test]
    fn close_denied_detail_has_window() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let mut target = top(0x20, "notepad.exe", "a.txt");
        target.info.elevated = Some(true);
        w.top = vec![target];
        let world = w.shared();
        let p = policy(ENABLED);
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(detail(&e)["window"]["process"], "notepad.exe");
        assert_eq!(detail(&e)["window"]["class"], "TargetClass");
        assert_no_title(&e);
    }

    /// deny のクラスに当たる窓 (検証の名前の判定は通る。D-139 の述語だけが撃つ)
    fn class_denied_policy() -> Policy {
        policy(&format!("{ENABLED}[deny]\nclasses=[\"TargetClass\"]\n"))
    }

    /// 窓が 1 つも無い世界で `s` を走らせた失敗 (名指しの失敗の比べ先。D-139)
    fn absent_err(p: &Policy, s: &Script) -> AppError {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let (r, _) = exec(&w.shared(), p, &mut StopSet::new(), s);
        r.unwrap_err()
    }

    /// 名指しの失敗 `e` が窓が無いときの失敗 `absent` と同じ (kind、reason、message、hint、detail) で、message が
    /// 範囲の文で終わる (D-139)
    fn assert_like_absent(e: &AppError, absent: &AppError) {
        assert_eq!(e.kind, absent.kind, "{e}");
        assert_eq!(e.reason, absent.reason);
        assert_eq!(e.message, absent.message);
        assert_eq!(e.hint, absent.hint);
        assert_eq!(e.detail, absent.detail);
        assert!(
            e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
            "{}",
            e.message
        );
    }

    /// focus (D-139): deny のクラスに当たる窓は名指しの候補に入らず、窓が無いときと同じ 11 の not_found で、
    /// SetForegroundWindow を呼ばない
    #[test]
    fn focus_class_denied_window_is_not_found() {
        let s = script(r#"{"type":"focus","process":"notepad.exe","title":"a.txt"}"#);
        let p = class_denied_policy();
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed, "{e}");
        assert_eq!(e.reason, Some(reason::NOT_FOUND));
        assert_eq!(e.hint.as_deref(), Some(FOCUS_NOT_FOUND_HINT));
        assert_like_absent(&e, &absent_err(&p, &s));
        assert!(world.borrow().focus_calls.is_empty());
    }

    /// focus の取り直し (`still_there`。D-139): 断られた後、題名の一致する許可の外の窓が前に出てきても、それを
    /// 「対象が閉じられた」の根拠にしない (closed にならず、前面のプロセスで分けた refused_* になる)
    #[test]
    fn focus_recheck_ignores_denied_window() {
        let s = script(r#"{"type":"focus","process":"notepad.exe","title":"a.txt"}"#);
        let p = policy(&format!("{ENABLED}[deny]\nclasses=[\"SecretClass\"]\n"));
        let target = top(0x20, "notepad.exe", "a.txt");
        let mut denied = top(0x30, "notepad.exe", "a.txt");
        denied.info.class = "SecretClass".into();
        let mut w = World::new();
        w.foreground = Some(chrome());
        w.top = vec![target.clone()];
        w.focus_accepts = false;
        // 2 回目の探索 (取り直し) から、許可の外の窓が前に出る
        w.top_after_finds = Some((1, vec![denied, target]));
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::FocusFailed, "{e}");
        assert_eq!(e.reason, Some(reason::REFUSED_OTHER_APP), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(FOCUS_REFUSED_HINT));
        let w = world.borrow();
        assert_eq!(w.focus_calls, vec![0x20]);
        assert!(w.find_window_calls >= 2, "取り直しで探した");
    }

    /// close (D-139): deny のクラスに当たる窓は名指しの候補に入らず、窓が無いときと同じ 12 で、WM_CLOSE を置かない
    #[test]
    fn close_class_denied_window_is_not_found() {
        let s = script(r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#);
        let p = class_denied_policy();
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        let world = w.shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CLOSE_NOT_FOUND_HINT));
        assert_like_absent(&e, &absent_err(&p, &s));
        assert!(world.borrow().close_calls.is_empty());
    }

    /// wait_for の absent (D-139): deny のクラスに当たる窓しか無ければ最初の確認で満たされ、窓を出さず、note は
    /// 窓が無いときと同じ文 (許可の外を無いと数えることを書いた文)
    #[test]
    fn wait_for_absent_counts_denied_window_as_absent() {
        let s = script(&wait_json(r#","absent":true"#, 1000));
        let p = class_denied_policy();
        let world = wait_world().shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let report = r.unwrap();
        let wi = &report.waited[0];
        assert_eq!(wi.checks, 1);
        assert!(wi.window.is_none());
        let n = waited_note(&report).expect("note");
        assert!(
            n.contains("許可の外と ai-cu を起動した側の窓は無いと数える"),
            "{n}"
        );
        let mut w = World::new();
        w.foreground = Some(terminal());
        let (r, _) = exec(&w.shared(), &p, &mut StopSet::new(), &s);
        assert_eq!(waited_note(&r.unwrap()).as_deref(), Some(n.as_str()));
        assert_eq!(world.borrow().ui_element_calls, 0);
    }

    /// wait_for の present (D-139 の限界): deny のクラスに当たる窓を待つと、即時の 4 ではなく時間切れまで待って
    /// 15 で、message と detail と hint は窓が無いときと同じ (message は範囲の文で終わる)
    #[test]
    fn wait_for_present_on_denied_window_times_out_like_absent() {
        let s = script(&wait_json("", 300));
        let p = class_denied_policy();
        let world = wait_world().shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        let d = wait_timeout(&e).clone();
        assert_eq!(d["unmet"], "window");
        assert!(d["window"].is_null(), "{d}");
        assert_like_absent(&e, &absent_err(&p, &s));
        assert_eq!(world.borrow().now_ms, 300, "時間切れまで待つ");
    }

    /// 利用者のマウスでの停止 (6): 読んだカーソルと、直前の移動の線分
    #[test]
    fn user_mouse_stop_detail_has_cursor() {
        let mut w = World::new();
        w.user_moves.push((0, Point::new(1500, 900)));
        let world = w.shared();
        let s = script(r#"{"type":"move","x":10,"y":10},{"type":"move","x":20,"y":20}"#);
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        let d = detail(&e);
        assert_eq!(d["cursor"], serde_json::json!({"x": 1500, "y": 900}));
        assert_eq!(d["expected"], serde_json::json!({"x": 10, "y": 10}));
        // from は直前の移動の始点 (モックの初期カーソル (100,100))。expected やカーソルとの取り違えを落とす
        assert_eq!(d["from"], serde_json::json!({"x": 100, "y": 100}));
        // distance_px は線分 from → expected からカーソルまでの距離を丸めた値 (切り捨てではない)
        let want = Point::new(1500, 900)
            .distance_to_segment(Point::new(100, 100), Point::new(10, 10))
            .round() as i64;
        assert_eq!(d["distance_px"].as_i64(), Some(want), "{d}");
        assert!(
            want > i64::from(policy(ENABLED).mouse_threshold_px()),
            "{d}"
        );
    }

    #[test]
    fn run_type_sends_unicode_pairs() {
        let world = World::new().shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"日本"}"#),
        );
        r.unwrap();
        let ev = world.borrow().all_events();
        assert_eq!(ev.len(), 4);
        assert_eq!(
            ev[0],
            InputEvent::Unicode {
                unit: '日' as u16,
                down: true
            }
        );
    }

    // --- パスワード欄への入力 (D-090) ---

    const ALLOW_TARGET: &str = "[allow]\nprocesses=[\"target\"]\n";

    fn pw_world(focused: Result<Option<bool>, BackendError>) -> Rc<RefCell<World>> {
        let mut w = World::new();
        w.focused_password = focused;
        w.shared()
    }

    /// 既定の設定 (password_input 偽) でフォーカスがパスワード欄なら、type も key も 4 で何も送らない
    #[test]
    fn password_focus_refuses_type_and_key_by_default() {
        for action in [
            r#"{"type":"type","text":"ab"}"#,
            r#"{"type":"key","keys":"enter"}"#,
        ] {
            let world = pw_world(Ok(Some(true)));
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(action),
            );
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::PolicyDenied, "{action}: {e}");
            assert_eq!(
                e.deny.as_ref().map(|d| d.rule),
                Some("general.password_input")
            );
            assert!(
                e.deny.as_ref().and_then(|d| d.fix.as_deref()).is_some(),
                "直し方がある"
            );
            assert_eq!(
                e.hint.as_deref(),
                Some(hint(Hint::PasswordField, Surface::Cli))
            );
            assert_eq!(detail(&e)["window"]["hwnd"], 0x1000);
            assert_eq!(detail(&e)["window"]["process"], "target.exe");
            assert_no_title(&e);
            assert_eq!(progress(&e)["executed"], 0);
            assert_eq!(progress(&e)["partial"], false);
            let w = world.borrow();
            assert!(w.sent.is_empty(), "{action}: {:?}", w.sent);
            assert_eq!(w.focused_password_calls, 1);
        }
    }

    /// 全層が password_input 真なら、フォーカスの要素を読まずに送る
    #[test]
    fn password_focus_allowed_by_config_does_not_read_uia() {
        let world = pw_world(Ok(Some(true)));
        let p = policy("[general]\ninput_enabled = true\npassword_input = true\n");
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"ab"},{"type":"key","keys":"enter"}"#),
        );
        r.unwrap();
        let w = world.borrow();
        assert!(!w.sent.is_empty());
        assert_eq!(w.focused_password_calls, 0);
    }

    /// 読めない (`Ok(None)`、時間切れでない `Err`) かパスワード欄でなければ送る (fail-open)
    #[test]
    fn password_focus_unknown_or_error_passes() {
        for focused in [
            Ok(None),
            Ok(Some(false)),
            Err(BackendError::Os("UIA の失敗".into())),
        ] {
            let world = pw_world(focused.clone());
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(r#"{"type":"type","text":"ab"}"#),
            );
            r.unwrap_or_else(|e| panic!("{focused:?}: {e}"));
            let w = world.borrow();
            assert_eq!(w.sent.len(), 2, "{focused:?}");
            assert_eq!(w.focused_password_calls, 1, "{focused:?}");
        }
    }

    /// 判定の時間切れ (`TimedOut`) は、type も key も何も送らずに 8 で断る (D-104、本人の決定)
    #[test]
    fn password_focus_timeout_refuses_with_os_error() {
        for action in [
            r#"{"type":"type","text":"ab"}"#,
            r#"{"type":"key","keys":"enter"}"#,
        ] {
            let world = pw_world(Err(BackendError::TimedOut("時間切れ".into())));
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(action),
            );
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::OsError, "{action}: {e}");
            assert!(e.message.contains("時間切れ"), "{action}: {}", e.message);
            assert!(
                e.message.contains("パスワード欄かの判定") && e.message.contains("送らずに断った"),
                "{action}: {}",
                e.message
            );
            let w = world.borrow();
            assert!(w.sent.is_empty(), "{action}: {:?}", w.sent);
            assert_eq!(w.focused_password_calls, 1, "{action}");
        }
    }

    /// 全層が password_input 真なら読まないので、判定が時間切れになる状況でも送る (D-104)
    #[test]
    fn password_focus_allowed_by_config_sends_even_if_timeout() {
        let world = pw_world(Err(BackendError::TimedOut("時間切れ".into())));
        let p = policy("[general]\ninput_enabled = true\npassword_input = true\n");
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"ab"}"#),
        );
        r.unwrap();
        let w = world.borrow();
        assert_eq!(w.sent.len(), 2);
        assert_eq!(w.focused_password_calls, 0);
    }

    /// 1 つの操作の中では最初のキーの束で 1 回だけ読み、操作が変われば読み直す
    #[test]
    fn password_focus_read_once_per_action() {
        let world = pw_world(Ok(Some(false)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"abc","interval_ms":10}"#),
        );
        r.unwrap();
        assert_eq!(world.borrow().sent.len(), 3, "束は 3 つ");
        assert_eq!(world.borrow().focused_password_calls, 1);

        let world = pw_world(Ok(Some(false)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(
                r#"{"type":"type","text":"ab"},{"type":"click","x":10,"y":10},{"type":"type","text":"cd"}"#,
            ),
        );
        r.unwrap();
        assert_eq!(world.borrow().focused_password_calls, 2);
    }

    /// type の途中の Tab と改行はフォーカスを動かしうるので、その次のキーの束で読み直す (D-090)。
    /// Tab の後でパスワード欄に入ったら、それ以降は送らない
    #[test]
    fn password_focus_rechecked_after_tab_and_enter_in_type() {
        let world = pw_world(Ok(Some(false)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"a\tb\nc"}"#),
        );
        r.unwrap();
        assert_eq!(world.borrow().sent.len(), 5, "束は 5 つ");
        // 最初の束、Tab の次、改行の次
        assert_eq!(world.borrow().focused_password_calls, 3);

        // Tab を送った後にフォーカスがパスワード欄に移る: "a" と Tab は送り、"b" から先は 4 で送らない
        let world = pw_world(Ok(Some(false)));
        world.borrow_mut().password_after_calls = Some((1, Ok(Some(true))));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"a\tb"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::PolicyDenied);
        assert_eq!(
            e.deny.as_ref().map(|d| d.rule),
            Some("general.password_input")
        );
        assert_eq!(world.borrow().sent.len(), 2, "a と Tab だけ");
    }

    /// type の途中 (Tab の後) にパスワード欄で断ったとき、前置きの「途中まで行った」と、続く
    /// PasswordField の「何も送っていない」が食い違わない: 前置きは「止まった所から先について」と結ぶ
    #[test]
    fn password_hint_after_partial_type_reads_consistently() {
        let world = pw_world(Ok(Some(false)));
        world.borrow_mut().password_after_calls = Some((1, Ok(Some(true))));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"a\tb"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::PolicyDenied);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], true);
        assert_eq!(world.borrow().sent.len(), 2, "a と Tab だけ");
        let h = e.hint.as_deref().unwrap();
        assert!(
            h.starts_with("actions[0] も途中まで行った (partial=true)。"),
            "{h}"
        );
        assert!(!h.contains("以下は失敗した"), "{h}");
        assert!(
            h.ends_with(&format!(
                "以下は actions[0] の止まった所から先について (それより前に起こしたことはそのまま): {}",
                hint(Hint::PasswordField, Surface::Cli)
            )),
            "{h}"
        );
    }

    /// Tab と Enter の押下だけを、フォーカスを動かしうる束とみなす (D-090)
    #[test]
    fn moves_focus_matches_tab_and_enter_down_only() {
        use crate::device::InputEvent;
        let key = |vk, down| InputEvent::Key {
            vk,
            down,
            extended: false,
        };
        assert!(moves_focus(&[key(0x09, true), key(0x09, false)]));
        assert!(moves_focus(&[key(0x0D, true), key(0x0D, false)]));
        assert!(!moves_focus(&[key(0x41, true), key(0x41, false)]));
        assert!(!moves_focus(&[key(0x09, false)]));
        assert!(!moves_focus(&[InputEvent::Unicode {
            unit: 0x09,
            down: true
        }]));
    }

    /// マウスの操作ではフォーカスの要素を読まない
    #[test]
    fn password_focus_not_read_for_mouse_actions() {
        let world = pw_world(Ok(Some(true)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(
                r#"{"type":"click","x":10,"y":10},{"type":"scroll","x":10,"y":10,"clicks":-1}"#,
            ),
        );
        r.unwrap();
        let w = world.borrow();
        assert_eq!(w.sent.len(), 2);
        assert_eq!(w.focused_password_calls, 0);
    }

    /// 読んだ後にガードを取り直す: 読む間に前面が許可されていない窓になれば、既存のガードの 7 で何も送らない
    #[test]
    fn password_focus_regards_after_read() {
        let world = pw_world(Ok(Some(false)));
        world.borrow_mut().foreground_after_password_read = Some(chrome());
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"ab"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(!e.message.contains("前面のウィンドウが変わった"), "{e}");
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::KeyboardGuard, Surface::Cli))
        );
        assert_eq!(detail(&e)["window"]["process"], "chrome.exe");
        let w = world.borrow();
        assert!(w.sent.is_empty(), "{:?}", w.sent);
        assert_eq!(w.focused_password_calls, 1);
    }

    /// 読む間に前面が許可された別の窓に変われば、読み直さずに 7 で断る (古い判定で送らない)
    #[test]
    fn password_focus_refuses_when_foreground_changed_during_read() {
        let world = pw_world(Ok(Some(false)));
        let mut moved = window("target.exe", "TargetClass");
        moved.hwnd = 0x2000;
        world.borrow_mut().foreground_after_password_read = Some(moved);
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"ab"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert!(e.message.contains("前面のウィンドウが変わった"), "{e}");
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::ForegroundChanged, Surface::Cli))
        );
        assert_eq!(detail(&e)["window"]["hwnd"], 0x2000);
        assert_no_title(&e);
        let w = world.borrow();
        assert!(w.sent.is_empty(), "{:?}", w.sent);
        assert_eq!(w.focused_password_calls, 1, "読み直さない");
    }

    // ---- ドラッグの途中でガードに断られたときの次の手 (D-132) ----

    /// (10, 10) から (200, 10) へ 4 歩: 補間の点の x は 58、105、153、200
    const DRAG_4: &str = r#"{"type":"drag","from":{"x":10,"y":10},"to":{"x":200,"y":10},"steps":4,"duration_ms":40}"#;

    /// x が 100 以上の場所の下が `at` の窓になる world (2 点目の x=105 から許可外)
    fn drag_world(at: WindowInfo) -> Rc<RefCell<World>> {
        let mut w = World::new();
        w.windows_at = vec![(Rect::new(100, 0, 3000, 2000), at)];
        w.shared()
    }

    fn is_button(events: &[InputEvent], down: bool) -> bool {
        matches!(events, [InputEvent::Button { down: d, .. }] if *d == down)
    }

    #[test]
    fn drag_refused_midway_says_partial_and_how_to_rebuild() {
        let world = drag_world(chrome());
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(DRAG_4),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.reason, None);
        let h = e.hint.as_deref().unwrap();
        assert!(h.contains("partial=true"), "{h}");
        assert!(
            h.ends_with(hint(Hint::DragGuardMidway, Surface::Cli)),
            "{h}"
        );
        assert_eq!(detail(&e)["point"]["x"], 105);
        assert_eq!(detail(&e)["point"]["y"], 10);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], true);
        let w = world.borrow();
        assert!(
            !w.sent
                .iter()
                .any(|b| b.contains(&InputEvent::MoveTo { x: 105, y: 10 })),
            "断った点は送らない: {:?}",
            w.sent
        );
        assert!(
            w.sent.iter().any(|b| is_button(b, true)),
            "押してある: {:?}",
            w.sent
        );
        assert!(
            is_button(w.sent.last().unwrap(), false),
            "押したボタンは離す: {:?}",
            w.sent
        );
    }

    /// 始点で断られたら何も送っていないので、個々の hint のまま (前置きも付かない)
    #[test]
    fn drag_refused_at_start_keeps_mouse_guard_hint() {
        let mut w = World::new();
        w.windows_at = vec![(Rect::new(0, 0, 3000, 2000), chrome())];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(DRAG_4),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::MouseGuard, Surface::Cli))
        );
        assert!(world.borrow().sent.is_empty());
    }

    /// 始点への移動は送ったが押す前に断られた: partial でもボタンは押していないのでドラッグの途中の文にしない
    #[test]
    fn drag_refused_before_press_keeps_mouse_guard_hint() {
        let mut w = World::new();
        w.windows_at = vec![(Rect::new(0, 0, 3000, 2000), chrome())];
        // 移動のガード (t = 0) の後、DRAG_SETTLE_MS の Sleep の間に点の下が変わる
        w.windows_at_from_ms = Some(1);
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(DRAG_4),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        let h = e.hint.as_deref().unwrap();
        assert!(h.contains("partial=true"), "{h}");
        assert!(h.ends_with(hint(Hint::MouseGuard, Surface::Cli)), "{h}");
        let w = world.borrow();
        assert_eq!(w.sent.len(), 1, "始点への移動だけ: {:?}", w.sent);
        assert!(
            !w.sent.iter().any(|b| is_button(b, true)),
            "押していない: {:?}",
            w.sent
        );
    }

    /// 途中の点の下がホストの窓なら、個々の hint (focus の案内) でなくドラッグの途中の文を呼ばれ方で出す
    #[test]
    fn drag_midway_over_host_window_uses_drag_hint() {
        for surface in [Surface::Cli, Surface::Mcp] {
            let world = drag_world(terminal());
            let e = exec_on(
                &world,
                &mut StopSet::new(),
                &script(DRAG_4),
                &[terminal().pid],
                surface,
            )
            .unwrap_err();
            assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
            let h = e.hint.as_deref().unwrap();
            assert!(h.ends_with(hint(Hint::DragGuardMidway, surface)), "{h}");
        }
    }

    /// 補間の点は始点と終点の間にあり、validate を通った drag では途中の点が画面の外にならないので、純関数で見る
    #[test]
    fn drag_midway_off_screen_uses_drag_hint() {
        let mouse = Target::Mouse {
            points: vec![Point::new(5000, 10)],
        };
        for surface in [Surface::Cli, Surface::Mcp] {
            let e = AppError::new(ErrorKind::ForegroundGuard, "画面の外");
            let e = mid_drag_hint(e, true, &mouse, surface);
            assert_eq!(
                e.hint.as_deref(),
                Some(hint(Hint::DragGuardMidway, surface))
            );
        }
    }

    #[test]
    fn mid_drag_hint_leaves_keyboard_and_policy_denied_alone() {
        let mouse = Target::Mouse { points: vec![] };
        let e = AppError::new(ErrorKind::ForegroundGuard, "前面").with_hint("元の文");
        let e = mid_drag_hint(e, true, &Target::Keyboard, Surface::Cli);
        assert_eq!(e.hint.as_deref(), Some("元の文"));
        let e = AppError::new(ErrorKind::PolicyDenied, "領域");
        let e = mid_drag_hint(e, true, &mouse, Surface::Cli);
        assert_eq!(e.hint, None);
    }

    const ALLOW_TARGET_AND_NOTEPAD: &str = "[allow]\nprocesses=[\"target\",\"notepad\"]\n";

    /// type の途中で前面が許可された別のプロセスに移れば、残りを送らずに 7 (target_changed) で止める (D-100)
    #[test]
    fn type_stops_when_foreground_moves_to_another_allowed_process() {
        let world = World::new().shared();
        world
            .borrow_mut()
            .foreground_after_sends
            .push((0, other("notepad.exe", 0x3000, 77)));
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET_AND_NOTEPAD}")),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"abc","interval_ms":0}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.reason, Some(reason::TARGET_CHANGED));
        // 途中まで送ったので、with_progress が「操作ごと打ち直さない」の前置きを足す
        let h = e.hint.as_deref().unwrap();
        assert!(h.contains("partial=true"), "{h}");
        assert!(h.ends_with(hint(Hint::TargetChanged, Surface::Cli)), "{h}");
        assert_eq!(detail(&e)["window"]["pid"], 77);
        assert_eq!(detail(&e)["window"]["process"], "notepad.exe");
        assert_eq!(detail(&e)["pinned_pid"], 4242);
        assert_no_title(&e);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], true, "最初の文字は送ってある");
        assert_eq!(world.borrow().sent.len(), 1, "残りは送らない");
    }

    /// 同じプロセスの別の窓 (type の Enter で開いたダイアログなど) への移りは止めない (D-100)
    #[test]
    fn type_continues_when_foreground_moves_within_same_process() {
        let world = World::new().shared();
        let mut dialog = window("target.exe", "#32770");
        dialog.hwnd = 0x2000;
        world.borrow_mut().foreground_after_sends.push((0, dialog));
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"abc","interval_ms":0}"#),
        );
        r.unwrap();
        assert_eq!(world.borrow().sent.len(), 3);
    }

    /// 固定は操作ごと: 次の操作は新しい前面 (許可されていれば) に送る (D-100)
    #[test]
    fn foreground_pin_resets_for_each_action() {
        let world = World::new().shared();
        world
            .borrow_mut()
            .foreground_after_sends
            .push((0, other("notepad.exe", 0x3000, 77)));
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET_AND_NOTEPAD}")),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"a"},{"type":"type","text":"b"}"#),
        );
        r.unwrap();
        assert_eq!(world.borrow().sent.len(), 2);
    }

    /// ガード → 読む の順: 前面が許可されていなければ、読まずにガードの 7
    #[test]
    fn password_focus_not_read_when_foreground_not_allowed() {
        let world = pw_world(Ok(Some(true)));
        world.borrow_mut().foreground = Some(chrome());
        let (r, _) = exec(
            &world,
            &policy(&format!("{ENABLED}{ALLOW_TARGET}")),
            &mut StopSet::new(),
            &script(r#"{"type":"type","text":"ab"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::KeyboardGuard, Surface::Cli))
        );
        let w = world.borrow();
        assert!(w.sent.is_empty());
        assert_eq!(w.focused_password_calls, 0);
    }

    /// 読んだ後に停止を確かめ直す: 手順の頭の check_stop の次 (読んだ後) で止まれば何も送らない
    #[test]
    fn password_focus_rechecks_stop_after_read() {
        let world = pw_world(Ok(Some(false)));
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(1)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut stops,
            &script(r#"{"type":"key","keys":"a"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        let w = world.borrow();
        assert!(w.sent.is_empty(), "{:?}", w.sent);
        assert_eq!(w.focused_password_calls, 1);
    }

    // --- 操作に入る前の読み取りの判定 (D-066) ---

    const SHOT: &str = r#"{"type":"screenshot","path":"a.png","region":{"x":0,"y":0,"w":4,"h":3}}"#;
    const CLOSE_A: &str = r#"{"type":"close","process":"notepad.exe","title":"a.txt"}"#;

    fn winlogon() -> InputDesktop {
        InputDesktop::Other {
            name: Some("Winlogon".into()),
        }
    }

    fn cursor_locked() -> BackendError {
        BackendError::DesktopUnavailable("GetCursorPos が失敗 (GetLastError=5)".into())
    }

    /// 閉じる対象のメモ帳がある world。前面は無い (ロック中は GetForegroundWindow が null)
    fn close_world() -> World {
        let mut w = World::new();
        w.foreground = None;
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w
    }

    /// 何も送っていない 14: 進み具合が無く、何にも触れていない
    fn assert_desktop_refused(world: &Rc<RefCell<World>>, e: &AppError, why: &str) {
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{e}");
        assert_eq!(e.kind.exit_code(), 14);
        assert_eq!(e.reason, Some(why));
        assert_eq!(e.action_index, None);
        assert!(e.progress.is_none(), "{e}");
        assert!(e.to_json().get("executed").is_none());
        let w = world.borrow();
        assert!(w.sent.is_empty(), "{:?}", w.sent);
        assert!(w.focus_calls.is_empty());
        assert!(w.close_calls.is_empty());
        assert_eq!(w.keys_held_calls, 0, "14 の後は keys_held を見ない");
    }

    /// セーバー中の click は何も送らず 14 `screensaver`
    #[test]
    fn preflight_screensaver_refuses_click_with_14() {
        let mut w = World::new();
        w.input_desktop = Ok(InputDesktop::ScreenSaver {
            name: Some("Screen-saver".into()),
        });
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10}"#),
        );
        let e = r.unwrap_err();
        assert_desktop_refused(&world, &e, reason::SCREENSAVER);
        assert_eq!(
            *detail(&e),
            serde_json::json!({"input_desktop": "Screen-saver"})
        );
        assert_eq!(world.borrow().cursor_reads, 0, "カーソルより先に断る");
    }

    /// 別のデスクトップ (Winlogon) では、key、focus だけ、screenshot だけの操作列も 14 `other_desktop`
    #[test]
    fn preflight_other_desktop_refuses_key_focus_and_screenshot_with_14() {
        for actions in [
            r#"{"type":"key","keys":"a"}"#,
            r#"{"type":"focus","process":"notepad.exe"}"#,
            SHOT,
        ] {
            let mut w = close_world();
            w.input_desktop = Ok(winlogon());
            let world = w.shared();
            let (r, images) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(actions),
            );
            let e = r.unwrap_err();
            assert_desktop_refused(&world, &e, reason::OTHER_DESKTOP);
            assert_eq!(
                *detail(&e),
                serde_json::json!({"input_desktop": "Winlogon"}),
                "{actions}"
            );
            assert!(e.message.contains("Winlogon"), "{}", e.message);
            assert!(images.written.is_empty(), "撮っていない: {actions}");
        }
        // 名前が読めない (開けない) ときは null
        let mut w = World::new();
        w.input_desktop = Ok(InputDesktop::Other { name: None });
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &script(SHOT));
        let e = r.unwrap_err();
        assert_desktop_refused(&world, &e, reason::OTHER_DESKTOP);
        assert_eq!(*detail(&e), serde_json::json!({"input_desktop": null}));
    }

    /// ロック中 (カーソルが 5 で読めない) でも、close だけの操作列は監視なしで閉じる。間に wait があっても同じ
    #[test]
    fn preflight_close_only_runs_unmonitored_while_locked() {
        for actions in [
            CLOSE_A.to_string(),
            format!(r#"{{"type":"wait","ms":200}},{CLOSE_A}"#),
        ] {
            let mut w = close_world();
            w.input_desktop = Ok(winlogon());
            w.cursor_fail_after_reads = Some((0, cursor_locked()));
            let world = w.shared();
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(&actions),
            );
            let report = r.unwrap_or_else(|e| panic!("{actions}: {e}"));
            assert_eq!(report.closed.len(), 1, "{actions}");
            assert!(report.closed[0].gone);
            assert!(!report.closed[0].hidden);
            let w = world.borrow();
            assert_eq!(w.close_calls, vec![0x20]);
            assert!(w.sent.is_empty());
            assert_eq!(w.keys_held_calls, 0, "入力が無いので keys_held を見ない");
        }
    }

    /// close だけの操作列でも、5 以外のカーソルの読みの失敗は今までどおり 8 (監視が黙って外れない)
    #[test]
    fn preflight_close_only_with_other_cursor_failure_is_8() {
        let mut w = close_world();
        w.cursor_fail_after_reads = Some((0, BackendError::Os("GetCursorPos".into())));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLOSE_A),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::OsError, "{e}");
        assert!(world.borrow().close_calls.is_empty());
    }

    /// 監視なしで始めても、カーソルが読めるようになった (人が解除した) 時点から監視が戻り、人の移動で 6
    #[test]
    fn preflight_unmonitored_close_resumes_mouse_watch_after_unlock() {
        let mut w = close_world();
        w.input_desktop = Ok(winlogon());
        // 読み 0 (開始) と 1 (最初の停止の確認) は読めない。2 で読めて監視を始め、3 の前に人が動かす
        w.cursor_fail_until_reads = Some((2, cursor_locked()));
        w.cursor_moves_after_reads = vec![(3, Point::new(1500, 900))];
        let world = w.shared();
        let s = script(&format!(r#"{{"type":"wait","ms":500}},{CLOSE_A}"#));
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.kind.exit_code(), 6);
        assert_eq!(
            detail(&e)["cursor"],
            serde_json::json!({"x": 1500, "y": 900})
        );
        assert_eq!(
            detail(&e)["expected"],
            serde_json::json!({"x": 100, "y": 100})
        );
        assert!(world.borrow().close_calls.is_empty());
    }

    /// 通常の画面では、close だけの操作列にも今までどおりマウスの監視がある
    #[test]
    fn preflight_close_only_keeps_mouse_watch_on_normal_desktop() {
        let mut w = close_world();
        w.cursor_moves_after_reads = vec![(1, Point::new(1500, 900))];
        let world = w.shared();
        let s = script(&format!(r#"{{"type":"wait","ms":200}},{CLOSE_A}"#));
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind.exit_code(), 6, "{e}");
        assert!(world.borrow().close_calls.is_empty());
    }

    /// 押されたままの Shift で click は何も送らず 10 `keys_held`。hint は呼ばれ方ごとの専用の文 (MCP は status を読む。D-073)
    #[test]
    fn preflight_keys_held_refuses_input_with_10() {
        for surface in [Surface::Cli, Surface::Mcp] {
            let mut w = World::new();
            w.held.keys = vec!["shift"];
            let world = w.shared();
            let e = exec_on(
                &world,
                &mut StopSet::new(),
                &script(r#"{"type":"click","x":10,"y":10}"#),
                &[],
                surface,
            )
            .unwrap_err();
            assert_eq!(e.kind, ErrorKind::UserActive, "{e}");
            assert_eq!(e.kind.exit_code(), 10);
            assert_eq!(e.reason, Some(reason::KEYS_HELD));
            assert_eq!(
                *detail(&e),
                serde_json::json!({"keys": ["shift"], "buttons": []})
            );
            assert!(e.message.contains("shift"), "{}", e.message);
            assert!(e.progress.is_none());
            assert!(e.to_json().get("executed").is_none());
            let h = e.hint.as_deref().unwrap();
            assert_eq!(h, hint(Hint::KeysHeld, surface));
            assert!(!h.contains("ai-cu "), "{h}");
            let w = world.borrow();
            assert!(w.sent.is_empty(), "{:?}", w.sent);
            assert_eq!(w.keys_held_calls, 1);
        }
    }

    /// 入力の無い操作列 (screenshot だけ、close だけ) は、ボタンが押されたままでも keys_held を見ない
    #[test]
    fn preflight_keys_held_is_not_checked_without_input() {
        for actions in [SHOT, CLOSE_A] {
            let mut w = close_world();
            w.held.buttons = vec!["left"];
            let world = w.shared();
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(actions),
            );
            r.unwrap_or_else(|e| panic!("{actions}: {e}"));
            assert_eq!(world.borrow().keys_held_calls, 0, "{actions}");
        }
    }

    /// 撮影を付けた close (close の後に screenshot) は撮影を含むので、ロック中は 14 で WM_CLOSE を置かない
    #[test]
    fn preflight_close_with_screenshot_is_refused_while_locked() {
        let mut w = close_world();
        w.input_desktop = Ok(winlogon());
        w.cursor_fail_after_reads = Some((0, cursor_locked()));
        let world = w.shared();
        let (r, images) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&format!("{CLOSE_A},{SHOT}")),
        );
        let e = r.unwrap_err();
        assert_desktop_refused(&world, &e, reason::OTHER_DESKTOP);
        assert!(images.written.is_empty());
    }

    /// keys_held は実行の最初の 1 回だけ (drag の途中で自分の押下に引っかからない)
    #[test]
    fn preflight_keys_held_is_checked_once_per_run() {
        let world = World::new().shared();
        let s = script(
            r#"{"type":"drag","from":{"x":10,"y":10},"to":{"x":200,"y":10},"steps":5,"duration_ms":50},{"type":"key","keys":"ctrl+a"}"#,
        );
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        assert_eq!(r.unwrap().executed, 2);
        assert_eq!(world.borrow().keys_held_calls, 1);
    }

    /// 途中のステップでカーソルが 5 で読めなくなる (実行中のロック) と 14 で、進み具合と実行済みの一文が付く。
    /// hint は実行済みの一文の後に 14 の既定の文
    #[test]
    fn preflight_cursor_lost_mid_run_is_14_with_progress() {
        for surface in [Surface::Cli, Surface::Mcp] {
            let mut w = World::new();
            // 読み 0 (開始) と 1 (1 つ目の click の前) は読め、2 (2 つ目の click の前) から読めない
            w.cursor_fail_after_reads = Some((2, cursor_locked()));
            let world = w.shared();
            let e = exec_on(
                &world,
                &mut StopSet::new(),
                &script(r#"{"type":"click","x":10,"y":10},{"type":"click","x":20,"y":20}"#),
                &[],
                surface,
            )
            .unwrap_err();
            assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{e}");
            assert_eq!(e.reason, Some(reason::OTHER_DESKTOP));
            assert_eq!(e.action_index, Some(1));
            assert_eq!(progress(&e)["executed"], 1);
            assert_eq!(e.to_json()["executed"], 1);
            let h = e.hint.as_deref().unwrap();
            assert!(h.starts_with("actions[0] は実行済み (executed=1)。"), "{h}");
            assert!(
                h.ends_with(&format!(
                    "以下は失敗した actions[1] について: {}",
                    ErrorKind::DesktopUnavailable.hint()
                )),
                "{h}"
            );
            assert!(h.contains("executed が無ければ"), "{h}");
            assert_eq!(world.borrow().sent.len(), 1, "1 つ目の click だけ送った");
        }
    }

    /// 入力デスクトップが読めない (OS の失敗) は 8。判定が読めないのを「ロック」と決めつけない
    #[test]
    fn preflight_input_desktop_read_failure_is_8() {
        let mut w = World::new();
        w.input_desktop = Err(BackendError::Os("OpenInputDesktop".into()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::OsError, "{e}");
        assert!(world.borrow().sent.is_empty());
    }

    /// 判定が漏れても (Available と読めたのにカーソルが 5)、入力の操作列は何も送らず 14 `other_desktop`
    #[test]
    fn preflight_cursor_denied_backs_up_available_desktop() {
        let mut w = World::new();
        w.cursor_fail_after_reads = Some((0, cursor_locked()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{e}");
        assert_eq!(e.reason, Some(reason::OTHER_DESKTOP));
        assert!(e.progress.is_none());
        assert!(e.to_json().get("executed").is_none());
        assert!(world.borrow().sent.is_empty());
    }

    /// 写しの関数 (`shot` も使う): Available は None
    #[test]
    fn input_desktop_error_maps_states() {
        assert!(input_desktop_error(&InputDesktop::Available).is_none());
        let e = input_desktop_error(&InputDesktop::ScreenSaver { name: None }).unwrap();
        assert_eq!(e.kind.exit_code(), 14);
        assert_eq!(e.reason, Some(reason::SCREENSAVER));
        assert_eq!(*detail(&e), serde_json::json!({"input_desktop": null}));
        let e = input_desktop_error(&winlogon()).unwrap();
        assert_eq!(e.reason, Some(reason::OTHER_DESKTOP));
        // WTS のロック (D-067): reason は locked、detail は読めた名前 (幕の間は Default)
        let e = input_desktop_error(&lock_curtain()).unwrap();
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable);
        assert_eq!(e.kind.exit_code(), 14);
        assert_eq!(e.reason, Some(reason::LOCKED));
        assert_eq!(e.reason, Some("locked"));
        assert_eq!(*detail(&e), serde_json::json!({"input_desktop": "Default"}));
        assert_eq!(e.message, "画面がロックされているので何も送らない");
        let e = input_desktop_error(&InputDesktop::Locked { name: None }).unwrap();
        assert_eq!(e.reason, Some(reason::LOCKED));
        assert_eq!(*detail(&e), serde_json::json!({"input_desktop": null}));
    }

    // --- セッションのロック (WTS) と、途中の撮影と focus の直前の判定 (D-067) ---

    /// ロックの直後の幕 (LockApp) の間: WTS ではロック、入力デスクトップの名前は Default のまま
    fn lock_curtain() -> InputDesktop {
        InputDesktop::Locked {
            name: Some("Default".into()),
        }
    }

    /// ロック中の click、focus だけ、screenshot だけの操作列は、何も送らず 14 `locked` (D-067)
    #[test]
    fn preflight_session_lock_refuses_with_14_locked() {
        for actions in [
            r#"{"type":"click","x":10,"y":10}"#,
            r#"{"type":"focus","process":"notepad.exe"}"#,
            SHOT,
        ] {
            let mut w = close_world();
            w.input_desktop = Ok(lock_curtain());
            let world = w.shared();
            let (r, images) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(actions),
            );
            let e = r.unwrap_err();
            assert_desktop_refused(&world, &e, reason::LOCKED);
            assert_eq!(
                *detail(&e),
                serde_json::json!({"input_desktop": "Default"}),
                "{actions}"
            );
            assert!(images.written.is_empty(), "撮っていない: {actions}");
        }
    }

    /// 途中の screenshot の直前にロック、セーバー、読めない判定になると、撮らずに 14 `locked`、
    /// 14 `screensaver`、8 で止まり、click は済んでいるので進み具合 (executed 1) が付く
    #[test]
    fn mid_run_screenshot_is_refused_when_desktop_becomes_unavailable() {
        let cases: [(Result<InputDesktop, BackendError>, ErrorKind, Option<&str>); 3] = [
            (
                Ok(lock_curtain()),
                ErrorKind::DesktopUnavailable,
                Some(reason::LOCKED),
            ),
            (
                Ok(InputDesktop::ScreenSaver {
                    name: Some("Screen-saver".into()),
                }),
                ErrorKind::DesktopUnavailable,
                Some(reason::SCREENSAVER),
            ),
            (
                Err(BackendError::Os("OpenInputDesktop".into())),
                ErrorKind::OsError,
                None,
            ),
        ];
        for (later, kind, why) in cases {
            let mut w = World::new();
            // 読み 0 (開始時) は使える。読み 1 (screenshot の直前) から変わる
            w.input_desktop_after_reads = Some((1, later.clone()));
            let world = w.shared();
            let (r, images) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(&format!(r#"{{"type":"click","x":10,"y":10}},{SHOT}"#)),
            );
            let e = r.unwrap_err();
            assert_eq!(e.kind, kind, "{later:?}: {e}");
            assert_eq!(e.reason, why, "{later:?}");
            assert_eq!(e.action_index, Some(1), "{later:?}");
            assert_eq!(progress(&e)["executed"], 1, "{later:?}");
            assert_eq!(e.to_json()["executed"], 1, "{later:?}");
            let h = e.hint.as_deref().unwrap();
            assert!(h.starts_with("actions[0] は実行済み (executed=1)。"), "{h}");
            assert!(images.written.is_empty(), "撮っていない: {later:?}");
            let w = world.borrow();
            assert_eq!(w.input_desktop_reads, 2, "開始時と撮影の直前");
            assert!(!w.sent.is_empty(), "click は送った");
        }
    }

    /// 途中の focus の直前にロックになると、前面化を呼ばずに 14 `locked` (以前は 11 になっていた)
    #[test]
    fn mid_run_focus_is_refused_with_14_when_session_locks() {
        let mut w = World::new();
        w.top = vec![top(0x20, "notepad.exe", "a.txt")];
        w.input_desktop_after_reads = Some((1, Ok(lock_curtain())));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"click","x":10,"y":10},{"type":"focus","process":"notepad.exe"}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{e}");
        assert_eq!(e.reason, Some(reason::LOCKED));
        assert_eq!(*detail(&e), serde_json::json!({"input_desktop": "Default"}));
        assert_eq!(e.action_index, Some(1));
        assert_eq!(progress(&e)["executed"], 1);
        let w = world.borrow();
        assert!(w.focus_calls.is_empty(), "前面化を呼ばない");
        assert!(w.helper_calls.is_empty());
        assert_eq!(w.input_desktop_reads, 2, "開始時と focus の直前");
    }

    // --- 条件がそろうまで待つ wait_for (D-068) ---

    /// `wait_for` の操作 (対象は notepad.exe)。`extra` は `,"title":"…"` のような追加のフィールド
    fn wait_json(extra: &str, timeout_ms: u64) -> String {
        format!(r#"{{"type":"wait_for","process":"notepad.exe"{extra},"timeout_ms":{timeout_ms}}}"#)
    }

    fn el(name: &str, ty: &str, rect: Option<Rect>, text: Option<&str>) -> UiElement {
        UiElement {
            name: name.into(),
            control_type: ty.into(),
            automation_id: String::new(),
            class: String::new(),
            rect,
            enabled: true,
            offscreen: rect.is_none(),
            focused: false,
            password: false,
            toggle: None,
            selected: None,
            expanded: None,
            text: text.map(Into::into),
        }
    }

    fn save_button() -> (usize, UiElement) {
        (
            0x20,
            el("保存", "button", Some(Rect::new(110, 60, 40, 20)), None),
        )
    }

    fn document() -> (usize, UiElement) {
        (
            0x20,
            el(
                "テキスト エディター",
                "document",
                Some(Rect::new(100, 100, 600, 400)),
                Some("一手ずつ\r\n二行目"),
            ),
        )
    }

    /// 対象のメモ帳 (0x20) が一覧にあり、前面は端末の world
    fn wait_world() -> World {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "notepad.exe", "a.txt - メモ帳")];
        w
    }

    fn wait_timeout(e: &AppError) -> &serde_json::Value {
        assert_eq!(e.kind, ErrorKind::WaitTimeout, "{e}");
        assert_eq!(e.kind.exit_code(), 15);
        assert_eq!(e.reason, None);
        assert_no_title(e);
        detail(e)
    }

    /// (1) ウィンドウの条件が最初から満たされる: 何も送らず、入力デスクトップも keys_held も読まない。
    /// 結果の waited に窓が入り、要素のキーは無い。wait_for の無い操作列の出力には waited を出さない
    #[test]
    fn wait_for_window_present_is_satisfied_immediately_without_sending() {
        let world = wait_world().shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json("", 1000)),
        );
        let report = r.unwrap();
        assert_eq!(report.executed, 1);
        assert_eq!(report.waited.len(), 1);
        let wi = &report.waited[0];
        assert_eq!(wi.action_index, 0);
        assert_eq!(wi.window.as_ref().map(|w| w.hwnd), Some(0x20));
        assert_eq!(wi.checks, 1);
        assert_eq!(wi.elapsed_ms, 0);
        assert!(wi.element.is_none());
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["waited"][0]["window"]["process"], "notepad.exe");
        assert_eq!(v["waited"][0]["window"]["title"], "a.txt - メモ帳");
        assert!(v["waited"][0].get("element").is_none(), "{v}");
        assert!(
            v["waited"][0].get("note").is_none(),
            "present が最初から満たされても注意は付けない (D-070): {v}"
        );
        {
            let w = world.borrow();
            assert!(w.sent.is_empty(), "{:?}", w.sent);
            assert!(w.focus_calls.is_empty());
            assert_eq!(w.input_desktop_reads, 0, "読むだけなので判定しない");
            assert_eq!(w.keys_held_calls, 0);
            assert_eq!(w.find_window_calls, 1);
            assert_eq!(w.ui_element_calls, 0, "ウィンドウの条件では要素を読まない");
            assert_eq!(w.now_ms, 0, "待たない");
        }
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"wait","ms":1}"#),
        );
        assert!(
            serde_json::to_value(r.unwrap())
                .unwrap()
                .get("waited")
                .is_none(),
            "wait_for の無い操作列の出力は今までどおり"
        );
    }

    /// (2) 最初は無く、3 回目の判定で現れる: 250 ms ごとに確かめ、checks 3、elapsed_ms は 2 回分の間隔
    #[test]
    fn wait_for_polls_every_250ms_until_window_appears() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top_after_finds = Some((2, vec![top(0x20, "notepad.exe", "a.txt")]));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json("", 5000)),
        );
        let report = r.unwrap();
        let wi = &report.waited[0];
        assert_eq!(wi.checks, 3);
        // 間隔は 250 ms 固定 (D-068)。定数ではなく数で固定して、間隔の書き換えをここで捕まえる
        assert_eq!(wi.elapsed_ms, 500, "2 回の間隔");
        assert_eq!(wi.elapsed_ms, 2 * WAIT_FOR_POLL_MS);
        assert_eq!(wi.window.as_ref().map(|w| w.hwnd), Some(0x20));
        let w = world.borrow();
        assert_eq!(w.find_window_calls, 3);
        assert_eq!(w.now_ms, 500);
        assert!(w.sent.is_empty());
    }

    /// (3) 要素の条件: 要素があれば waited の element に center つきで入る (種類は正規化して比べる)。
    /// 2 回目の読みで現れれば checks 2。無ければ 15 で unmet が element、window に対象 (題名なし)
    #[test]
    fn wait_for_element_condition_reports_center_or_15_unmet_element() {
        let mut w = wait_world();
        w.elements = vec![save_button()];
        let world = w.shared();
        let s = script(&wait_json(
            r#","element":{"name":"保存","type":"Button"}"#,
            600,
        ));
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        let wi = &report.waited[0];
        let el = wi.element.as_ref().unwrap();
        assert_eq!(el["center"], serde_json::json!({ "x": 130, "y": 70 }));
        assert_eq!(el["type"], "button");
        assert_eq!(el["name"], "保存");
        assert!(el.get("text").is_none());
        assert_eq!(wi.window.as_ref().map(|w| w.hwnd), Some(0x20));
        assert_eq!(wi.checks, 1);
        // 2 回目の読みで現れる
        let mut w = wait_world();
        w.elements_after_reads = Some((1, vec![save_button()]));
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let wi = &r.unwrap().waited[0];
        assert_eq!(wi.checks, 2);
        assert_eq!(wi.elapsed_ms, WAIT_FOR_POLL_MS);
        assert!(wi.element.is_some());
        // 無ければ 15
        let world = wait_world().shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        let d = wait_timeout(&e).clone();
        assert_eq!(d["unmet"], "element");
        assert_eq!(d["timeout_ms"], 600);
        assert_eq!(d["checks"], 4, "t = 0、250、500、600 の 4 回");
        assert_eq!(
            d["window"],
            serde_json::json!({"process": "notepad.exe", "class": "TargetClass", "pid": 4242, "hwnd": 0x20})
        );
        assert!(e.message.contains("element が無い"), "{}", e.message);
        assert!(
            e.message.contains("name=\"保存\"") && e.message.contains("type=\"Button\""),
            "{}",
            e.message
        );
        assert_eq!(e.to_json()["error"]["code"], "wait_timeout");
        assert_eq!(world.borrow().now_ms, 600);
    }

    /// 要素の条件で満たされた waited の element に、フォーカスと状態が出る (D-089。要素の条件は find の問い合わせ)
    #[test]
    fn wait_for_element_reports_focus_and_states_in_waited() {
        let mut w = wait_world();
        let (h, mut e) = save_button();
        e.focused = true;
        e.toggle = Some("off".into());
        w.elements = vec![(h, e)];
        let world = w.shared();
        let s = script(&wait_json(r#","element":{"name":"保存"}"#, 600));
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let report = r.unwrap();
        let el = report.waited[0].element.as_ref().unwrap();
        assert_eq!(el["focused"], true, "{el}");
        assert_eq!(el["toggle"], "off", "{el}");
        assert!(el.get("selected").is_none(), "{el}");
        assert!(el.get("expanded").is_none(), "{el}");
    }

    /// (4) 文字列の条件: 最初に文字列を持つ要素の文字列 (LF にそろえる) が含めば満たされ、要素に text キーは無い。
    /// 含まなければ 15 で unmet が text。文字列を持つ要素が無ければ unmet が element
    #[test]
    fn wait_for_text_condition_matches_or_15_unmet_text_or_element() {
        let mut w = wait_world();
        w.elements = vec![save_button(), document()];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","text":"二行目""#, 600)),
        );
        let report = r.unwrap();
        let wi = &report.waited[0];
        let el = wi.element.as_ref().unwrap();
        assert_eq!(el["type"], "document");
        assert!(el.get("text").is_none(), "{el}");
        assert!(!serde_json::to_string(&report).unwrap().contains("一手ずつ"));
        assert_eq!(el["center"], serde_json::json!({ "x": 400, "y": 300 }));
        // LF にそろえた文字列に対する部分一致。条件の側を CRLF / CR で書いても同じ (両方を LF にそろえる)
        for needle in [
            r#"一手ずつ\n二行目"#,
            r#"一手ずつ\r\n二行目"#,
            r#"一手ずつ\r二行目"#,
        ] {
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(&wait_json(&format!(r#","text":"{needle}""#), 0)),
            );
            assert!(r.is_ok(), "{needle}: {r:?}");
        }
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","text":"三行目""#, 300)),
        );
        let e = r.unwrap_err();
        let d = wait_timeout(&e).clone();
        assert_eq!(d["unmet"], "text");
        assert_eq!(d["window"]["hwnd"], 0x20);
        assert!(e.message.contains("text が無い"), "{}", e.message);
        // 絞り込みの中に文字列を持つ要素が無い (button には文字列が無い)
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(
                r#","element":{"type":"button"},"text":"二行目""#,
                0,
            )),
        );
        assert_eq!(wait_timeout(&r.unwrap_err())["unmet"], "element");
        // 文字列を持つ要素が 1 つも無い
        world.borrow_mut().elements = vec![save_button()];
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","text":"二行目""#, 0)),
        );
        assert_eq!(wait_timeout(&r.unwrap_err())["unmet"], "element");
    }

    /// 文字列の条件はパスワードの要素の文字列を読まずに飛ばす (D-090): その要素だけの窓では、文字列が一致しても
    /// 満たされず 15 で unmet は element。absent を付けると最初の判定で満たされ、note は既存の element の文
    /// (パスワードの要素は最初から無いと数える。受け入れた振る舞いを固定する)
    #[test]
    fn wait_for_text_condition_never_matches_password_element() {
        let secret = || {
            let mut e = el(
                "パスワード",
                "edit",
                Some(Rect::new(110, 60, 200, 20)),
                None,
            );
            e.text = Some("secret".into());
            e.password = true;
            (0x20, e)
        };
        let mut w = wait_world();
        w.elements = vec![secret()];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","text":"secret""#, 300)),
        );
        let e = r.unwrap_err();
        assert_eq!(wait_timeout(&e)["unmet"], "element");
        // absent: 最初の判定で満たされ、note は文字列を持つ要素が無いときと同じ element の文
        let report = |elements: Vec<(usize, UiElement)>| {
            let mut w = wait_world();
            w.elements = elements;
            let world = w.shared();
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(&wait_json(r#","text":"secret","absent":true"#, 1000)),
            );
            r.unwrap()
        };
        let got = report(vec![secret()]);
        assert_eq!(got.waited[0].checks, 1);
        assert!(got.waited[0].element.is_none());
        let note = waited_note(&got).expect("note");
        assert_eq!(Some(note), waited_note(&report(vec![save_button()])));
        assert!(!serde_json::to_string(&got).unwrap().contains("secret"));
    }

    /// (5) absent: ウィンドウが無ければ即時に満たされ window キーは無い。要素があれば時間切れで unmet が element。
    /// 2 回目に消えれば checks 2。absent + text が「要素はあるが含まない」で満たされたときは window はあり element は無い
    #[test]
    fn wait_for_absent_is_satisfied_when_missing_and_times_out_while_present() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","absent":true"#, 1000)),
        );
        let report = r.unwrap();
        let wi = &report.waited[0];
        assert!(wi.window.is_none());
        assert!(wi.element.is_none());
        assert_eq!(wi.checks, 1);
        let v = serde_json::to_value(&report).unwrap();
        assert!(v["waited"][0].get("window").is_none(), "{v}");
        assert_eq!(v["waited"][0]["checks"], 1);
        // 要素が残っていれば時間切れ
        let mut w = wait_world();
        w.elements = vec![save_button()];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(
                r#","element":{"name":"保存"},"absent":true"#,
                300,
            )),
        );
        let e = r.unwrap_err();
        let d = wait_timeout(&e).clone();
        assert_eq!(d["unmet"], "element");
        assert_eq!(d["window"]["hwnd"], 0x20);
        assert!(e.message.contains("element がまだある"), "{}", e.message);
        assert!(e.message.contains("absent=true"), "{}", e.message);
        // ウィンドウ自身が残っていれば unmet は window
        let world = wait_world().shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","absent":true"#, 0)),
        );
        assert_eq!(wait_timeout(&r.unwrap_err())["unmet"], "window");
        // 2 回目の判定で消える
        let mut w = wait_world();
        w.top_after_finds = Some((1, vec![]));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","absent":true"#, 1000)),
        );
        let wi = &r.unwrap().waited[0];
        assert_eq!(wi.checks, 2);
        assert!(wi.window.is_none(), "消えた後は窓を出さない");
        assert_eq!(wi.elapsed_ms, WAIT_FOR_POLL_MS);
        // absent + text: 要素はあるが含まない → 満たされ、window はあり element は無い
        let mut w = wait_world();
        w.elements = vec![document()];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","text":"三行目","absent":true"#, 1000)),
        );
        let report = r.unwrap();
        let wi = &report.waited[0];
        assert_eq!(wi.window.as_ref().map(|w| w.hwnd), Some(0x20));
        assert!(wi.element.is_none());
        let v = serde_json::to_value(&report).unwrap();
        assert!(v["waited"][0].get("window").is_some());
        assert!(v["waited"][0].get("element").is_none(), "{v}");
        // absent + text で含んでいれば時間切れで unmet が text
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","text":"二行目","absent":true"#, 0)),
        );
        assert_eq!(wait_timeout(&r.unwrap_err())["unmet"], "text");
    }

    /// 満たされた結果の JSON の `waited[0].note` (無ければ None)
    fn waited_note(report: &RunReport) -> Option<String> {
        let v = serde_json::to_value(report).unwrap();
        v["waited"][0]
            .get("note")
            .map(|n| n.as_str().expect("note は文字列").to_string())
    }

    /// (5b) absent が最初の判定で満たされると、最初に無かった段 (window / element / text) に応じて
    /// 次に確かめる道具を書いた note が付く (D-070)
    #[test]
    fn wait_for_absent_note_on_first_check_names_next_tool() {
        let run = |w: World, extra: &str| -> RunReport {
            let world = w.shared();
            let (r, _) = exec(
                &world,
                &policy(ENABLED),
                &mut StopSet::new(),
                &script(&wait_json(extra, 1000)),
            );
            r.unwrap()
        };
        // (a) ウィンドウが最初から無い → window の文
        let mut w = World::new();
        w.foreground = Some(terminal());
        let report = run(w, r#","absent":true"#);
        assert_eq!(report.waited[0].checks, 1);
        let n = waited_note(&report).expect("note");
        assert!(n.contains("windows") && n.contains("最小化"), "{n}");
        assert!(n.starts_with("最初の確認で既に無かった。"), "{n}");
        // 最小化されているだけでも同じ文
        let mut w = wait_world();
        w.top[0].minimized = true;
        w.top[0].rect = None;
        assert_eq!(
            waited_note(&run(w, r#","absent":true"#)).as_deref(),
            Some(n.as_str())
        );
        // (b) ウィンドウはあるが要素が最初から無い → element の文
        let report = run(
            wait_world(),
            r#","element":{"name":"文書の索引"},"absent":true"#,
        );
        let ne = waited_note(&report).expect("note");
        assert!(ne.contains("find") && ne.contains("gpui"), "{ne}");
        assert!(!ne.contains("windows"), "{ne}");
        assert_eq!(report.waited[0].window.as_ref().map(|w| w.hwnd), Some(0x20));
        // absent + text だけで、文字列を持つ要素が無い → 同じ element の文
        let mut w = wait_world();
        w.elements = vec![save_button()];
        let report = run(w, r#","text":"二行目","absent":true"#);
        assert_eq!(waited_note(&report).as_deref(), Some(ne.as_str()));
        // (c) 要素はあるが最初から含まない → text の文
        let mut w = wait_world();
        w.elements = vec![document()];
        let nt = waited_note(&run(w, r#","text":"三行目","absent":true"#)).expect("note");
        assert!(nt.contains("text で"), "{nt}");
        assert!(!nt.contains("gpui") && !nt.contains("windows"), "{nt}");
    }

    /// absent でも、2 回目の判定で満たされたときと、timeout_ms 0 (1 回だけ確かめる) では note のキーが無い (D-070)
    #[test]
    fn wait_for_absent_note_absent_after_second_check_or_timeout_zero() {
        // (a) 2 回目に消えた
        let mut w = wait_world();
        w.top_after_finds = Some((1, vec![]));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","absent":true"#, 1000)),
        );
        let report = r.unwrap();
        assert_eq!(report.waited[0].checks, 2);
        assert_eq!(waited_note(&report), None);
        assert!(report.waited[0].note.is_none());
        // (b) timeout_ms 0 で最初から無い
        let mut w = World::new();
        w.foreground = Some(terminal());
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","absent":true"#, 0)),
        );
        let report = r.unwrap();
        assert_eq!(report.waited[0].checks, 1);
        assert_eq!(waited_note(&report), None);
    }

    /// (6) 時間切れの進み具合: click の後の wait_for が 15。executed 1、partial false、hint の頭に実行済みの一文、
    /// 末尾は 15 の既定の文。detail の checks は t = 0、250、300 の 3 回、window は null (キーはある)
    #[test]
    fn wait_for_timeout_after_click_reports_progress_and_default_hint() {
        let world = World::new().shared();
        let s = script(&format!(
            r#"{{"type":"click","x":10,"y":10}},{}"#,
            wait_json(r#","title":"zzz""#, 300)
        ));
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        let d = wait_timeout(&e).clone();
        assert_eq!(e.action_index, Some(1));
        assert_eq!(progress(&e)["executed"], 1);
        assert_eq!(progress(&e)["partial"], false);
        assert_eq!(d["timeout_ms"], 300);
        assert_eq!(d["checks"], 3);
        assert_eq!(d["unmet"], "window");
        assert!(d["window"].is_null(), "{d}");
        assert!(d.as_object().unwrap().contains_key("window"));
        let h = e.hint.as_deref().unwrap();
        assert!(h.starts_with("actions[0] は実行済み (executed=1)。"), "{h}");
        assert!(
            h.ends_with(&format!(
                "以下は失敗した actions[1] について: {}",
                ErrorKind::WaitTimeout.hint()
            )),
            "{h}"
        );
        assert!(
            e.message.contains("300 ms") && e.message.contains("確かめた回数 3"),
            "{}",
            e.message
        );
        assert!(e.message.contains("title=\"zzz\""), "{}", e.message);
        let v = e.to_json();
        assert_eq!(v["error"]["code"], "wait_timeout");
        assert_eq!(v["error"]["exit_code"], 15);
        assert_eq!(v["executed"], 1);
        assert!(v.get("waited").is_none(), "満たされた wait_for は無い: {v}");
        let w = world.borrow();
        assert_eq!(w.sent.len(), 1, "click だけ送った");
        assert_eq!(w.now_ms, 300);
    }

    /// (7) 待っている間も停止要因 (目印など) と期限を見る: 最初の 50 ms の待ちで止まる。期限は時計を進める停止要因で
    #[test]
    fn wait_for_honors_stop_sources_and_deadline_while_waiting() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let world = w.shared();
        let mut stops = StopSet::new();
        // 1 回目の poll (操作の前) は通し、待ちの中の 2 回目で止める
        stops.push(Box::new(StopAfterPolls(1)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut stops,
            &script(&wait_json("", 5000)),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.action_index, Some(0));
        assert_eq!(progress(&e)["partial"], false, "何も送っていない");
        assert_eq!(world.borrow().now_ms, STOP_POLL_MS);
        // 期限: 予算 900 + 余裕 1000 <= 2000 で検証は通るが、待ちの途中で期限が先に来る
        let world = World::new().shared();
        let p = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 2000\n"));
        let mut stops = StopSet::new();
        stops.push(Box::new(Jump(world.clone())));
        let (r, _) = exec(&world, &p, &mut stops, &script(&wait_json("", 900)));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert!(e.message.contains("最大実行時間"), "{}", e.message);
        assert!(world.borrow().sent.is_empty());
    }

    /// (8) 最小化は「無い」。ホスト、自分の端末、設定の拒否 (deny のクラス) の窓も「無い」(D-139) で、要素も読まない
    #[test]
    fn wait_for_treats_minimized_host_self_and_denied_as_absent() {
        let mut w = wait_world();
        w.top[0].minimized = true;
        w.top[0].rect = None;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json("", 0)),
        );
        let e = r.unwrap_err();
        let d = wait_timeout(&e).clone();
        assert_eq!(d["unmet"], "window");
        assert!(d["window"].is_null(), "最小化は見なかったのと同じ: {d}");
        assert_eq!(d["checks"], 1, "timeout_ms 0 は 1 回だけ確かめる");
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&wait_json(r#","absent":true"#, 0)),
        );
        assert!(r.unwrap().waited[0].window.is_none());
        // ホスト (起動した側)、自分の端末 (TUI からの実行)、deny.classes の窓は名指しの候補に入らず、「無い」と
        // 数える (D-139。以前は待たずに 7 / 4)。時間切れまで待って 15 で、要素を読まず、窓を出さない
        let s = script(&wait_json(r#","element":{"name":"保存"}"#, 300));
        let me = wait_world().top[0].info.clone();
        let denied = policy(&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\"]\n[deny]\nclasses=[\"TargetClass\"]\n"
        ));
        let cases: [(&str, &Policy, Option<&WindowInfo>, &[u32]); 3] = [
            ("host", &policy(ENABLED), None, &[me.pid]),
            ("self", &policy(ENABLED), Some(&me), &[]),
            ("deny.classes", &denied, None, &[]),
        ];
        for (label, p, self_window, host) in cases {
            let world = wait_world().shared();
            let (r, _) = exec_with(&world, p, &mut StopSet::new(), &s, self_window, host);
            let e = r.unwrap_err();
            let d = wait_timeout(&e).clone();
            assert_eq!(d["unmet"], "window", "{label}");
            assert!(d["window"].is_null(), "{label}: {d}");
            assert!(
                e.message.ends_with(NAMED_WINDOW_SCOPE_NOTE),
                "{label}: {}",
                e.message
            );
            assert_eq!(progress(&e)["executed"], 0, "{label}");
            let w = world.borrow();
            assert_eq!(w.now_ms, 300, "{label}: 時間切れまで待つ");
            assert_eq!(w.ui_element_calls, 0, "{label}: 要素を読まない");
            assert!(w.sent.is_empty(), "{label}");
        }
    }

    /// (9) 要素の読み取りの失敗: 読む直前にウィンドウが消えていればその回は「無い」(absent なら満たされる)、
    /// 残っていれば 8
    #[test]
    fn wait_for_tolerates_element_read_failure_when_window_vanished_else_8() {
        let s = script(&wait_json(
            r#","element":{"name":"保存"},"absent":true"#,
            1000,
        ));
        let mut w = wait_world();
        w.elements = vec![save_button()];
        w.ui_elements_fail_at = Some((0, BackendError::Os("ElementFromHandle".into())));
        w.top_after_finds = Some((1, vec![]));
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let wi = &r.unwrap().waited[0];
        assert_eq!(wi.checks, 1);
        assert!(wi.window.is_none());
        assert_eq!(world.borrow().find_window_calls, 2, "探し直した");
        let mut w = wait_world();
        w.elements = vec![save_button()];
        w.ui_elements_fail_at = Some((0, BackendError::Os("ElementFromHandle".into())));
        let world = w.shared();
        let (r, _) = exec(&world, &policy(ENABLED), &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::OsError, "{e}");
        assert_eq!(e.action_index, Some(0));
        assert_eq!(progress(&e)["executed"], 0);
        assert!(e.message.contains("ElementFromHandle"), "{}", e.message);
    }

    /// (10) wait_for だけの操作列は入力デスクトップを読まず、ロック中 (カーソルが 5) でも監視なしで読める
    #[test]
    fn wait_for_only_script_skips_input_desktop_and_runs_while_locked() {
        let mut w = wait_world();
        w.input_desktop = Ok(winlogon());
        w.cursor_fail_after_reads = Some((0, cursor_locked()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&format!(
                r#"{{"type":"wait","ms":100}},{}"#,
                wait_json("", 1000)
            )),
        );
        let report = r.unwrap();
        assert_eq!(report.executed, 2);
        assert_eq!(report.waited.len(), 1);
        let w = world.borrow();
        assert_eq!(w.input_desktop_reads, 0);
        assert_eq!(w.keys_held_calls, 0);
        assert!(w.sent.is_empty());
        // 撮影を付けると (wait_for の後に screenshot) 撮影を含むので、ロック中は 14 で止まる
        drop(w);
        let (r, images) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&format!("{},{SHOT}", wait_json("", 1000))),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{e}");
        assert!(images.written.is_empty());
    }

    // --- 画面の待ち (wait_stable / wait_change。D-081) ---
    // 矩形は 10x10 (100 点。長辺 256 以下なので縮小を素通りし、差は frames の k の差 / 100)

    const SCREEN_RECT: Rect = Rect::new(0, 0, 10, 10);

    /// `{"type": kind, "region": 10x10, ...extra}`
    fn screen_json(kind: &str, extra: &str) -> String {
        format!(r#"{{"type":"{kind}","region":{{"x":0,"y":0,"w":10,"h":10}}{extra}}}"#)
    }

    fn screen_world(frames: Vec<usize>) -> Rc<RefCell<World>> {
        let mut w = World::new();
        w.frames = frames;
        w.shared()
    }

    fn run_screen(
        world: &Rc<RefCell<World>>,
        json: &str,
    ) -> (Result<RunReport, AppError>, MemoryImages) {
        exec(world, &policy(ENABLED), &mut StopSet::new(), &script(json))
    }

    /// 満たされた記録 (waited) の elapsed_ms と checks
    fn waited_at(report: &RunReport) -> (u64, usize) {
        assert_eq!(report.waited.len(), 1, "{report:?}");
        (report.waited[0].elapsed_ms, report.waited[0].checks)
    }

    /// 動かない画面: stable_ms 300 で t = 0、100、200、300 の 4 回撮って満たす。何も送らず、何も保存しない。
    /// waited に最後の矩形 (rect) が入り、changed_ratio、element、note、window は出さない
    #[test]
    fn wait_stable_still_screen_settles_after_stable_ms() {
        let world = screen_world(Vec::new());
        let (r, images) = run_screen(&world, &screen_json("wait_stable", ""));
        let report = r.unwrap();
        assert_eq!(report.executed, 1);
        assert_eq!(waited_at(&report), (300, 4));
        assert_eq!(report.waited[0].rect, Some(SCREEN_RECT));
        assert!(report.screenshots.is_empty());
        let w = world.borrow();
        assert_eq!(w.capture_calls, 4);
        assert_eq!(w.capture_rects, vec![SCREEN_RECT; 4]);
        assert!(w.sent.is_empty(), "何も送らない");
        assert!(images.written.is_empty(), "画像は保存しない");
        assert_eq!(w.now_ms, 300);
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(
            v["waited"][0],
            serde_json::json!({"action_index":0,"elapsed_ms":300,"checks":4,"rect":{"x":0,"y":0,"w":10,"h":10}})
        );
    }

    /// `{"type":"screenshot","path":"a.png","region":{w x h}, ...extra}`
    fn shot_json(w: u32, h: u32, extra: &str) -> String {
        format!(
            r#"{{"type":"screenshot","path":"a.png","region":{{"x":0,"y":0,"w":{w},"h":{h}}}{extra}}}"#
        )
    }

    /// screenshot の settle (D-081): 動かない画面は wait_stable と同じく t = 0、100、200、300 の 4 回撮って落ち着き、
    /// 最後のフレームを 1 枚だけ保存する (撮り直さない)。結果に settled: true と settle_ms (待った時間) が付く。
    /// settle の無い screenshot の結果には、どちらのキーも出ない
    #[test]
    fn screenshot_settle_waits_until_stable_and_saves_last_frame() {
        let world = screen_world(Vec::new());
        let (r, images) = run_screen(&world, &shot_json(10, 10, r#","settle":{}"#));
        let report = r.unwrap();
        assert_eq!(report.executed, 1);
        assert_eq!(report.screenshots.len(), 1);
        assert_eq!(report.screenshots[0].settled, Some(true));
        assert_eq!(report.screenshots[0].settle_ms, Some(300));
        assert!(report.waited.is_empty(), "settle は waited に積まない");
        let w = world.borrow();
        assert_eq!(
            w.capture_calls, 4,
            "落ち着いた最後のフレームを使い、撮り直さない"
        );
        assert_eq!(w.capture_rects, vec![SCREEN_RECT; 4]);
        assert!(w.sent.is_empty(), "何も送らない");
        assert_eq!(w.now_ms, 300);
        drop(w);
        assert_eq!(images.written.len(), 1, "保存は 1 枚");
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["screenshots"][0]["settled"], true);
        assert_eq!(v["screenshots"][0]["settle_ms"], 300);
        // settle の無い screenshot: 1 回だけ撮り、settled / settle_ms のキーは無い
        let world = screen_world(Vec::new());
        let (r, _) = run_screen(&world, &shot_json(10, 10, ""));
        let report = r.unwrap();
        assert_eq!(world.borrow().capture_calls, 1);
        assert_eq!(report.screenshots[0].settled, None);
        let v = serde_json::to_value(&report).unwrap();
        assert!(v["screenshots"][0].get("settled").is_none(), "{v}");
        assert!(v["screenshots"][0].get("settle_ms").is_none(), "{v}");
    }

    /// screenshot の settle (D-081) で保存するのは、比べるのに縮めた画像 (長辺 256) ではなく撮った大きさの画像。
    /// 長辺 256 を超える矩形 (1000x500) で、settle の無い同じ screenshot と同じ大きさになる
    #[test]
    fn screenshot_settle_saves_full_size_frame() {
        let world = screen_world(Vec::new());
        let (r, images) = run_screen(&world, &shot_json(1000, 500, r#","settle":{}"#));
        let settled = r.unwrap();
        let world = screen_world(Vec::new());
        let (r, _) = run_screen(&world, &shot_json(1000, 500, ""));
        let plain = r.unwrap();
        let size = |rep: &RunReport| (rep.screenshots[0].width, rep.screenshots[0].height);
        assert_eq!(size(&settled), (1000, 500));
        assert_eq!(size(&settled), size(&plain));
        assert_eq!(settled.screenshots[0].settled, Some(true));
        assert_eq!(
            (images.written[0].1.width, images.written[0].1.height),
            (1000, 500)
        );
    }

    /// screenshot の settle (D-081) の時間切れは失敗にしない: 動き続ける画面で既定の 3000 ms 待ち、最後のフレームを
    /// 保存して settled: false、settle_ms 3000 の成功で返す
    #[test]
    fn screenshot_settle_times_out_as_settled_false() {
        // 最後 (31 回目。frames[30]) だけを他のどのフレームとも違う 37 にする (範囲外も最後の値で 37)
        let mut frames: Vec<usize> = (0..30).map(|i| (i % 2) * 100).collect();
        frames.push(37);
        let world = screen_world(frames);
        let (r, images) = run_screen(&world, &shot_json(10, 10, r#","settle":{}"#));
        let report = r.unwrap();
        assert_eq!(report.executed, 1);
        assert_eq!(report.screenshots[0].settled, Some(false));
        assert_eq!(
            report.screenshots[0].settle_ms,
            Some(crate::script::DEFAULT_SETTLE_TIMEOUT_MS)
        );
        assert_eq!(
            world.borrow().capture_calls,
            31,
            "t = 0 から 3000 まで 100 ms ごと"
        );
        assert_eq!(images.written.len(), 1);
        // 最後 (31 回目。frames[30] = 37) のフレームを保存した: 先頭 37 点が 0x00、残りが 0x7f
        let rgba = &images.written[0].1.rgba;
        assert!(rgba[..37 * 4].iter().all(|&b| b == 0x00), "最後のフレーム");
        assert!(rgba[37 * 4..].iter().all(|&b| b == 0x7f), "最後のフレーム");
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["screenshots"][0]["settled"], false);
    }

    /// 最初は動いていて 4 回目 (t = 300) から止まる: 変化のたびに静けさの始まりが進み、最後の変化 + 300 で満たす
    #[test]
    fn wait_stable_resets_quiet_start_on_change() {
        let world = screen_world(vec![0, 1, 2, 3]);
        let (r, _) = run_screen(&world, &screen_json("wait_stable", ""));
        assert_eq!(waited_at(&r.unwrap()), (600, 7));
    }

    /// threshold 以下の小さな変化 (1 段 0.01) は静けさを破らない。threshold 0 なら破る
    #[test]
    fn wait_stable_ignores_change_within_threshold() {
        let frames = vec![0, 1, 2, 3, 4];
        let world = screen_world(frames.clone());
        let (r, _) = run_screen(&world, &screen_json("wait_stable", r#","threshold":0.01"#));
        assert_eq!(waited_at(&r.unwrap()), (300, 4), "0.01 ちょうどは静か");
        let world = screen_world(frames);
        let (r, _) = run_screen(&world, &screen_json("wait_stable", ""));
        assert_eq!(waited_at(&r.unwrap()), (700, 8), "threshold 0 は破る");
    }

    /// 動き続ける画面 (毎回全点が変わる): 15、unmet: stable、detail に最後の矩形と差 1.0、hint は画面の待ちの文
    #[test]
    fn wait_stable_times_out_with_unmet_stable_and_screen_hint() {
        let frames: Vec<usize> = (0..40).map(|i| if i % 2 == 0 { 0 } else { 100 }).collect();
        let world = screen_world(frames);
        let (r, images) = run_screen(&world, &screen_json("wait_stable", r#","timeout_ms":1000"#));
        let e = r.unwrap_err();
        let d = wait_timeout(&e);
        assert_eq!(
            d,
            &serde_json::json!({
                "timeout_ms": 1000,
                "checks": 11,
                "unmet": "stable",
                "window": null,
                "rect": {"x":0,"y":0,"w":10,"h":10},
                "last_changed_ratio": 1.0,
            })
        );
        assert_eq!(e.action_index, Some(0));
        assert_eq!(e.hint.as_deref(), Some(SCREEN_WAIT_HINT));
        assert_ne!(e.hint.as_deref(), Some(ErrorKind::WaitTimeout.hint()));
        assert!(e.message.contains("落ち着かなかった"), "{}", e.message);
        assert_eq!(progress(&e)["executed"], 0);
        assert_eq!(progress(&e)["partial"], false);
        assert_eq!(world.borrow().capture_calls, 11);
        assert!(images.written.is_empty());
        // MCP でも同じ文
        let world = screen_world((0..40).map(|i| (i % 2) * 100).collect());
        let mut sink = MockSink(world.clone());
        let desktop = MockDesktop(world.clone());
        let mut clock = MockClock(world.clone());
        let mut images = MemoryImages::default();
        let e = Executor {
            policy: &policy(ENABLED),
            sink: &mut sink,
            desktop: &desktop,
            clock: &mut clock,
            stops: &mut StopSet::new(),
            images: &mut images,
            indicator: &mut MockIndicator(world.clone()),
            input_lock: &mut MockInputLock(world.clone()),
        }
        .execute_on(
            &script(&screen_json("wait_stable", r#","timeout_ms":300"#)),
            &opts(None, &[], Surface::Mcp),
        )
        .unwrap_err();
        assert_eq!(e.hint.as_deref(), Some(SCREEN_WAIT_HINT));
    }

    /// wait_change: 4 回目 (t = 300) で半分が変わると満たし、changed_ratio に基準との差。
    /// 変わらなければ 15 で unmet: change、last_changed_ratio 0
    #[test]
    fn wait_change_satisfied_on_change_or_15_unmet_change() {
        let world = screen_world(vec![0, 0, 0, 50]);
        let (r, _) = run_screen(&world, &screen_json("wait_change", ""));
        let report = r.unwrap();
        assert_eq!(waited_at(&report), (300, 4));
        assert_eq!(report.waited[0].changed_ratio, Some(0.5));
        assert_eq!(report.waited[0].rect, Some(SCREEN_RECT));
        let world = screen_world(Vec::new());
        let (r, _) = run_screen(&world, &screen_json("wait_change", r#","timeout_ms":500"#));
        let e = r.unwrap_err();
        let d = wait_timeout(&e);
        assert_eq!(d["unmet"], "change");
        assert_eq!(d["checks"], 6);
        assert_eq!(d["last_changed_ratio"], 0.0);
        assert_eq!(d["timeout_ms"], 500);
        assert!(e.message.contains("変わらなかった"), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(SCREEN_WAIT_HINT));
        assert_eq!(world.borrow().now_ms, 500);
    }

    /// wait_change の threshold: それ以下の変化 (0.03 <= 0.05) では満たさず、超えれば (0.06) 満たす
    #[test]
    fn wait_change_ignores_change_within_threshold() {
        let world = screen_world(vec![0, 0, 3]);
        let (r, _) = run_screen(
            &world,
            &screen_json("wait_change", r#","threshold":0.05,"timeout_ms":500"#),
        );
        let e = r.unwrap_err();
        assert_eq!(wait_timeout(&e)["last_changed_ratio"], 0.03);
        let world = screen_world(vec![0, 0, 6]);
        let (r, _) = run_screen(
            &world,
            &screen_json("wait_change", r#","threshold":0.05,"timeout_ms":500"#),
        );
        let report = r.unwrap();
        assert_eq!(waited_at(&report), (200, 3));
        assert_eq!(report.waited[0].changed_ratio, Some(0.06));
    }

    /// wait_change は直前ではなく基準 (待ち始めのフレーム) と比べる: 1 段ずつ 0.01 だけ変わる画面でも、基準との差が
    /// threshold 0.025 を超えた 3 段目で満たし changed_ratio は 0.03。2 段で止まれば時間切れで last_changed_ratio 0.02
    #[test]
    fn wait_change_compares_with_baseline_not_previous() {
        let json = screen_json("wait_change", r#","threshold":0.025,"timeout_ms":1000"#);
        let world = screen_world(vec![0, 1, 2, 3]);
        let (r, _) = run_screen(&world, &json);
        let report = r.unwrap();
        assert_eq!(waited_at(&report), (300, 4));
        assert_eq!(report.waited[0].changed_ratio, Some(0.03));
        let world = screen_world(vec![0, 1, 2]);
        let (r, _) = run_screen(&world, &json);
        let e = r.unwrap_err();
        assert_eq!(wait_timeout(&e)["last_changed_ratio"], 0.02);
    }

    /// window で待つ: 矩形の位置だけが変わった比較 (大きさは同じ) は変化と数える (差 1.0)。waited に窓と動いた後の
    /// 矩形。時間切れの detail.window に題名は無い。見つからなければ 12 で撮らない
    #[test]
    fn wait_screen_window_move_counts_as_change_and_missing_window_is_12() {
        let np = |rect: Rect| {
            let mut t = top(0x20, "Notepad.exe", "a.txt - メモ帳");
            t.rect = Some(rect);
            t
        };
        let a = Rect::new(100, 100, 200, 100);
        let b = Rect::new(120, 100, 200, 100);
        let mut w = World::new();
        w.default_window = Some(np(a).info);
        w.top = vec![np(a)];
        // 2 回の find_window (撮影 2 回) の後に、同じ大きさのまま右へ動く
        w.top_after_finds = Some((2, vec![np(b)]));
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let s = script(r#"{"type":"wait_change","window":{"process":"notepad.exe"}}"#);
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let report = r.unwrap();
        assert_eq!(waited_at(&report), (200, 3));
        assert_eq!(report.waited[0].changed_ratio, Some(1.0));
        assert_eq!(report.waited[0].rect, Some(b));
        assert_eq!(report.waited[0].window.as_ref().map(|w| w.hwnd), Some(0x20));
        assert_eq!(world.borrow().capture_rects, vec![a, a, b]);
        // 動かない窓の時間切れ: detail.window に題名を入れない
        let s =
            script(r#"{"type":"wait_change","window":{"process":"notepad.exe"},"timeout_ms":200}"#);
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        let d = wait_timeout(&e);
        assert_no_title(&e);
        assert_eq!(d["window"]["hwnd"], 0x20);
        assert_eq!(d["window"]["process"], "Notepad.exe");
        assert_eq!(
            d["rect"],
            serde_json::json!({"x":120,"y":100,"w":200,"h":100})
        );
        // 見つからない
        let before = world.borrow().capture_calls;
        let s =
            script(r#"{"type":"wait_stable","window":{"process":"notepad.exe","title":"zzz"}}"#);
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &s);
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::WindowNotFound, "{e}");
        assert_eq!(e.action_index, Some(0));
        assert_eq!(world.borrow().capture_calls, before, "撮らない");
        assert_eq!(world.borrow().now_ms, 400, "待たない (前の 2 回の分だけ)");
    }

    /// 撮るたびの判定 (D-081): deny の窓が見えていれば撮らず待たずに 7。待つ間にロックされたら次の撮影の前に 14
    #[test]
    fn wait_screen_refuses_denied_window_and_lock_without_waiting() {
        let mut w = World::new();
        w.stack = vec![stacked(terminal(), Rect::new(5, 5, 200, 200), false)];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &deny_zed(),
            &mut StopSet::new(),
            &script(&screen_json("wait_stable", "")),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard, "{e}");
        assert_eq!(e.hint.as_deref(), Some(CAPTURE_SENSITIVE_HINT));
        assert_eq!(detail(&e)["window"]["hwnd"], 0x10);
        assert_no_title(&e);
        assert_eq!(world.borrow().capture_calls, 0, "撮らない");
        assert_eq!(world.borrow().now_ms, 0, "待たない");
        // 読みの 0 回目は操作に入る前、1 回目は最初の撮影、2 回目 (次の撮影の前) からロック
        let world = screen_world(Vec::new());
        world.borrow_mut().input_desktop_after_reads = Some((2, Ok(lock_curtain())));
        let (r, _) = run_screen(&world, &screen_json("wait_change", ""));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{e}");
        assert_eq!(e.reason, Some(reason::LOCKED));
        assert_eq!(e.action_index, Some(0));
        assert_eq!(progress(&e)["partial"], false);
        let w = world.borrow();
        assert_eq!(w.capture_calls, 1, "ロックの後は撮らない");
        assert_eq!(w.input_desktop_reads, 3);
    }

    /// n 回目の poll で目印のファイルを作る停止要因 (待ちの途中で目印が置かれたことを模す)
    struct SentinelAt {
        polls: u32,
        at: u32,
        inner: crate::estop::SentinelStop,
        path: std::path::PathBuf,
    }

    impl StopSource for SentinelAt {
        fn poll(&mut self) -> Option<StopReason> {
            self.polls += 1;
            if self.polls == self.at {
                std::fs::write(&self.path, b"").unwrap();
            }
            self.inner.poll()
        }
    }

    /// n 回目の poll で時計を ms だけ進める停止要因 (待ちの途中で期限を過ぎたことを模す)
    struct JumpAt(Rc<RefCell<World>>, u32, u64);

    impl StopSource for JumpAt {
        fn poll(&mut self) -> Option<StopReason> {
            if self.1 > 0 {
                self.1 -= 1;
                if self.1 == 0 {
                    self.0.borrow_mut().now_ms += self.2;
                }
            }
            None
        }
    }

    /// 待つ間も停止要因を見る: 停止の要因と目印のファイルで 6、期限 (max_duration_ms) で 6。何も送らない
    #[test]
    fn wait_screen_honors_stop_sources_and_deadline() {
        let world = screen_world(Vec::new());
        let mut stops = StopSet::new();
        // 1 回目の poll (操作の前) は通し、待ちの中の 2 回目で止める
        stops.push(Box::new(StopAfterPolls(1)));
        let json = screen_json("wait_change", r#","timeout_ms":5000"#);
        let (r, _) = exec(&world, &policy(ENABLED), &mut stops, &script(&json));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.action_index, Some(0));
        assert_eq!(progress(&e)["partial"], false);
        assert_eq!(world.borrow().now_ms, STOP_POLL_MS);
        assert_eq!(world.borrow().capture_calls, 1);
        // 目印のファイル: 3 回目の poll (待ちの 2 回目の区切り) で置かれる
        let path = crate::testing::unique_temp_dir("wait-screen-stop").join("STOP");
        let world = screen_world(Vec::new());
        let mut stops = StopSet::new();
        stops.push(Box::new(SentinelAt {
            polls: 0,
            at: 3,
            inner: crate::estop::SentinelStop::new(vec![path.clone()]),
            path,
        }));
        let (r, _) = exec(&world, &policy(ENABLED), &mut stops, &script(&json));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert!(e.message.contains("STOP"), "{}", e.message);
        assert_eq!(world.borrow().now_ms, 2 * STOP_POLL_MS);
        // 期限: 予算 900 + 余裕 1000 <= 2000 で検証は通るが、待ちの途中で時計が期限を越える
        let world = screen_world(Vec::new());
        let p = policy(&format!("{ENABLED}[limits]\nmax_duration_ms = 2000\n"));
        let mut stops = StopSet::new();
        stops.push(Box::new(JumpAt(world.clone(), 2, 2500)));
        let json = screen_json("wait_change", r#","timeout_ms":900"#);
        let (r, _) = exec(&world, &p, &mut stops, &script(&json));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert!(e.message.contains("最大実行時間"), "{}", e.message);
        assert!(world.borrow().sent.is_empty());
    }

    /// 画面の待ちだけの操作列も、操作に入る前に入力デスクトップを判定する (needs_desktop。D-081): ロック中は
    /// 14 で止まり、action_index が付かない (grab の中の判定なら 0 が付く)。撮らない
    #[test]
    fn wait_screen_only_script_checks_input_desktop_before_start() {
        for kind in ["wait_stable", "wait_change"] {
            let world = screen_world(Vec::new());
            world.borrow_mut().input_desktop = Ok(lock_curtain());
            let (r, _) = run_screen(&world, &screen_json(kind, ""));
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::DesktopUnavailable, "{kind}: {e}");
            assert_eq!(e.reason, Some(reason::LOCKED));
            assert_eq!(e.action_index, None, "{kind}");
            let w = world.borrow();
            assert_eq!(w.capture_calls, 0, "{kind}");
            assert_eq!(w.input_desktop_reads, 1, "{kind}");
        }
    }

    // --- 画面の待ちの隠れの猶予 (D-082)。メモ帳 800x600 で、frames の大きな値は全点が変わったフレーム ---

    const ALL_DARK: usize = 1_000_000;

    fn notepad_wait(kind: &str, extra: &str) -> Script {
        script(&format!(
            r#"{{"type":"{kind}","window":{{"process":"notepad.exe"}}{extra}}}"#
        ))
    }

    /// 最初から隠れていて t = 120 で前面に描かれる: 最初のフレームは猶予の後に撮り、変化を捉えて満たす
    #[test]
    fn wait_change_window_waits_out_transient_cover() {
        let mut w = covered_notepad_world();
        w.windows_at_until_ms = Some(120);
        w.frames = vec![0, ALL_DARK];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &notepad_wait("wait_change", r#","timeout_ms":5000"#),
        );
        let report = r.unwrap();
        assert_eq!(report.waited.len(), 1);
        assert_eq!(report.waited[0].changed_ratio, Some(1.0));
        assert_eq!(world.borrow().capture_calls, 2);
    }

    /// 最初のフレームの猶予 (400 ms) は待ちの起点をずらすので、それより短い timeout_ms (300) を食わない:
    /// 15 ではなく満たし、elapsed_ms は猶予を含まない
    #[test]
    fn wait_change_first_frame_grace_does_not_eat_timeout() {
        let mut w = covered_notepad_world();
        w.windows_at_until_ms = Some(400);
        w.frames = vec![0, ALL_DARK];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &notepad_wait("wait_change", r#","timeout_ms":300"#),
        );
        let report = r.unwrap();
        let (elapsed, checks) = waited_at(&report);
        assert!(elapsed < 300, "{elapsed}");
        assert_eq!(checks, 2);
        assert_eq!(world.borrow().now_ms, 400 + WAIT_SCREEN_POLL_MS);
    }

    /// 待つ途中 (t = 50 から 350) の隠れの猶予は elapsed_ms に入る (起点をずらすのは最初のフレームだけ):
    /// t = 0 で基準、t = 100 から 350 まで猶予、350 で 2 枚目 (同じ)、450 で 3 枚目が変わる
    #[test]
    fn wait_change_mid_wait_grace_counts_in_elapsed() {
        let mut w = covered_notepad_world();
        w.windows_at_from_ms = Some(50);
        w.windows_at_until_ms = Some(350);
        w.frames = vec![0, 0, ALL_DARK];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &notepad_wait("wait_change", r#","timeout_ms":5000"#),
        );
        let report = r.unwrap();
        let (elapsed, checks) = waited_at(&report);
        assert!(elapsed >= 350, "{elapsed}");
        assert_eq!(checks, 3);
        assert_eq!(report.waited[0].changed_ratio, Some(1.0));
    }

    /// wait_stable の途中 (t = 50 から 550) の隠れの猶予は静けさに数えない: 猶予の後の 2 枚目では満たさず、
    /// そこから stable_ms (300) の静けさを確かめてから満たす
    #[test]
    fn wait_stable_mid_wait_grace_is_not_quiet() {
        let mut w = covered_notepad_world();
        w.windows_at_from_ms = Some(50);
        w.windows_at_until_ms = Some(550);
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &notepad_wait("wait_stable", r#","stable_ms":300,"timeout_ms":5000"#),
        );
        let report = r.unwrap();
        let (elapsed, checks) = waited_at(&report);
        assert!(checks >= 4, "{checks}");
        assert!(elapsed >= 550 + 300, "{elapsed}");
    }

    // ---- 枠と予告の間 (D-093) ----

    const CLICK: &str = r#"{"type":"click","x":10,"y":10}"#;

    /// 入力を許す設定に `[indicator]` の行を足した方針
    fn indicator_policy(extra: &str) -> Policy {
        policy(&format!("{ENABLED}[indicator]\n{extra}"))
    }

    /// 枠の見え方 (`shown_for_ms` の値) を決めた world
    fn indicator_world(shown_for: Option<u64>) -> World {
        let mut w = World::new();
        w.indicator_shown_for = shown_for;
        w
    }

    /// 呼ばれ方: 残す時間 `linger_ms`、予告を省くか
    fn lead_opts(linger_ms: u64, skip_lead: bool) -> ExecOptions<'static> {
        ExecOptions {
            self_window: None,
            host_pids: &[],
            surface: Surface::Cli,
            linger_ms,
            skip_lead,
        }
    }

    /// 予告の間に待った長さ: 実行の開始 (モックの時計の `start`) から `ctx` の起点 (`elapsed_ms` の起点) まで
    fn waited_before_run(world: &Rc<RefCell<World>>, start: u64, report: &RunReport) -> u64 {
        world.borrow().now_ms - start - report.elapsed_ms
    }

    /// 予告の間で止まった失敗: 何も送らず、進み具合が無く、枠を 1 回出して残さずに解く
    fn assert_lead_refused(world: &Rc<RefCell<World>>, e: &AppError) {
        assert!(e.progress.is_none(), "{e:?}");
        assert!(e.to_json().get("executed").is_none());
        assert_eq!(e.action_index, None);
        let w = world.borrow();
        assert!(w.sent.is_empty(), "{:?}", w.sent);
        assert_eq!(w.shows.len(), 1);
        assert_eq!(w.releases, vec![0], "止められた実行の後は枠を残さない");
    }

    #[test]
    fn overlay_shows_and_releases_for_input_script() {
        let world = World::new().shared();
        let r = exec_opts(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
            &lead_opts(5000, false),
        );
        r.unwrap();
        let w = world.borrow();
        assert_eq!(w.shows.len(), 1);
        assert_eq!(w.releases, vec![5000]);
        assert_eq!(w.sent.len(), 1);
    }

    /// 入力のバッチが無くても、focus だけ、close だけの操作列は画面を変えるので枠を出す
    #[test]
    fn overlay_shows_for_focus_only_and_close_only() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "Notepad.exe", "a.txt - メモ帳")];
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"focus","process":"notepad.exe"}"#),
        );
        assert_eq!(r.unwrap().focused.len(), 1);
        assert_eq!(world.borrow().shows.len(), 1, "focus だけ");
        assert_eq!(world.borrow().releases, vec![0]);

        let world = close_world().shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &script(CLOSE_A));
        assert_eq!(r.unwrap().closed.len(), 1);
        assert_eq!(world.borrow().shows.len(), 1, "close だけ");
        assert_eq!(world.borrow().releases, vec![0]);
    }

    /// 撮影と待ちだけ (画面を変えない) の操作列では、枠を読まず出さない
    #[test]
    fn overlay_not_shown_for_read_only_script() {
        let world = indicator_world(None).shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(&format!(r#"{SHOT},{{"type":"wait","ms":100}}"#)),
        );
        r.unwrap();
        let w = world.borrow();
        assert!(w.shows.is_empty());
        assert!(w.releases.is_empty());
        assert_eq!(w.indicator_reads, 0);
        assert_eq!(w.now_ms, 100, "予告を待たない");
    }

    /// 枠を出さない操作列 (待ちだけ) でも、止められた実行の後は前の呼び出しが残した自分の枠を消す (D-107)
    #[test]
    fn stopped_read_only_script_releases_overlay() {
        let world = World::new().shared();
        let mut stops = StopSet::new();
        // 1 回目の poll (操作の前) は通し、待ちの中の 2 回目で止める
        stops.push(Box::new(StopAfterPolls(1)));
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut stops,
            &script(r#"{"type":"wait","ms":5000}"#),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_eq!(e.kind.exit_code(), 6);
        let w = world.borrow();
        assert!(w.shows.is_empty(), "枠を新しく出さない");
        assert_eq!(w.releases, vec![0], "残っている枠を消す");
    }

    /// 検証の失敗 (4)、ロック (14)、押されたままのキー (10) で断る実行では枠を出さない
    #[test]
    fn overlay_not_shown_when_validation_or_precheck_fails() {
        let world = indicator_world(None).shared();
        let (r, _) = exec(
            &world,
            &Policy::closed(),
            &mut StopSet::new(),
            &script(CLICK),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::PolicyDenied);
        assert!(world.borrow().shows.is_empty(), "検証の失敗");

        let mut w = indicator_world(None);
        w.input_desktop = Ok(InputDesktop::Locked { name: None });
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::DesktopUnavailable);
        assert!(world.borrow().shows.is_empty(), "ロック");

        let mut w = indicator_world(None);
        w.held.keys = vec!["shift"];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        assert_eq!(r.unwrap_err().reason, Some(reason::KEYS_HELD));
        let w = world.borrow();
        assert!(w.shows.is_empty(), "押されたままのキー");
        assert!(w.releases.is_empty());
        assert_eq!(w.indicator_reads, 0);
    }

    /// `enabled = false` なら枠も予告の間も無い
    #[test]
    fn overlay_disabled_skips_show_and_lead() {
        let world = indicator_world(None).shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("enabled = false\nsound = true\n"),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let report = r.unwrap();
        assert_eq!(waited_before_run(&world, 0, &report), 0);
        let w = world.borrow();
        assert!(w.shows.is_empty());
        assert!(w.releases.is_empty());
        assert_eq!((w.indicator_reads, w.beeps), (0, 0));
        assert_eq!(w.sent.len(), 1);
    }

    /// 枠が出ていなければ lead_ms まるごと待ってから最初の入力を送る。sound が false なら鳴らさない
    #[test]
    fn lead_waits_when_no_overlay() {
        let world = indicator_world(None).shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("lead_ms = 3000\n"),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let report = r.unwrap();
        assert_eq!(waited_before_run(&world, 0, &report), 3000);
        let w = world.borrow();
        assert_eq!(w.beeps, 0);
        assert_eq!(w.indicator_reads, 1);
        assert_eq!(w.sent.len(), 1);
    }

    /// 枠が出て間もなければ (1000 ms 前)、残り (2000 ms) だけ待つ
    #[test]
    fn lead_waits_only_remaining_when_overlay_is_young() {
        let world = indicator_world(Some(1000)).shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("lead_ms = 3000\n"),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let report = r.unwrap();
        let waited = waited_before_run(&world, 0, &report);
        assert!((2000..3000).contains(&waited), "{waited}");
        assert_eq!(waited, 2000);
    }

    /// 枠が lead_ms 以上前から出ていれば待たず、音も鳴らさない (枠は出す)
    #[test]
    fn lead_skipped_when_overlay_old_enough() {
        let world = indicator_world(Some(3000)).shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("lead_ms = 3000\nsound = true\n"),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let report = r.unwrap();
        assert_eq!(waited_before_run(&world, 0, &report), 0);
        let w = world.borrow();
        assert_eq!(w.beeps, 0);
        assert_eq!(w.shows.len(), 1);
    }

    /// TUI (skip_lead) は枠を出すが、枠を読まず待たない
    #[test]
    fn lead_skipped_for_tui() {
        let world = indicator_world(None).shared();
        let report = exec_opts(
            &world,
            &indicator_policy("lead_ms = 3000\nsound = true\n"),
            &mut StopSet::new(),
            &script(CLICK),
            &lead_opts(0, true),
        )
        .unwrap();
        assert_eq!(waited_before_run(&world, 0, &report), 0);
        let w = world.borrow();
        assert_eq!(w.indicator_reads, 0);
        assert_eq!(w.beeps, 0);
        assert_eq!(w.shows.len(), 1);
        assert_eq!(w.releases, vec![0]);
    }

    /// 予告の間にカーソルを動かすと 6 で、何も送らない
    #[test]
    fn lead_stops_on_mouse_move() {
        let mut w = indicator_world(None);
        // 読み 0 は開始の読み。予告の間の 2 回目の確認の前に人が動かす
        w.cursor_moves_after_reads = vec![(2, Point::new(1500, 900))];
        let world = w.shared();
        // 残す時間を 0 以外にして、止められた実行が release(0) になることを見分ける
        let e = exec_opts(
            &world,
            &indicator_policy("lead_ms = 3000\n"),
            &mut StopSet::new(),
            &script(CLICK),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert!(e.message.contains("マウス"), "{}", e.message);
        assert_lead_refused(&world, &e);
        assert_eq!(world.borrow().now_ms, 2 * STOP_POLL_MS, "その場で止まる");
    }

    /// 予告の間の人の入力の判定は、予告の始めから猶予 (300 ms) より後の入力だけを見る。
    /// 始めより前 (100 ms 前) の入力と、始めから 200 ms の入力では止まらず、1500 ms の入力で 10 `lead_input`
    #[test]
    fn lead_detects_input_only_after_lead_start() {
        let start = 1000;
        let run = |input_at: Option<u64>| {
            let mut w = indicator_world(None);
            w.now_ms = start;
            w.last_input_at_ms = Some(start - 100);
            w.input_at_lead_ms = input_at;
            let world = w.shared();
            // 残す時間を 0 以外にして、10 の後が release(0) になることを見分ける
            let r = exec_opts(
                &world,
                &indicator_policy("lead_ms = 3000\n"),
                &mut StopSet::new(),
                &script(CLICK),
                &lead_opts(5000, false),
            );
            (world, r)
        };
        let (world, r) = run(None);
        let report = r.unwrap();
        assert_eq!(waited_before_run(&world, start, &report), 3000);
        let (world, r) = run(Some(start + 200));
        r.unwrap();
        assert_eq!(world.borrow().sent.len(), 1, "猶予の内の入力では止まらない");

        let (world, r) = run(Some(start + 1500));
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::UserActive);
        assert_eq!(e.kind.exit_code(), 10);
        assert_eq!(e.reason, Some(reason::LEAD_INPUT));
        assert_eq!(
            *detail(&e),
            serde_json::json!({"idle_ms": 1500, "lead_ms": 3000})
        );
        assert_eq!(
            e.hint.as_deref(),
            Some(hint(Hint::LeadUserActive, Surface::Cli))
        );
        assert_lead_refused(&world, &e);
    }

    /// 予告の間に目印のファイル (停止要因) があれば 6 で止まり、枠を残さない
    #[test]
    fn lead_stop_file_releases_without_linger() {
        let world = indicator_world(None).shared();
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(3)));
        let e = exec_opts(
            &world,
            &indicator_policy("lead_ms = 3000\n"),
            &mut stops,
            &script(CLICK),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        assert_lead_refused(&world, &e);
        assert_eq!(world.borrow().now_ms, 4 * STOP_POLL_MS);
    }

    /// 予告の間は時間の上限に数えない: 上限 1500 ms で予告 3000 ms でも、click は通る
    #[test]
    fn lead_does_not_count_toward_max_duration() {
        let world = indicator_world(None).shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("lead_ms = 3000\n[limits]\nmax_duration_ms = 1500\n"),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let report = r.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.executed, 1);
        assert!(report.elapsed_ms < 1500, "{}", report.elapsed_ms);
        assert_eq!(waited_before_run(&world, 0, &report), 3000);
    }

    /// 枠の期限は 予告の間 (待つときだけ) + 時間の上限 + 残す時間 + 余裕
    #[test]
    fn overlay_deadline_covers_lead_and_run() {
        for (shown_for, want) in [
            (None, 3000 + 600_000 + 5000 + OVERLAY_DEADLINE_MARGIN_MS),
            (Some(3000), 600_000 + 5000 + OVERLAY_DEADLINE_MARGIN_MS),
        ] {
            let world = indicator_world(shown_for).shared();
            exec_opts(
                &world,
                &indicator_policy("lead_ms = 3000\n[limits]\nmax_duration_ms = 600000\n"),
                &mut StopSet::new(),
                &script(CLICK),
                &lead_opts(5000, false),
            )
            .unwrap();
            assert_eq!(world.borrow().shows, vec![want], "{shown_for:?}");
        }
    }

    /// sound なら、予告の間に入るときに 1 回鳴らす
    #[test]
    fn overlay_sound_beeps_once_when_waiting() {
        let world = indicator_world(None).shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("sound = true\n"),
            &mut StopSet::new(),
            &script(&format!("{CLICK},{CLICK}")),
        );
        r.unwrap();
        assert_eq!(world.borrow().beeps, 1);
    }

    /// 実行の途中の失敗 (7) でも枠を解き、残す時間はそのまま。途中で人に止められた (6) なら残さない
    #[test]
    fn overlay_released_after_failure() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let world = w.shared();
        let p = policy(&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"));
        let e = exec_opts(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(world.borrow().releases, vec![5000]);

        let mut w = World::new();
        w.user_moves.push((0, Point::new(1500, 900)));
        let world = w.shared();
        let e = exec_opts(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"move","x":10,"y":10},{"type":"move","x":20,"y":20}"#),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        assert_eq!(world.borrow().releases, vec![0]);
    }

    /// 始めのカーソルが読めない (ロック中) close だけの操作列では、枠も予告も出さず、14 にせず close を走らせる
    #[test]
    fn overlay_skipped_when_desktop_unavailable_at_start() {
        let mut w = close_world();
        w.indicator_shown_for = None;
        w.input_desktop = Ok(winlogon());
        w.cursor_fail_after_reads = Some((0, cursor_locked()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &indicator_policy("lead_ms = 3000\n"),
            &mut StopSet::new(),
            &script(CLOSE_A),
        );
        let report = r.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.closed.len(), 1);
        assert_eq!(waited_before_run(&world, 0, &report), 0);
        let w = world.borrow();
        assert!(w.shows.is_empty());
        assert!(w.releases.is_empty());
        assert_eq!(w.indicator_reads, 0);
        assert_eq!(w.close_calls, vec![0x20]);
    }

    // ---- 入力の錠 (別の ai-cu との排他。D-113) ----

    /// 錠の `release` が呼ばれた回数
    fn lock_releases(w: &World) -> usize {
        w.lock_events
            .iter()
            .filter(|e| matches!(e, LockEvent::Release { .. }))
            .count()
    }

    /// 錠を取ってから操作の前の判定 (押されたままのキー) と枠に進み、途中で止まった drag の離す束を送って枠を解いた
    /// 後に錠を解く
    #[test]
    fn input_lock_taken_before_checks_and_released_after_release_batches() {
        let mut w = World::new();
        // ボタンを押した直後 (2 回目の send の後) に利用者が別の場所へ動かした
        w.user_moves.push((1, Point::new(2000, 1500)));
        let world = w.shared();
        let s = script(
            r#"{"type":"drag","from":{"x":10,"y":10},"to":{"x":200,"y":10},"steps":4,"duration_ms":40}"#,
        );
        let e = exec_opts(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &s,
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        let w = world.borrow();
        assert!(
            matches!(
                w.sent.last().unwrap()[..],
                [InputEvent::Button { down: false, .. }]
            ),
            "送信の最後が離す束: {:?}",
            w.sent
        );
        assert_eq!(w.indicator_calls.last(), Some(&"release"));
        assert_eq!(
            w.lock_events,
            vec![
                LockEvent::Acquire {
                    got: true,
                    now_ms: 0,
                    sent: 0,
                    keys_held_calls: 0,
                    indicator_calls: 0,
                },
                LockEvent::Release {
                    now_ms: w.now_ms,
                    sent: w.sent.len(),
                    keys_held_calls: w.keys_held_calls,
                    indicator_calls: w.indicator_calls.len(),
                },
            ],
            "錠は判定と枠より前に取り、離す束と枠の解放の後に解く"
        );
        assert_eq!(w.keys_held_calls, 1, "押されたままのキーの判定は錠の後");
    }

    /// 入力のバッチが無くても、focus だけ、close だけの操作列は画面を変えるので錠を取る
    #[test]
    fn input_lock_taken_for_focus_only_and_close_only() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        w.top = vec![top(0x20, "Notepad.exe", "a.txt - メモ帳")];
        let world = w.shared();
        let p = notepad_policy();
        let (r, _) = exec(
            &world,
            &p,
            &mut StopSet::new(),
            &script(r#"{"type":"focus","process":"notepad.exe"}"#),
        );
        assert_eq!(r.unwrap().focused.len(), 1);
        assert_eq!(world.borrow().lock_try_calls, 1, "focus だけ");
        assert_eq!(lock_releases(&world.borrow()), 1);

        let world = close_world().shared();
        let (r, _) = exec(&world, &p, &mut StopSet::new(), &script(CLOSE_A));
        assert_eq!(r.unwrap().closed.len(), 1);
        assert_eq!(world.borrow().lock_try_calls, 1, "close だけ");
        assert_eq!(lock_releases(&world.borrow()), 1);
    }

    /// 撮影だけ、待ちだけ、wait_for だけの操作列は画面を変えないので錠を取らない
    #[test]
    fn input_lock_not_taken_for_capture_wait_or_read_only_scripts() {
        let wait_for = wait_json("", 0);
        for actions in [SHOT, r#"{"type":"wait","ms":100}"#, wait_for.as_str()] {
            let world = World::new().shared();
            let (_r, _) = exec(
                &world,
                &notepad_policy(),
                &mut StopSet::new(),
                &script(actions),
            );
            let w = world.borrow();
            assert_eq!(w.lock_try_calls, 0, "{actions}");
            assert!(w.lock_events.is_empty(), "{actions}");
        }
    }

    /// 検証で断る操作列 (4) は錠を取らない
    #[test]
    fn input_lock_not_taken_when_validation_refuses() {
        let world = World::new().shared();
        let (r, _) = exec(
            &world,
            &Policy::closed(),
            &mut StopSet::new(),
            &script(CLICK),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::PolicyDenied);
        assert_eq!(world.borrow().lock_try_calls, 0);
    }

    /// 取れない間は 50 ms ごとに試し、4 回目 (始めから 150 ms) で取って送る。取れない間はキーの状態を読まない
    #[test]
    fn input_lock_waits_until_free_then_runs() {
        let start = 1000;
        let mut w = World::new();
        w.now_ms = start;
        w.lock_busy = 3;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        r.unwrap_or_else(|e| panic!("{e}"));
        let w = world.borrow();
        assert_eq!(w.sent.len(), 1);
        assert_eq!(w.lock_try_calls, 4);
        let acquires: Vec<(bool, u64, usize)> = w
            .lock_events
            .iter()
            .filter_map(|e| match *e {
                LockEvent::Acquire {
                    got,
                    now_ms,
                    keys_held_calls,
                    ..
                } => Some((got, now_ms - start, keys_held_calls)),
                LockEvent::Release { .. } => None,
            })
            .collect();
        assert_eq!(
            acquires,
            vec![
                (false, 0, 0),
                (false, STOP_POLL_MS, 0),
                (false, 2 * STOP_POLL_MS, 0),
                (true, 3 * STOP_POLL_MS, 0),
            ]
        );
        assert_eq!(lock_releases(&w), 1);
    }

    /// 錠を待った実行は、枠が十分前から出ていても、TUI (skip_lead) でも、予告の間を縮めず lead_ms をまるごと待つ。
    /// 待たない実行は今までどおり省く。lead_ms が 0 なら待った後も待たない
    #[test]
    fn input_lock_wait_runs_full_lead_even_if_frame_shown() {
        let run = |busy: usize, extra: &str, skip_lead: bool| {
            // 枠は lead_ms より十分前から出ている
            let mut w = indicator_world(Some(u64::MAX));
            w.lock_busy = busy;
            let world = w.shared();
            let report = exec_opts(
                &world,
                &indicator_policy(extra),
                &mut StopSet::new(),
                &script(CLICK),
                &lead_opts(0, skip_lead),
            )
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(world.borrow().sent.len(), 1);
            waited_before_run(&world, 0, &report)
        };
        for skip_lead in [false, true] {
            assert_eq!(run(0, "lead_ms = 3000\n", skip_lead), 0, "{skip_lead}");
            assert_eq!(
                run(1, "lead_ms = 3000\n", skip_lead),
                STOP_POLL_MS + 3000,
                "{skip_lead}"
            );
            assert_eq!(
                run(1, "lead_ms = 0\n", skip_lead),
                STOP_POLL_MS,
                "{skip_lead}"
            );
        }
    }

    /// ずっと取れなければ、30 秒で 16 (`input_busy`) を返し、何も送らない。進み具合を付けず、錠も枠も解かない
    #[test]
    fn input_lock_timeout_is_16_without_sending() {
        let mut w = World::new();
        w.lock_busy = usize::MAX;
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::InputBusy);
        assert_eq!(e.kind.exit_code(), 16);
        let v = e.to_json();
        assert_eq!(v["error"]["code"], "input_busy");
        assert!(
            e.message
                .contains(&format!("{} 秒", INPUT_LOCK_WAIT_MS / 1000)),
            "{}",
            e.message
        );
        let waited = detail(&e)["waited_ms"].as_u64().unwrap();
        assert!(waited >= INPUT_LOCK_WAIT_MS, "{waited}");
        assert!(e.progress.is_none());
        assert!(v.get("executed").is_none());
        assert!(v.get("partial").is_none());
        assert_eq!(e.hint(), ErrorKind::InputBusy.hint());
        let w = world.borrow();
        assert!(w.sent.is_empty());
        assert_eq!(lock_releases(&w), 0);
        assert!(w.shows.is_empty());
        assert!(w.releases.is_empty());
        assert_eq!(w.keys_held_calls, 0);
    }

    /// 錠を待つ間に停止要因が立つと 6 で何も送らず、前の呼び出しの枠を 1 回だけ消す。待った後の予告の間で
    /// 止まる (マウスの移動) ときも、枠の解放は 1 回だけ
    #[test]
    fn input_lock_wait_honors_stop_file() {
        let mut w = World::new();
        w.lock_busy = usize::MAX;
        let world = w.shared();
        let mut stops = StopSet::new();
        stops.push(Box::new(StopAfterPolls(2)));
        let e = exec_opts(
            &world,
            &policy(ENABLED),
            &mut stops,
            &script(CLICK),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert!(e.progress.is_none());
        {
            let w = world.borrow();
            assert!(w.sent.is_empty());
            assert_eq!(w.now_ms, 3 * STOP_POLL_MS, "3 回目の待ちの後の確認で止まる");
            assert!(w.shows.is_empty());
            assert_eq!(w.releases, vec![0], "前の呼び出しの枠を 1 回だけ消す");
            assert_eq!(lock_releases(&w), 0);
        }

        let mut w = World::new();
        w.lock_busy = 1;
        // 読み 0 は開始の読み。予告の間の 2 回目の確認の前に人が動かす
        w.cursor_moves_after_reads = vec![(2, Point::new(1500, 900))];
        let world = w.shared();
        let e = exec_opts(
            &world,
            &indicator_policy("lead_ms = 3000\n"),
            &mut StopSet::new(),
            &script(CLICK),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped, "{e}");
        assert_lead_refused(&world, &e);
        assert_eq!(lock_releases(&world.borrow()), 1);
    }

    /// 錠を待つ間はマウスを読まない (動かしているのは前の ai-cu で、見ると誤って 6 になる)。待つ間に
    /// カーソルが動いていても、監視は錠を取った後の始めの読みから始まり、6 にならずに送る。
    /// 取れないまま 16 で終わる待ちでも、カーソルを 1 度も読まない
    #[test]
    fn input_lock_wait_ignores_mouse_moves() {
        let mut w = World::new();
        w.lock_busy = 3;
        // 待つ間に前の ai-cu が動かした: 始めの読みの時点で、もう遠くにある
        w.cursor_moves_after_reads = vec![(0, Point::new(3000, 2000))];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        r.unwrap_or_else(|e| panic!("{e}"));
        {
            let w = world.borrow();
            assert_eq!(w.sent.len(), 1);
            assert_eq!(w.lock_try_calls, 4);
        }

        let mut w = World::new();
        w.lock_busy = usize::MAX;
        w.cursor_moves_after_reads = vec![(0, Point::new(3000, 2000)), (1, Point::new(10, 10))];
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        assert_eq!(r.unwrap_err().kind, ErrorKind::InputBusy);
        let w = world.borrow();
        assert_eq!(w.cursor_reads, 0, "錠を待つ間にマウスを読まない");
        assert!(w.sent.is_empty());
    }

    /// 実行の途中の失敗 (7) と停止 (6) の後も錠を 1 回解く
    #[test]
    fn input_lock_released_after_failure_and_stop() {
        let mut w = World::new();
        w.foreground = Some(terminal());
        let world = w.shared();
        let e = exec_opts(
            &world,
            &notepad_policy(),
            &mut StopSet::new(),
            &script(r#"{"type":"key","keys":"a"}"#),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::ForegroundGuard);
        assert_eq!(lock_releases(&world.borrow()), 1);

        let mut w = World::new();
        w.user_moves.push((0, Point::new(1500, 900)));
        let world = w.shared();
        let e = exec_opts(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(r#"{"type":"move","x":10,"y":10},{"type":"move","x":20,"y":20}"#),
            &lead_opts(5000, false),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Stopped);
        assert_eq!(lock_releases(&world.borrow()), 1);
    }

    /// 錠の試みが失敗したら 8 で何も送らない (守りの層なので断る向き)
    #[test]
    fn input_lock_error_is_8_without_sending() {
        let mut w = World::new();
        w.lock_err = Some(BackendError::Os("CreateMutexExW が失敗".into()));
        let world = w.shared();
        let (r, _) = exec(
            &world,
            &policy(ENABLED),
            &mut StopSet::new(),
            &script(CLICK),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::OsError);
        assert_eq!(e.kind.exit_code(), 8);
        let w = world.borrow();
        assert!(w.sent.is_empty());
        assert_eq!(w.lock_try_calls, 1);
        assert!(w.lock_events.is_empty());
        assert!(w.shows.is_empty());
        assert_eq!(w.keys_held_calls, 0);
    }
}
