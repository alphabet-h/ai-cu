//! UI Automation (COM) の境界 (D-054)。要素の名前、種類、矩形、文字列を読むだけで、入力は送らない。
//! find の経路ではフォーカスと状態 (Toggle / SelectionItem / ExpandCollapse) とパスワードの要素か (IsPassword) も読む
//! (D-089、D-090)。文字列を読む問い合わせでは IsPassword を先に読み、真の要素の文字列は読まずに飛ばす (D-090)。
//! 入力の判定のために、システム全体のキーボードフォーカスの要素の IsPassword も読む (`focused_is_password`。D-090)。
//! 呼び出しごとに専用のスレッドを立てて MTA で COM を初期化する (UIA のクライアントは窓を持たない MTA の
//! スレッドから呼ぶ。Microsoft Learn「Understanding Threading Issues」)。主スレッドでは COM を初期化しない。

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_TIMEOUT, HWND, RECT, S_OK};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement,
    IUIAutomationExpandCollapsePattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTextPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, TreeScope_Descendants, UIA_E_TIMEOUT, UIA_ExpandCollapsePatternId,
    UIA_SelectionItemPatternId, UIA_TextPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};
use windows::core::HRESULT;

use crate::elements::{
    ElementQuery, Elements, ReadError, TreeSource, UiElement, UiTree, WalkStep, control_type_name,
    expand_collapse_state_name, skip_or_abort, toggle_state_name, walk_tree,
};
use crate::geometry::Rect;

/// UIA の層の誤り (D-104)。時間切れ (上限つきの待ちの時間切れ、返らないスレッドの上限、UIA 自身の時間切れ (どの HRESULT かは `is_uia_timeout`。フォーカスの判定と、要素と木の読み (D-110))) を他の失敗と分け、
/// 装置の層で `BackendError::TimedOut` に写す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiaError {
    /// 読み取りの失敗 (今までの `String` の誤り)
    Failed(String),
    /// 上限の時間で返らなかったか、返らないスレッドが上限の本数だけ残っているか、UIA 自身が時間切れを返した (どの HRESULT かは `is_uia_timeout`。フォーカスの判定と、要素と木の読み。要素と木の読みは最初の時間切れで残りを読まずに返る (D-110))
    TimedOut(String),
}

/// 要素と木の読みの失敗を UIA の層の誤りに写す (D-110)。種類はそのまま (`Failed` は `Failed`、`TimedOut` は `TimedOut`)
impl From<ReadError> for UiaError {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::Failed(m) => UiaError::Failed(m),
            ReadError::TimedOut(m) => UiaError::TimedOut(m),
        }
    }
}

/// 要素と木の読み取り (`elements`、`tree`) を待つ上限。UIA の既定の処理の時間切れ (20 秒) で 1 回は正規に返れ、
/// Codex の道具の既定の時間切れ (60 秒) より短い (D-104)
const UIA_READ_LIMIT: Duration = Duration::from_secs(30);

/// 終わっていない UIA のスレッドの本数の上限。これだけ残っていれば新しく立てない (固まったスレッドを積もらせない。D-104)
const MAX_INFLIGHT_UIA_THREADS: usize = 4;

/// 要素と木の読み取り (`elements`、`tree`) の終わっていないスレッドの数 (フォーカスの判定とは別に数える。D-104)
static UIA_READ_INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// 入口ごとの待ち方の組 (スレッドの名前、上限、数える場所)。COM に依らないので配線をテストで撃てる (D-104)
struct Bounds {
    name: &'static str,
    limit: Duration,
    inflight: &'static AtomicUsize,
}

/// 要素と木の読み取りの組 (`UIA_READ_LIMIT`、`UIA_READ_INFLIGHT`)
fn read_bounds(name: &'static str) -> Bounds {
    Bounds {
        name,
        limit: UIA_READ_LIMIT,
        inflight: &UIA_READ_INFLIGHT,
    }
}

/// 終わっていないスレッドの数を、スレッドの中の Drop で 1 減らす (panic の巻き戻しでも減る。D-104)
struct InflightGuard(&'static AtomicUsize);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 秒を末尾の 0 を出さずに書く (`0.1`、`4`、`30`)
fn secs_text(d: Duration) -> String {
    let ms = d.as_millis();
    if ms.is_multiple_of(1000) {
        return (ms / 1000).to_string();
    }
    let s = format!("{}.{:03}", ms / 1000, ms % 1000);
    s.trim_end_matches('0').to_string()
}

/// `f` を名前 `name` の専用のスレッドで走らせ、`limit` まで待つ (D-104)。時間切れならスレッドを切り離して
/// `TimedOut` を返す (切り離したスレッドは自分で COM の後始末をして終わる)。`inflight` の数が `max_inflight` 以上
/// なら、立てずにすぐ `TimedOut` を返す。呼び出しは直列が前提で、比べてから足す間の競合は数が一時的に 1 超える程度
fn run_bounded<T: Send + 'static>(
    name: &str,
    limit: Duration,
    inflight: &'static AtomicUsize,
    max_inflight: usize,
    f: impl FnOnce() -> Result<T, UiaError> + Send + 'static,
) -> Result<T, UiaError> {
    // 比べて足すのを 1 つの操作にする (呼び出しは今は直列だが、並列になっても上限を超えて立てない)
    if let Err(n) = inflight.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
        (n < max_inflight).then_some(n + 1)
    }) {
        return Err(UiaError::TimedOut(format!(
            "前の UIA の読み取りのスレッドが {n} 本まだ返らないので、新しく読まない (上限 {max_inflight})。\
             応答しないアプリのスレッドはプロセスが終わるまで残りうるので、ai-cu (MCP サーバー) を起動し直す"
        )));
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            // 最初に作り、閉包の panic の巻き戻しでも数を戻す
            let _guard = InflightGuard(inflight);
            // 時間切れの後は受け手がもう無いので、送れなくても捨てる
            let _ = tx.send(f());
        });
    let handle = match spawned {
        Ok(h) => h,
        Err(e) => {
            inflight.fetch_sub(1, Ordering::SeqCst);
            return Err(UiaError::Failed(format!("UIA のスレッドを作れない: {e}")));
        }
    };
    match rx.recv_timeout(limit) {
        Ok(r) => {
            // 送った後は終わりかけなので待つ。panic はもう起きない (結果は送り済み)
            let _ = handle.join();
            r
        }
        Err(RecvTimeoutError::Timeout) => {
            drop(handle);
            Err(UiaError::TimedOut(format!(
                "UIA の読み取りが {} 秒で時間切れ (応答しないアプリの恐れ)。スレッドは切り離した。\
                 同じアプリをすぐ読み直すと固まったスレッドが増える",
                secs_text(limit)
            )))
        }
        Err(RecvTimeoutError::Disconnected) => {
            // 送らずに送り手が drop された = 閉包が panic した
            let _ = handle.join();
            Err(UiaError::Failed("UIA のスレッドが異常終了した".into()))
        }
    }
}

/// hwnd が 0 のときの文。`elements`、`tree` と `root_element` の 3 か所の守りで共有し、文がずれないようにする (D-109)
const HWND_ZERO: &str = "hwnd が 0 (無効なウィンドウ)";

/// `hwnd` のウィンドウ自身とその子孫 (コントロールビュー) の要素を、`query` で絞って返す (木の順)。
/// 取得の後に消えた要素は飛ばして `skipped` に数える。待ちは `UIA_READ_LIMIT` まで (D-104)。
/// 窓が固まると、読みの中で最初に UIA が時間切れを返した所で残りを読まずに `TimedOut` を返す (根を読んだ後なら
/// 文の頭は `CHILD_READ_CONTEXT`。時間切れの子を `skipped` に数えたり、属性を既定値にしたりはしない)。待ちは UIA の
/// 1 回分で、そのうちに明ければ読める (縮めた待ちでは、根を読んだ後に固まっても約 1 秒で `TimedOut` になり、
/// 既定の待ちでは 6 秒の固まりを待って読めた。測った事実は docs/design-notes.md の Windows の入力の節、D-109、D-110)
pub fn elements(hwnd: usize, query: &ElementQuery) -> Result<Elements, UiaError> {
    if hwnd == 0 {
        return Err(UiaError::Failed(HWND_ZERO.into()));
    }
    let q = query.clone();
    let b = read_bounds("ai-cu-uia");
    run_bounded(
        b.name,
        b.limit,
        b.inflight,
        MAX_INFLIGHT_UIA_THREADS,
        move || on_mta_thread(hwnd, &q).map_err(UiaError::from),
    )
}

/// `CoInitializeEx` と `CoUninitialize` の対を構造で保証する (panic で巻き戻るときも Drop で呼ぶ)。
/// COM のオブジェクトより先に作る。ローカルは宣言の逆順に drop されるので、オブジェクトの Release が先になる。
/// `CoUninitialize` は初期化したスレッドで呼ぶ必要があるので、`!Send` にして別のスレッドへ動かせないようにする
struct ComApartment {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl ComApartment {
    fn init_mta() -> Result<ComApartment, String> {
        // SAFETY: 呼び出したスレッドでは他に COM を使っていない。MTA で初期化し、成功 (S_FALSE を含む) なら
        //         Drop で必ず CoUninitialize と対にする。失敗 (RPC_E_CHANGED_MODE など) ならガードを作らず、
        //         対にもしない。
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(format!("CoInitializeEx: {hr}"));
        }
        Ok(ComApartment {
            _not_send: std::marker::PhantomData,
        })
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: init_mta で CoInitializeEx が成功したスレッドで、1 回だけ呼ぶ。このガードより後に作った COM の
        //         オブジェクトは、宣言の逆順の drop (panic の巻き戻しを含む) で先に Release されている。
        unsafe { CoUninitialize() };
    }
}

fn on_mta_thread(hwnd: usize, query: &ElementQuery) -> Result<Elements, ReadError> {
    let _apartment = ComApartment::init_mta().map_err(ReadError::Failed)?;
    query_elements(hwnd, query)
}

/// `CoInitializeEx` 済みのスレッドで、UIA の入口と `hwnd` の窓の要素を作る (本番の要素と木の入口。既定の待ち)。
/// 入口を作れないときは `Failed`、根の Err は `root_element` のまま (時間切れは `TimedOut`。D-110)
fn automation_and_root(hwnd: usize) -> Result<(IUIAutomation, IUIAutomationElement), ReadError> {
    let automation = default_automation().map_err(ReadError::Failed)?;
    let root = root_element(&automation, hwnd)?;
    Ok((automation, root))
}

/// `CoInitializeEx` 済みのスレッドで、`CUIAutomation` の UIA の入口を既定の待ちのまま作る (D-109。作り方と文を 1 か所にする)
fn default_automation() -> Result<IUIAutomation, String> {
    // SAFETY: CoInitializeEx 済みのスレッド (呼び手はどれも ComApartment を作った後に呼ぶ)。CUIAutomation は
    //         in-process のサーバーで、返るインターフェースは windows クレートが参照を数えて drop で Release する。
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("CUIAutomation を作れない: {e}"))
}

/// `automation` で `hwnd` の窓の要素を作る (D-109。本番と計測の入口で共有する)。hwnd が 0 なら UIA を呼ばずに Err
/// (本番の `elements` と `tree` は先に断るのでここには来ない。計測の入口のための守り)。
/// `ElementFromHandle` の UIA の時間切れは `TimedOut`、他の失敗は `Failed` (`read_error`。D-110)
fn root_element(
    automation: &IUIAutomation,
    hwnd: usize,
) -> Result<IUIAutomationElement, ReadError> {
    if hwnd == 0 {
        return Err(ReadError::Failed(HWND_ZERO.into()));
    }
    // SAFETY: automation は同じスレッドで作った有効な入口。hwnd は 0 ではなく、呼び出し側が find_window などで得た値。
    //         無効なら Err になるだけで、メモリには触れない。
    unsafe { automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void)) }
        .map_err(|e| read_error(&format!("ElementFromHandle({hwnd:#x})"), &e))
}

/// フォーカスの要素の読み取りで UIA が待つ上限 (ミリ秒)。既定は Connection 2 秒・Transaction 20 秒で
/// (Microsoft Learn「IUIAutomation2::get_TransactionTimeout」)、応答しないアプリで送る前に長く止まらないように縮める (D-090)。
/// 要素と木の計測の入口 (`probe_elements`、`probe_tree`) も、縮めた待ちとしてこの値を使う (D-109)
const FOCUS_TIMEOUT_MS: u32 = 1000;

/// フォーカスの判定 (`focused_is_password`) を待つ上限。接続 1 回と処理 2 回 (`GetFocusedElement` と
/// `CurrentIsPassword`) の UIA の待ち (各 `FOCUS_TIMEOUT_MS`) に余裕を足し、UIA 自身の時間切れが先に返れるようにする (D-104)
const FOCUS_READ_LIMIT: Duration = Duration::from_millis(3 * FOCUS_TIMEOUT_MS as u64 + 1000);

/// フォーカスの判定の終わっていないスレッドの数 (要素と木とは別に数え、別の窓が固まっても判定に響かない。D-104)
static UIA_FOCUS_INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// フォーカスの判定の組 (`FOCUS_READ_LIMIT`、`UIA_FOCUS_INFLIGHT`)
fn focus_bounds() -> Bounds {
    Bounds {
        name: "ai-cu-uia-focus",
        limit: FOCUS_READ_LIMIT,
        inflight: &UIA_FOCUS_INFLIGHT,
    }
}

/// HRESULT が UIA 自身の時間切れか (D-104)。フォーカスの判定ではこれを他の失敗と分けて断り、要素と木の読みでは
/// これを見たら残りを読まずに読み全体を `TimedOut` にする (`read_error`、`attr`、`walker_step` がこの判定だけを使う。
/// D-110)。`UIA_E_TIMEOUT` (Microsoft Learn の
/// UI Automation Error Codes) は測った: 自プロセスのフィクスチャのスレッドを眠らせると、`GetFocusedElement` も
/// `CurrentIsPassword` も縮めた待ち (約 1 秒) で `UIA_E_TIMEOUT` (0x80131505) を返した (D-108、docs/design-notes.md)。
/// Win32 の `ERROR_TIMEOUT` を包んだ HRESULT (0x800705B4) はその場では出なかったが、接続の段階や別プロセスの窓で
/// 返りうるかは測っていない (推定。`put_ConnectionTimeout` の頁は切れたときの HRESULT を書いていない。win32-reviewer
/// の指摘) ので、外さずに含める
fn is_uia_timeout(code: HRESULT) -> bool {
    code == HRESULT(UIA_E_TIMEOUT as i32) || code == HRESULT::from_win32(ERROR_TIMEOUT.0)
}

/// 要素と木の読みの UIA の呼び出し `what` の Err を振り分ける純関数 (D-110)。UIA の時間切れ (`is_uia_timeout`) は
/// `TimedOut` (文はフォーカスの判定の文の形に、対象のアプリの固まりと読める 1 句を足す)、他は `Failed` (今までの文の形)
fn read_error(what: &str, e: &windows::core::Error) -> ReadError {
    let code = e.code();
    if is_uia_timeout(code) {
        ReadError::TimedOut(format!(
            "{what} が UIA の時間切れ ({code})。対象のアプリが応答していない"
        ))
    } else {
        ReadError::Failed(format!("{what}: {e}"))
    }
}

/// 要素の属性やパターンの読み (UIA の呼び出し `what`) の結果を振り分ける純関数 (D-110)。Ok は `Some`、UIA の時間切れは
/// 読み全体を打ち切る Err (`read_error`)、他の Err (要素が消えた、パターンを持たない S_OK の `Error::empty` など) は
/// `None` (呼び手が既定値にするか「無い」とする。今までどおり)
fn attr<T>(what: &str, r: windows::core::Result<T>) -> Result<Option<T>, ReadError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if is_uia_timeout(e.code()) => Err(read_error(what, &e)),
        Err(_) => Ok(None),
    }
}

/// 要素の読みの子の繰り返しで、時間切れで打ち切ったときの文の頭 (D-110。根の時間切れには付けない)
const CHILD_READ_CONTEXT: &str = "子の要素の読み: ";

/// システム全体のキーボードフォーカスの要素が IsPassword か (D-090)。`elements` と同じく専用のスレッドで MTA を
/// 初期化して読む。フォーカスの要素が無い (null) か IsPassword が読めなければ `Ok(None)`。
/// 待ちは `FOCUS_READ_LIMIT` まで。時間切れ (UIA 自身の時間切れを含む) は `TimedOut` (D-104)。
/// hwnd を取らないので、窓で絞らない (呼び出し側の門は `backend::focused_password_allowed`)
pub fn focused_is_password() -> Result<Option<bool>, UiaError> {
    let b = focus_bounds();
    run_bounded(
        b.name,
        b.limit,
        b.inflight,
        MAX_INFLIGHT_UIA_THREADS,
        focused_is_password_on_mta_thread,
    )
}

fn focused_is_password_on_mta_thread() -> Result<Option<bool>, UiaError> {
    let raw = read_focus_raw(|| {}).map_err(UiaError::Failed)?;
    classify_focus(&raw)
}

/// 判定の 1 つの UIA の呼び出し (`GetFocusedElement`) の結果 (D-108)。`code` は Err の HRESULT (成功なら None。
/// S_OK は「要素が無い」)、`ms` は呼び出しの経過 (ミリ秒)
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeCall {
    pub code: Option<i32>,
    pub ms: u64,
}

/// `CurrentIsPassword` の結果 (D-108)。`result` は Ok(値) か Err(HRESULT)、`ms` は呼び出しの経過 (ミリ秒)
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordRead {
    pub ms: u64,
    pub result: Result<bool, i32>,
}

/// `read_focus_raw` の結果 (D-108)。COM のオブジェクトは持たない (スレッドの外へ出せる値だけ)
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusRaw {
    /// `CUIAutomation8` を作れて、接続と処理の待ちの setter がどちらも成功したか
    pub short_timeouts: bool,
    /// `GetFocusedElement` (Err の code が S_OK なら要素が無い)
    pub focused: ProbeCall,
    /// `GetFocusedElement` が要素を返したときだけ Some (型で矛盾した状態を作らない)
    pub is_password: Option<PasswordRead>,
}

/// 計測の入口 (`probe_focused_password`) の戻り (D-108)。生の読みと、それを本番と同じ `classify_focus` に通した結果、
/// 本番の判定の上限 (`FOCUS_READ_LIMIT`) と縮めた待ち (`FOCUS_TIMEOUT_MS`) の値 (テストが私的な定数を写さずに比べるため)
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusProbe {
    pub raw: FocusRaw,
    pub classified: Result<Option<bool>, UiaError>,
    pub focus_limit_ms: u64,
    pub short_timeout_ms: u64,
}

/// `GetFocusedElement` の時間切れでない失敗の文 (D-108)。`text` は `HRESULT::message()` の文で、系の文が引けない
/// HRESULT では空になるので、空なら HRESULT だけにする (末尾に空白を残さない)
fn focus_failed_text(hr: HRESULT, text: &str) -> String {
    if text.is_empty() {
        format!("GetFocusedElement: {hr}")
    } else {
        format!("GetFocusedElement: {hr} {text}")
    }
}

/// 経過をミリ秒にする (端数は切り捨て)
fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// 判定の生の読み (D-108)。COM を MTA で初期化し、`GetFocusedElement` (経過を測る) → `between()` → 要素があれば
/// `CurrentIsPassword` (経過を測る) の順に呼び、HRESULT と経過を集める。本番 (`between` は何もしない) と計測の入口が
/// 同じ呼び出しを同じ順で行う。COM の初期化や UIA の入口を作れなければ Err。
/// `between` は COM を MTA で初期化したこのスレッドで、UIA のオブジェクトを持ったまま走る。COM のオブジェクトを
/// スレッドの外へ持ち出さない閉包だけを渡す (計測の入口では、上限で切り離された後に走りうる)
fn read_focus_raw(between: impl FnOnce()) -> Result<FocusRaw, String> {
    let _apartment = ComApartment::init_mta()?;
    let (automation, short_timeouts) = automation_with_short_timeouts()?;
    let start = Instant::now();
    // SAFETY: CoInitializeEx 済みのスレッドで作った有効な automation オブジェクト。フォーカスの要素が無ければ
    //         S_OK と null が windows-core の Error::empty (code が S_OK) に変換されて返るだけ (walker_step と同じ)。
    let got = unsafe { automation.GetFocusedElement() };
    let ms = elapsed_ms(start);
    let (focused, element) = match got {
        Ok(el) => (ProbeCall { code: None, ms }, Some(el)),
        Err(e) => (
            ProbeCall {
                code: Some(e.code().0),
                ms,
            },
            None,
        ),
    };
    between();
    let is_password = element.map(|el| {
        let start = Instant::now();
        // SAFETY: el は同じスレッドで得た有効な要素 (`_apartment` より後に作り、この閉包の終わりで先に Release される)。
        //         getter は BOOL を値で返すだけで、要素が消えていれば Err (UIA_E_ELEMENTNOTAVAILABLE) に、UIA の待ちが
        //         切れれば Err (UIA_E_TIMEOUT など) になるだけ (read_password と同じ)。
        let r = unsafe { el.CurrentIsPassword() };
        PasswordRead {
            ms: elapsed_ms(start),
            result: r.map(|b| b.as_bool()).map_err(|e| e.code().0),
        }
    });
    Ok(FocusRaw {
        short_timeouts,
        focused,
        is_password,
    })
}

/// 生の読みを判定の戻り値に振り分ける純関数 (D-104、D-108)。`GetFocusedElement` の Err は、S_OK (要素が無い) なら
/// `Ok(None)`、UIA の時間切れ (`is_uia_timeout`) なら `TimedOut`、他は `Failed` (文は HRESULT と
/// `HRESULT::message()`。`windows::core::Error::from(HRESULT)` は `GetErrorInfo` を呼んでスレッドの誤り情報を
/// 消費するので使わない)。要素があれば、`CurrentIsPassword` の Ok(b) は `Ok(Some(b))`、時間切れは `TimedOut`、
/// 他の Err は `Ok(None)` (読めなければ通す)。要素があるのに IsPassword の読みが無い形も `Ok(None)`
fn classify_focus(raw: &FocusRaw) -> Result<Option<bool>, UiaError> {
    if let Some(code) = raw.focused.code {
        let hr = HRESULT(code);
        if hr == S_OK {
            return Ok(None);
        }
        if is_uia_timeout(hr) {
            return Err(UiaError::TimedOut(format!(
                "GetFocusedElement が UIA の時間切れ ({hr})"
            )));
        }
        return Err(UiaError::Failed(focus_failed_text(hr, &hr.message())));
    }
    match &raw.is_password {
        None => Ok(None),
        Some(PasswordRead { result: Ok(b), .. }) => Ok(Some(*b)),
        Some(PasswordRead {
            result: Err(code), ..
        }) if is_uia_timeout(HRESULT(*code)) => Err(UiaError::TimedOut(format!(
            "CurrentIsPassword が UIA の時間切れ ({})",
            HRESULT(*code)
        ))),
        Some(_) => Ok(None),
    }
}

/// 計測の入口 (`probe_focused_password`、`probe_elements`、`probe_tree`) を待つ上限。UIA の既定の接続 2 秒・処理 20 秒より、
/// 縮めた待ち (1 秒) で返る想定に十分で、本番の判定の上限 (4 秒) を超える待ちも記録できる (D-108)。
/// 要素と木の計測では本番の上限 (`UIA_READ_LIMIT` の 30 秒) より短い (フィクスチャのウォッチドッグ 30 秒の中で返るため。D-109)
const PROBE_READ_LIMIT: Duration = Duration::from_secs(20);

/// 計測の入口 (`probe_focused_password`、`probe_elements`、`probe_tree`) の終わっていないスレッドの数
/// (判定とも要素と木とも別に数える。D-108、D-109)
static UIA_PROBE_INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// 計測の入口全体 (`probe_focused_password`、`probe_elements`、`probe_tree`) の組
/// (`PROBE_READ_LIMIT`、`UIA_PROBE_INFLIGHT`。D-108、D-109)
fn probe_bounds() -> Bounds {
    Bounds {
        name: "ai-cu-uia-probe",
        limit: PROBE_READ_LIMIT,
        inflight: &UIA_PROBE_INFLIGHT,
    }
}

/// 実機の計測用 (D-108)。本番からは呼ばない。本番の判定 (`focused_is_password`) と同じ生の読みを、2 つの呼び出しの
/// 間に `between` を挟んで行い、生の読みと本番と同じ振り分けの結果を返す。待ちは `PROBE_READ_LIMIT` まで
#[doc(hidden)]
pub fn probe_focused_password(
    between: impl FnOnce() + Send + 'static,
) -> Result<FocusProbe, UiaError> {
    let b = probe_bounds();
    run_bounded(
        b.name,
        b.limit,
        b.inflight,
        MAX_INFLIGHT_UIA_THREADS,
        move || {
            let raw = read_focus_raw(between).map_err(UiaError::Failed)?;
            let classified = classify_focus(&raw);
            Ok(FocusProbe {
                raw,
                classified,
                focus_limit_ms: u64::try_from(FOCUS_READ_LIMIT.as_millis()).unwrap_or(u64::MAX),
                short_timeout_ms: u64::from(FOCUS_TIMEOUT_MS),
            })
        },
    )
}

/// 要素と木の計測の入口 (`probe_elements`、`probe_tree`) の戻り (D-109)。COM のオブジェクトは持たない。
/// `short_timeouts` は縮めた待ちの入口 (`automation_with_short_timeouts`) を使い、待ちを縮められたか (既定の待ちの入口なら偽)。
/// `ms` は COM を初期化した後から本体が返るまで (入口と根を作る時間を含む。ミリ秒の切り捨て)。
/// `result` は本体の結果 (入口か根を作れないとき、または読みの途中で UIA が時間切れを返したときの Err (D-110)。
/// 部分的な結果 (`skipped` など) もそのまま見られる)
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ReadProbe<T> {
    pub short_timeouts: bool,
    pub ms: u64,
    pub result: Result<T, ReadError>,
}

/// 計測の入口の UIA の入口。`short` なら縮めた待ちの入口 (`automation_with_short_timeouts`)、偽なら本番の要素と木と
/// 同じ既定の待ちの入口 (`default_automation`。縮めたかは偽) (D-109)
fn probe_automation(short: bool) -> Result<(IUIAutomation, bool), String> {
    if short {
        automation_with_short_timeouts()
    } else {
        default_automation().map(|a| (a, false))
    }
}

/// 実機の計測用 (D-109)。本番からは呼ばない。本番の要素の読み (`elements`) と同じ本体 (`query_elements_with`) を、
/// 根の要素を読んだ後・子の繰り返しの前に `between` を挟んで行い、経過と結果を返す。入口は `short` で選ぶ
/// (`probe_automation`)。待ちは `PROBE_READ_LIMIT` まで。入口か根を作れないとき、または読みの途中で UIA が時間切れを
/// 返したとき (D-110) は `result` を Err にして返し、
/// `UiaError` は COM の初期化の失敗と `run_bounded` の失敗 (上限の時間切れと本数の上限は `TimedOut`、スレッドを作れない
/// ときと panic は `Failed`) だけ。`between` の約束は `query_elements_with` のとおり
#[doc(hidden)]
pub fn probe_elements(
    hwnd: usize,
    query: &ElementQuery,
    short: bool,
    between: impl FnOnce() + Send + 'static,
) -> Result<ReadProbe<Elements>, UiaError> {
    let q = query.clone();
    let b = probe_bounds();
    run_bounded(
        b.name,
        b.limit,
        b.inflight,
        MAX_INFLIGHT_UIA_THREADS,
        move || {
            let _apartment = ComApartment::init_mta().map_err(UiaError::Failed)?;
            let start = Instant::now();
            // 入口と根はこの腕の中の局所で、`_apartment` より先に Release される。
            // 入口を作れないときは Failed、根と要素の読みの Err は本番と同じ ReadError のまま (D-110)
            let (short_timeouts, result) = match probe_automation(short) {
                Err(e) => (false, Err(ReadError::Failed(e))),
                Ok((automation, short_timeouts)) => (
                    short_timeouts,
                    root_element(&automation, hwnd)
                        .and_then(|root| query_elements_with(&automation, &root, &q, between)),
                ),
            };
            Ok(ReadProbe {
                short_timeouts,
                ms: elapsed_ms(start),
                result,
            })
        },
    )
}

/// 実機の計測用 (D-109)。本番からは呼ばない。本番の木 (`tree`) と同じ本体 (`tree_with`) を、根の最初の子の読み
/// (2 回目の `describe`) の頭で `before_child_read` を 1 回だけ呼んで行い、経過と結果を返す。入口は `short` で選ぶ
/// (`probe_automation`)。待ちは `PROBE_READ_LIMIT` まで。入口か根を作れないとき、または読みの途中で UIA が時間切れを
/// 返したとき (D-110) は `result` を Err にして返し、
/// `UiaError` は COM の初期化の失敗と `run_bounded` の失敗 (上限の時間切れと本数の上限は `TimedOut`、スレッドを作れない
/// ときと panic は `Failed`) だけ。`before_child_read` の約束は `tree_with` のとおり
#[doc(hidden)]
pub fn probe_tree(
    hwnd: usize,
    max_depth: usize,
    max_nodes: usize,
    short: bool,
    before_child_read: impl FnOnce() + Send + 'static,
) -> Result<ReadProbe<UiTree>, UiaError> {
    let b = probe_bounds();
    run_bounded(
        b.name,
        b.limit,
        b.inflight,
        MAX_INFLIGHT_UIA_THREADS,
        move || {
            let _apartment = ComApartment::init_mta().map_err(UiaError::Failed)?;
            let start = Instant::now();
            // 入口と根はこの腕の中の局所 (根は tree_with に移る) で、`_apartment` より先に Release される。
            // 入口を作れないときは Failed、根と木の読みの Err は本番と同じ ReadError のまま (D-110)
            let (short_timeouts, result) = match probe_automation(short) {
                Err(e) => (false, Err(ReadError::Failed(e))),
                Ok((automation, short_timeouts)) => (
                    short_timeouts,
                    root_element(&automation, hwnd).and_then(|root| {
                        tree_with(
                            &automation,
                            hwnd,
                            root,
                            max_depth,
                            max_nodes,
                            Some(Box::new(before_child_read)),
                        )
                    }),
                ),
            };
            Ok(ReadProbe {
                short_timeouts,
                ms: elapsed_ms(start),
                result,
            })
        },
    )
}

/// `CUIAutomation8` の `IUIAutomation2` を作り、接続と応答の待ちを `FOCUS_TIMEOUT_MS` にする。作れなければ
/// (古い OS など) `CUIAutomation` の `IUIAutomation` (`default_automation`) を既定の待ちのまま使う。待ちを縮められなくても
/// 続ける (D-090)。2 つ目の値は待ちを縮められたか (`CUIAutomation8` を作れて、2 つの setter がどちらも成功したときだけ真。D-108)。
/// 要素と木の計測の入口 (`probe_elements`、`probe_tree`) も、縮めた待ちの入口としてこれを使う (D-109)
fn automation_with_short_timeouts() -> Result<(IUIAutomation, bool), String> {
    // SAFETY: CoInitializeEx 済みのスレッド。CUIAutomation8 は in-process のサーバーで、IID は生成コードが
    //         IUIAutomation2::IID を渡す。作れなければ Err になるだけで、返るインターフェースは drop で Release される。
    let a2: windows::core::Result<IUIAutomation2> =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) };
    if let Ok(a2) = a2 {
        // SAFETY: a2 は有効な IUIAutomation2。setter はミリ秒の u32 を値で渡すだけ。失敗しても既定の待ちのまま
        //         (成否は縮めたかの値に入れるだけ)。
        let (connection, transaction) = unsafe {
            (
                a2.SetConnectionTimeout(FOCUS_TIMEOUT_MS).is_ok(),
                a2.SetTransactionTimeout(FOCUS_TIMEOUT_MS).is_ok(),
            )
        };
        // IUIAutomation2 は IUIAutomation を継ぐ (interface_hierarchy の From。QueryInterface を経ず参照を移す)
        return Ok((IUIAutomation::from(a2), connection && transaction));
    }
    // 代わりの経路の作り方と文は default_automation の 1 か所 (その SAFETY の文のとおり)
    default_automation().map(|a| (a, false))
}

/// `hwnd` の窓の要素から、コントロールビューの木を前順に深さで切ってたどる (D-084)。たどり方の判定は
/// `elements::walk_tree` が持ち、ここは UIA の木の読み方 (`UiaTreeSource`) だけを渡す。待ちは `UIA_READ_LIMIT` まで (D-104)。
/// 窓が固まると、たどる中で最初に UIA が時間切れを返した所で残りを読まずに `TimedOut` を返す (文の頭の付け方は
/// `elements::walk_tree` の doc のとおりで、根の子の取得の時間切れには付かない。`elements` と同じく、時間切れの要素を `skipped` に数えたり属性を既定値にしたりはしない)。
/// 縮めた待ちでは、根の子を取れた後に固まっても約 1 秒で `TimedOut` になった (測った事実は docs/design-notes.md の
/// Windows の入力の節、D-109、D-110)
pub fn tree(hwnd: usize, max_depth: usize, max_nodes: usize) -> Result<UiTree, UiaError> {
    if hwnd == 0 {
        return Err(UiaError::Failed(HWND_ZERO.into()));
    }
    let b = read_bounds("ai-cu-uia-tree");
    run_bounded(
        b.name,
        b.limit,
        b.inflight,
        MAX_INFLIGHT_UIA_THREADS,
        move || tree_on_mta_thread(hwnd, max_depth, max_nodes).map_err(UiaError::from),
    )
}

fn tree_on_mta_thread(
    hwnd: usize,
    max_depth: usize,
    max_nodes: usize,
) -> Result<UiTree, ReadError> {
    let _apartment = ComApartment::init_mta().map_err(ReadError::Failed)?;
    let (automation, root) = automation_and_root(hwnd)?;
    tree_with(&automation, hwnd, root, max_depth, max_nodes, None)
}

/// 木の本体 (D-084、D-109)。`automation` と `hwnd` の窓の要素 `root` から、根がコントロールビューに属すると読めたときだけ
/// `walk_tree` でたどる (`hwnd` は失敗の文に埋めるため)。本番 (`tree_on_mta_thread`) と計測の入口 (`probe_tree`) が同じ
/// 呼び出しを同じ順で行う。`before_child_read` は 2 回目の `describe` (根の最初の子の読み) の頭で 1 回だけ呼ぶ
/// (根の `describe` が失敗するか、根の子を取れないか、`max_depth` が 0 か、`max_nodes` が 1 以下なら呼ばない)。本番は None。
/// `before_child_read` の約束は `read_focus_raw` の `between` と同じ: COM を MTA で初期化したこのスレッドで、UIA の
/// オブジェクトを持ったまま走る。COM のオブジェクトをスレッドの外へ持ち出さない閉包だけを渡す (計測の入口では、
/// 上限で切り離された後に走りうる)。呼び手の `ComApartment` より後に作った局所の中で呼ぶ
fn tree_with(
    automation: &IUIAutomation,
    hwnd: usize,
    root: IUIAutomationElement,
    max_depth: usize,
    max_nodes: usize,
    before_child_read: Option<Box<dyn FnOnce()>>,
) -> Result<UiTree, ReadError> {
    // 根がコントロールビューに無いと、ControlViewWalker では根の子がビューの上で根の祖先の子に当たり、深さ 1 の
    // 兄弟の経路で根の外 (許可を通していない別の窓) に出るおそれがある。だから根がビューに属すると読めたときだけたどる。
    // 読みの UIA の時間切れは TimedOut、他の失敗は Failed (read_error。D-110)
    // SAFETY: root は同じスレッドで ElementFromHandle が返した有効な要素の getter。BOOL を返すだけで、
    //         要素が消えていれば Err に、UIA の待ちが切れれば Err (UIA_E_TIMEOUT など) になるだけ。
    let in_view = unsafe { root.CurrentIsControlElement() }
        .map_err(|e| read_error(&format!("IsControlElement({hwnd:#x})"), &e))?
        .as_bool();
    if !in_view {
        return Err(ReadError::Failed(format!(
            "ウィンドウ {hwnd:#x} の根の要素がコントロールビューに無い (IsControlElement が偽)"
        )));
    }
    // SAFETY: 有効な automation オブジェクトの getter。コントロールビュー (find の ControlViewCondition と同じ見え方)。
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|e| read_error("ControlViewWalker", &e))?;
    // walker を持つ source はこの関数の局所に置き、呼び手の `_apartment` (この関数より外の局所) の CoUninitialize より
    // 先に Release させる (末尾の式の一時値の drop の順に頼らない)。要素 (root と途中の子や兄弟) は walk_tree の中で drop される
    let src = UiaTreeSource {
        walker,
        before_child_read: RefCell::new(before_child_read),
        described: Cell::new(0),
    };
    walk_tree(&src, root, max_depth, max_nodes)
}

/// UIA のコントロールビューの木の読み方 (D-084)。要素は COM の参照で、drop で Release される
/// (このスレッドの `ComApartment` より先に drop される)。`before_child_read` は 2 回目の `describe` の頭で取り出して
/// 1 回だけ呼ぶ閉包、`described` は `describe` の呼び出しの数 (D-109。本番は None で、数えるだけ)
struct UiaTreeSource {
    walker: IUIAutomationTreeWalker,
    before_child_read: RefCell<Option<Box<dyn FnOnce()>>>,
    described: Cell<usize>,
}

impl TreeSource for UiaTreeSource {
    type Node = IUIAutomationElement;

    fn describe(&self, n: &IUIAutomationElement) -> Result<UiElement, ReadError> {
        let k = self.described.get() + 1;
        self.described.set(k);
        // walk_tree は前順なので、1 回目が根、2 回目が根の最初の子 (根の first_child が子を返した後)
        if k == 2 {
            // 借りを先に返してから呼ぶ (閉包の中で source に触れても二重に借りない)
            let f = self.before_child_read.borrow_mut().take();
            if let Some(f) = f {
                f();
            }
        }
        describe(n)
    }

    fn first_child(&self, n: &IUIAutomationElement) -> WalkStep<IUIAutomationElement> {
        // SAFETY: walker と n は同じスレッドで作った有効なオブジェクト。子が無ければ null が返り (Win32 の
        //         GetFirstChildElement の頁は null に触れないので、出典は .NET の TreeWalker.GetFirstChild の
        //         「null reference if there is no such element」と、子の無いフィクスチャの根のテスト)、windows-core が
        //         Err (Error::empty) にする。要素が消えていれば失敗の HRESULT の Err になるだけ。
        walker_step("GetFirstChildElement", unsafe {
            self.walker.GetFirstChildElement(n)
        })
    }

    fn next_sibling(&self, n: &IUIAutomationElement) -> WalkStep<IUIAutomationElement> {
        // SAFETY: first_child と同じ。最後の兄弟の後は S_OK と null (Microsoft Learn の GetNextSiblingElement)。
        walker_step("GetNextSiblingElement", unsafe {
            self.walker.GetNextSiblingElement(n)
        })
    }
}

/// 子や兄弟の取得 (UIA の呼び出し `what`) の結果を、終わりと失敗と時間切れに分ける (D-084、D-110)。windows 0.62 の
/// 生成コードは、S_OK と null を windows-core の `from_abi` で `Error::empty()` (`code()` が S_OK) にする。それは
/// 「無い」(`End`)。UIA の時間切れ (`is_uia_timeout`) は `TimedOut` (文は `read_error`)、それ以外の Err は取得の失敗 (`Failed`)
fn walker_step(
    what: &str,
    r: windows::core::Result<IUIAutomationElement>,
) -> WalkStep<IUIAutomationElement> {
    match r {
        Ok(e) => WalkStep::Next(e),
        Err(e) if e.code() == S_OK => WalkStep::End,
        // read_error は時間切れなら TimedOut を返し、Display はその文
        Err(e) if is_uia_timeout(e.code()) => WalkStep::TimedOut(read_error(what, &e).to_string()),
        Err(_) => WalkStep::Failed,
    }
}

fn query_elements(hwnd: usize, query: &ElementQuery) -> Result<Elements, ReadError> {
    let (automation, root) = automation_and_root(hwnd)?;
    query_elements_with(&automation, &root, query, || {})
}

/// 要素の読みの本体 (D-054、D-109)。`automation` と窓の要素 `root` から、コントロールビューの子孫を `FindAll` で取り、
/// 根、子の順に `consider` で見る。本番 (`query_elements`) と計測の入口 (`probe_elements`) が同じ呼び出しを同じ順で行う。
/// 子の取得 (`GetElement`) と子の `consider` の Err は `elements::skip_or_abort` に通し、時間切れでない失敗 (取得の後に
/// 消えた要素) は `skipped` に数えて次の子へ、UIA の時間切れは残りを読まずに Err (文の頭は `CHILD_READ_CONTEXT`) にする。
/// 根の側 (`ControlViewCondition`、`FindAll`、`Length`、根の `consider`) の Err は頭を付けずにそのまま返す (D-110)。
/// `between` は根の `consider` の後・子の繰り返しの前に 1 回だけ呼ぶ (`FindAll` か `Length` か根の `consider` が Err なら、
/// または根で続けない (上限や文字列の打ち切り) なら呼ばずに返る)。本番は何もしない閉包。
/// `between` の約束は `read_focus_raw` の `between` と同じ: COM を MTA で初期化したこのスレッドで、UIA のオブジェクトを
/// 持ったまま走る。COM のオブジェクトをスレッドの外へ持ち出さない閉包だけを渡す (計測の入口では、上限で切り離された後に
/// 走りうる)
fn query_elements_with(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    query: &ElementQuery,
    between: impl FnOnce(),
) -> Result<Elements, ReadError> {
    // SAFETY: 有効な automation オブジェクトの getter。コントロールビュー (レイアウト用の要素を除く) の条件。
    let cond = unsafe { automation.ControlViewCondition() }
        .map_err(|e| read_error("ControlViewCondition", &e))?;
    // SAFETY: root と cond は同じスレッドで得た有効なオブジェクト。Descendants はウィンドウ自身を含まないので、
    //         自身は先に別に見る。
    let found = unsafe { root.FindAll(TreeScope_Descendants, &cond) }
        .map_err(|e| read_error("FindAll", &e))?;
    // SAFETY: found は有効な配列。
    let n = unsafe { found.Length() }.map_err(|e| read_error("Length", &e))?;
    let mut out = Elements::default();
    // ウィンドウ自身が消えていれば (時間切れも)、全体を失敗にする
    if !consider(root, query, &mut out)? {
        return Ok(out);
    }
    between();
    for i in 0..n {
        // SAFETY: i は 0..Length の範囲。
        let el = match unsafe { found.GetElement(i) } {
            Ok(el) => el,
            Err(e) => {
                skip_or_abort(
                    read_error("GetElement", &e),
                    &mut out.skipped,
                    CHILD_READ_CONTEXT,
                )?;
                continue;
            }
        };
        match consider(&el, query, &mut out) {
            Ok(true) => {}
            Ok(false) => break,
            // FindAll の結果は取得時点のもの。その後に消えた要素 (UIA_E_ELEMENTNOTAVAILABLE) は飛ばし、
            // UIA の時間切れは残りを読まずに打ち切る (D-110)
            Err(e) => skip_or_abort(e, &mut out.skipped, CHILD_READ_CONTEXT)?,
        }
    }
    Ok(out)
}

/// 要素を見て、一致すれば `out` に足す。続けないとき (上限、または文字列を持つ最初の要素を見つけたとき) は false。
/// フォーカスは判定を通った要素についてだけ読み (focused で絞る問い合わせは判定の前、それ以外は結果に入れる直前)、
/// 状態は文字列を読まない問い合わせで結果に入れる要素 (上限の判定の後) についてだけ読む (D-089)。
/// IsPassword は、文字列を読む問い合わせでは read_text の前に、それ以外は状態と同じ位置で読む (D-090)。
/// どの読みの UIA の時間切れも、既定値に替えずに `?` で Err として返す (D-110)
fn consider(
    el: &IUIAutomationElement,
    query: &ElementQuery,
    out: &mut Elements,
) -> Result<bool, ReadError> {
    let mut e = describe(el)?;
    // フォーカスを読む前に、名前・種類・automation_id で外れた要素を捨てる (読み取りを増やさない)
    if !query.matches_except_focus(&e) {
        return Ok(true);
    }
    if query.focused {
        e.focused = read_focus(el)?;
        if !query.matches_element(&e) {
            return Ok(true);
        }
    }
    if query.with_text {
        // パスワードの要素は read_text より先に IsPassword を読み、真なら文字列を読まずに飛ばす
        // (中身をプロセスをまたいで持ち込まない。D-090)。結果に入れる要素の password は false のまま。
        // IsPassword の時間切れは偽とみなさずに打ち切る (読めないまま文字列を読みに進まない。D-110)
        if query.skips_text_of_password(read_password(el)?) {
            out.password_skipped += 1;
            return Ok(true);
        }
        // 文字列を持つ要素だけを返し、最初の 1 つで打ち切る (プロセスをまたぐ読み取りを最小にする)。
        // 状態は読まない (reads_states が偽)
        return Ok(match read_text(el)? {
            Some(t) => {
                if !query.focused {
                    e.focused = read_focus(el)?;
                }
                e.text = Some(t);
                out.elements.push(e);
                false
            }
            None => true,
        });
    }
    if out.elements.len() >= query.limit {
        out.truncated = true;
        return Ok(false);
    }
    if !query.focused {
        e.focused = read_focus(el)?;
    }
    e.password = read_password(el)?;
    if query.reads_states() {
        read_states(el, &mut e)?;
    }
    out.elements.push(e);
    Ok(true)
}

/// 要素がキーボードのフォーカスを持つか (`CurrentHasKeyboardFocus`)。UIA の時間切れは Err (D-110)、他の読めないときは
/// false (D-089)
fn read_focus(el: &IUIAutomationElement) -> Result<bool, ReadError> {
    // SAFETY: el は同じスレッドで得た有効な要素。getter は BOOL を値で返すだけで、要素が消えていれば
    //         Err (UIA_E_ELEMENTNOTAVAILABLE) に、UIA の待ちが切れれば Err (UIA_E_TIMEOUT など) になるだけ
    //         (describe の Current* と同じ)。
    let r = unsafe { el.CurrentHasKeyboardFocus() };
    Ok(attr("CurrentHasKeyboardFocus", r)?.is_some_and(|b| b.as_bool()))
}

/// 要素がパスワードの要素か (`CurrentIsPassword`)。UIA の時間切れは Err (D-110)、他の読めないときは false (D-090)
fn read_password(el: &IUIAutomationElement) -> Result<bool, ReadError> {
    // SAFETY: el は同じスレッドで得た有効な要素。getter は BOOL を値で返すだけで、要素が消えていれば
    //         Err (UIA_E_ELEMENTNOTAVAILABLE) に、UIA の待ちが切れれば Err (UIA_E_TIMEOUT など) になるだけ
    //         (describe の Current* と同じ)。
    let r = unsafe { el.CurrentIsPassword() };
    Ok(attr("CurrentIsPassword", r)?.is_some_and(|b| b.as_bool()))
}

/// 状態: TogglePattern の ToggleState、SelectionItemPattern の IsSelected、ExpandCollapsePattern の
/// ExpandCollapseState。UIA の時間切れは Err (D-110)。パターンを持たないか、他の読めないときか、知らない値なら、
/// その欄は None のまま (D-089)
fn read_states(el: &IUIAutomationElement, e: &mut UiElement) -> Result<(), ReadError> {
    // SAFETY: el は有効。パターンを持たない要素では、null が Err (windows-core の Error::empty) に変換されて返るだけ。
    //         IID は生成コードが T::IID を渡すので、id と型を取り違えても E_NOINTERFACE の Err になるだけ。
    let toggle =
        unsafe { el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) };
    if let Some(p) = attr("GetCurrentPattern(Toggle)", toggle)? {
        // SAFETY: p は有効な TogglePattern。ToggleState (i32 の newtype) を値で返す。
        let s = unsafe { p.CurrentToggleState() };
        if let Some(s) = attr("CurrentToggleState", s)? {
            e.toggle = toggle_state_name(s.0).map(str::to_string);
        }
    }
    // SAFETY: 同上 (SelectionItemPattern)。
    let selection = unsafe {
        el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
    };
    if let Some(p) = attr("GetCurrentPattern(SelectionItem)", selection)? {
        // SAFETY: p は有効な SelectionItemPattern。BOOL を値で返す。
        let b = unsafe { p.CurrentIsSelected() };
        if let Some(b) = attr("CurrentIsSelected", b)? {
            e.selected = Some(b.as_bool());
        }
    }
    // SAFETY: 同上 (ExpandCollapsePattern)。
    let expand = unsafe {
        el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId)
    };
    if let Some(p) = attr("GetCurrentPattern(ExpandCollapse)", expand)? {
        // SAFETY: p は有効な ExpandCollapsePattern。ExpandCollapseState (i32 の newtype) を値で返す。
        let s = unsafe { p.CurrentExpandCollapseState() };
        if let Some(s) = attr("CurrentExpandCollapseState", s)? {
            e.expanded = expand_collapse_state_name(s.0).map(str::to_string);
        }
    }
    Ok(())
}

/// 要素の `describe` の欄を読む。7 つの `Current*` を 1 つずつ順に呼び、UIA の時間切れならそこで残りを呼ばずに Err
/// (D-110。組で呼ぶと 1 要素の中で待ちが重なる)。種類の時間切れでない Err は全体の `Failed`、他の欄の時間切れでない
/// Err は既定値 (空、None、偽) にする (今までどおり)
fn describe(el: &IUIAutomationElement) -> Result<UiElement, ReadError> {
    // 以下の各 Current* の SAFETY: el は同じスレッドで得た有効な要素。getter は値を複製して返し、BSTR は windows クレートが
    // 解放する。要素が消えていれば Err (UIA_E_ELEMENTNOTAVAILABLE) に、UIA の待ちが切れれば Err (UIA_E_TIMEOUT など) に
    // なるだけ。
    // SAFETY: 上のとおり (名前)。
    let name = attr("CurrentName", unsafe { el.CurrentName() })?;
    // SAFETY: 上のとおり (種類)。
    let ty =
        unsafe { el.CurrentControlType() }.map_err(|e| read_error("CurrentControlType", &e))?;
    // SAFETY: 上のとおり (automation_id)。
    let aid = attr("CurrentAutomationId", unsafe { el.CurrentAutomationId() })?;
    // SAFETY: 上のとおり (クラス名)。
    let class = attr("CurrentClassName", unsafe { el.CurrentClassName() })?;
    // SAFETY: 上のとおり (矩形。RECT を値で返す)。
    let rect = attr("CurrentBoundingRectangle", unsafe {
        el.CurrentBoundingRectangle()
    })?;
    // SAFETY: 上のとおり (有効か。BOOL を値で返す)。
    let enabled = attr("CurrentIsEnabled", unsafe { el.CurrentIsEnabled() })?;
    // SAFETY: 上のとおり (画面外か。BOOL を値で返す)。
    let off = attr("CurrentIsOffscreen", unsafe { el.CurrentIsOffscreen() })?;
    Ok(UiElement {
        name: name.map(|b| b.to_string()).unwrap_or_default(),
        control_type: control_type_name(ty.0),
        automation_id: aid.map(|b| b.to_string()).unwrap_or_default(),
        class: class.map(|b| b.to_string()).unwrap_or_default(),
        rect: rect.and_then(rect_from),
        // 分からなければ安全側 (押しても効かないかもしれない)
        enabled: enabled.is_some_and(|b| b.as_bool()),
        offscreen: off.is_some_and(|b| b.as_bool()),
        // フォーカスと状態は describe (tree と共有) では読まない。find の経路は consider で読む (D-089)
        focused: false,
        password: false,
        toggle: None,
        selected: None,
        expanded: None,
        text: None,
    })
}

fn rect_from(r: RECT) -> Option<Rect> {
    let w = r.right.checked_sub(r.left)?;
    let h = r.bottom.checked_sub(r.top)?;
    (w > 0 && h > 0).then(|| Rect::new(r.left, r.top, w as u32, h as u32))
}

/// 文字列: TextPattern の文書全体、無ければ ValuePattern の値。どちらも無ければ None。UIA の時間切れは Err (D-110)、
/// 他の読めないときは None (パターンを持たないときと同じ扱い。今までどおり)
fn read_text(el: &IUIAutomationElement) -> Result<Option<String>, ReadError> {
    // SAFETY: el は有効。パターンを持たない要素では、null が Err (windows-core の Error::empty) に変換されて返るだけ。
    let text_pattern =
        unsafe { el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) };
    if let Some(tp) = attr("GetCurrentPattern(Text)", text_pattern)? {
        // SAFETY: tp は有効な TextPattern。
        let range = unsafe { tp.DocumentRange() };
        if let Some(r) = attr("DocumentRange", range)? {
            // SAFETY: r は有効な範囲。-1 は長さの上限なし。
            let s = unsafe { r.GetText(-1) };
            if let Some(s) = attr("GetText", s)? {
                return Ok(Some(s.to_string()));
            }
        }
    }
    // SAFETY: el は有効。パターンを持たない要素では、null が Err (windows-core の Error::empty) に変換されて返るだけ。
    let value_pattern =
        unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) };
    if let Some(vp) = attr("GetCurrentPattern(Value)", value_pattern)? {
        // SAFETY: vp は有効な ValuePattern。
        let s = unsafe { vp.CurrentValue() };
        if let Some(s) = attr("CurrentValue", s)? {
            return Ok(Some(s.to_string()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::UI::Accessibility::UIA_E_ELEMENTNOTAVAILABLE;

    /// UIA の時間切れの 2 つの HRESULT (`is_uia_timeout` が真のもの。D-104、D-110)
    fn timeout_codes() -> [HRESULT; 2] {
        [
            HRESULT(UIA_E_TIMEOUT as i32),
            HRESULT::from_win32(ERROR_TIMEOUT.0),
        ]
    }

    fn err(code: HRESULT) -> windows::core::Error {
        windows::core::Error::from_hresult(code)
    }

    /// 子や兄弟の無いときの null (windows-core の Error::empty) は End、失敗の HRESULT は Failed (D-084)、UIA の時間切れは
    /// TimedOut で文に呼び出しの名前と「UIA の時間切れ」を含む (D-110)。COM も unsafe も使わない
    #[test]
    fn walker_step_null_is_end_and_other_errors_are_failed() {
        assert!(matches!(
            walker_step("GetFirstChildElement", Err(windows::core::Error::empty())),
            WalkStep::End
        ));
        assert!(matches!(
            walker_step("GetFirstChildElement", Err(err(E_FAIL))),
            WalkStep::Failed
        ));
        let gone = HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
        assert!(matches!(
            walker_step("GetNextSiblingElement", Err(err(gone))),
            WalkStep::Failed
        ));
        for code in timeout_codes() {
            match walker_step("GetNextSiblingElement", Err(err(code))) {
                WalkStep::TimedOut(m) => assert!(
                    m.starts_with("GetNextSiblingElement が UIA の時間切れ"),
                    "{m}"
                ),
                other => panic!("{code}: TimedOut でない: {}", walk_step_kind(&other)),
            }
        }
    }

    /// `WalkStep` の種類 (要素は Debug を持たないので種類だけを出す)
    fn walk_step_kind<N>(s: &WalkStep<N>) -> &'static str {
        match s {
            WalkStep::Next(_) => "Next",
            WalkStep::End => "End",
            WalkStep::Failed => "Failed",
            WalkStep::TimedOut(_) => "TimedOut",
        }
    }

    /// 属性の読みの振り分け (D-110): Ok は Some、時間切れの 2 つは打ち切りの Err、要素が消えた・E_FAIL・S_OK の empty
    /// (パターンを持たない) は None (既定値にする)
    #[test]
    fn attr_keeps_value_drops_failure_and_stops_on_timeout() {
        assert_eq!(attr("CurrentName", Ok(5)), Ok(Some(5)));
        for code in timeout_codes() {
            match attr::<i32>("CurrentName", Err(err(code))) {
                Err(ReadError::TimedOut(m)) => {
                    assert!(
                        m.contains("UIA の時間切れ") && m.contains("CurrentName"),
                        "{m}"
                    )
                }
                other => panic!("{code}: TimedOut でない: {other:?}"),
            }
        }
        let gone = HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
        assert_eq!(attr::<i32>("CurrentName", Err(err(gone))), Ok(None));
        assert_eq!(attr::<i32>("CurrentName", Err(err(E_FAIL))), Ok(None));
        assert_eq!(
            attr::<i32>(
                "GetCurrentPattern(Text)",
                Err(windows::core::Error::empty())
            ),
            Ok(None)
        );
    }

    /// 呼び出しの Err の振り分け (D-110): 時間切れの 2 つは TimedOut (文はフォーカスの判定の形に 1 句を足したもの)、
    /// E_FAIL は Failed (今までの「名前: 誤り」の文)
    #[test]
    fn read_error_splits_timeout_from_failure() {
        for code in timeout_codes() {
            assert_eq!(
                read_error("FindAll", &err(code)),
                ReadError::TimedOut(format!(
                    "FindAll が UIA の時間切れ ({code})。対象のアプリが応答していない"
                ))
            );
        }
        match read_error("FindAll", &err(E_FAIL)) {
            ReadError::Failed(m) => assert!(m.starts_with("FindAll: "), "{m}"),
            other => panic!("Failed でない: {other:?}"),
        }
    }

    /// 読みの失敗を UIA の層の誤りに写すとき、種類と文を保つ (D-110)
    #[test]
    fn uia_error_from_read_error_keeps_kind() {
        assert_eq!(
            UiaError::from(ReadError::Failed("f".into())),
            UiaError::Failed("f".into())
        );
        assert_eq!(
            UiaError::from(ReadError::TimedOut("t".into())),
            UiaError::TimedOut("t".into())
        );
    }

    // 以下の run_bounded のテストは COM に触れない閉包だけを渡す (D-104)。各テストは自分の数を使う

    /// すぐ返る値と閉包の Err はそのまま返り、どちらの後も数が 0 に戻る
    #[test]
    fn run_bounded_returns_value_and_error_and_releases_count() {
        static C: AtomicUsize = AtomicUsize::new(0);
        let r = run_bounded("t-ok", Duration::from_secs(5), &C, 4, || Ok(7));
        assert_eq!(r, Ok(7));
        assert_eq!(C.load(Ordering::SeqCst), 0);
        let r: Result<(), UiaError> = run_bounded("t-err", Duration::from_secs(5), &C, 4, || {
            Err(UiaError::Failed("x".into()))
        });
        assert_eq!(r, Err(UiaError::Failed("x".into())));
        assert_eq!(C.load(Ordering::SeqCst), 0);
    }

    /// 上限より長く眠る閉包は上限の近くで時間切れになり、閉包が終わった後に数が 0 に戻る (実の時計なので範囲で見る)
    #[test]
    fn run_bounded_times_out_and_releases_count_when_thread_ends() {
        static C: AtomicUsize = AtomicUsize::new(0);
        let start = std::time::Instant::now();
        let r = run_bounded("t-slow", Duration::from_millis(100), &C, 4, || {
            std::thread::sleep(Duration::from_secs(3));
            Ok(())
        });
        let elapsed = start.elapsed();
        let Err(UiaError::TimedOut(m)) = r else {
            panic!("時間切れでない: {r:?}");
        };
        assert!(m.contains("時間切れ") && m.contains("0.1"), "{m}");
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_secs(2),
            "{elapsed:?}"
        );
        assert_eq!(
            C.load(Ordering::SeqCst),
            1,
            "切り離したスレッドはまだ数に入る"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while C.load(Ordering::SeqCst) != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(C.load(Ordering::SeqCst), 0);
    }

    /// panic する閉包は「異常終了」の Failed で、数が 0 に戻る
    #[test]
    fn run_bounded_reports_panic_and_releases_count() {
        static C: AtomicUsize = AtomicUsize::new(0);
        let r: Result<(), UiaError> = run_bounded("t-panic", Duration::from_secs(5), &C, 4, || {
            panic!("閉包の panic (テストの意図どおり)")
        });
        let Err(UiaError::Failed(m)) = r else {
            panic!("Failed でない: {r:?}");
        };
        assert!(m.contains("異常終了"), "{m}");
        assert_eq!(C.load(Ordering::SeqCst), 0);
    }

    /// 数が上限に達していれば閉包を呼ばずに TimedOut (起動し直す案内つき) を返し、数は変わらない
    #[test]
    fn run_bounded_refuses_when_inflight_reaches_max() {
        static C: AtomicUsize = AtomicUsize::new(2);
        static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let r = run_bounded("t-full", Duration::from_secs(5), &C, 2, || {
            CALLED.store(true, Ordering::SeqCst);
            Ok(())
        });
        let Err(UiaError::TimedOut(m)) = r else {
            panic!("TimedOut でない: {r:?}");
        };
        assert!(m.contains("起動し直す"), "{m}");
        assert!(!CALLED.load(Ordering::SeqCst));
        assert_eq!(C.load(Ordering::SeqCst), 2);
    }

    /// 要素と木の組は UIA_READ_LIMIT と UIA_READ_INFLIGHT を使う (D-104)
    #[test]
    fn read_bounds_use_read_limit_and_count() {
        let b = read_bounds("x");
        assert_eq!(b.name, "x");
        assert_eq!(b.limit, UIA_READ_LIMIT);
        assert_eq!(UIA_READ_LIMIT, Duration::from_secs(30));
        assert!(std::ptr::eq(b.inflight, &UIA_READ_INFLIGHT));
    }

    /// UIA 自身の時間切れの HRESULT だけを時間切れと見る (D-104)
    #[test]
    fn is_uia_timeout_matches_only_uia_timeout() {
        assert!(is_uia_timeout(HRESULT(UIA_E_TIMEOUT as i32)));
        assert!(is_uia_timeout(HRESULT::from_win32(ERROR_TIMEOUT.0)));
        assert!(!is_uia_timeout(E_FAIL));
        assert!(!is_uia_timeout(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32)));
        assert!(!is_uia_timeout(S_OK));
    }

    /// フォーカスの判定は要素と木とは別の数と、自分の上限と名前を使う (D-104、本人の決定)
    #[test]
    fn focus_and_read_bounds_use_separate_counts() {
        let f = focus_bounds();
        assert!(!std::ptr::eq(read_bounds("x").inflight, f.inflight));
        assert!(std::ptr::eq(f.inflight, &UIA_FOCUS_INFLIGHT));
        assert_eq!(f.limit, FOCUS_READ_LIMIT);
        assert_eq!(f.name, "ai-cu-uia-focus");
    }

    /// 生の読みを組む (COM なし)。`focused` が Some なら GetFocusedElement の Err の HRESULT、`is_password` は
    /// CurrentIsPassword の結果
    fn raw(focused: Option<HRESULT>, is_password: Option<Result<bool, HRESULT>>) -> FocusRaw {
        FocusRaw {
            short_timeouts: true,
            focused: ProbeCall {
                code: focused.map(|h| h.0),
                ms: 3,
            },
            is_password: is_password.map(|r| PasswordRead {
                ms: 5,
                result: r.map_err(|h| h.0),
            }),
        }
    }

    /// 振り分けは D-104 の判定の挙動を写す (D-108)。S_OK の null は通し、2 つの時間切れは TimedOut、他の失敗は
    /// Failed。要素があれば IsPassword の値を返し、時間切れは TimedOut、他の Err と読みの無い形は通す
    #[test]
    fn classify_focus_matches_current_behavior() {
        let timeouts = [
            HRESULT(UIA_E_TIMEOUT as i32),
            HRESULT::from_win32(ERROR_TIMEOUT.0),
        ];
        assert_eq!(classify_focus(&raw(Some(S_OK), None)), Ok(None));
        for t in timeouts {
            let Err(UiaError::TimedOut(m)) = classify_focus(&raw(Some(t), None)) else {
                panic!("GetFocusedElement の {t} が TimedOut でない");
            };
            assert!(
                m.contains("GetFocusedElement") && m.contains("時間切れ"),
                "{m}"
            );
        }
        let Err(UiaError::Failed(m)) = classify_focus(&raw(Some(E_FAIL), None)) else {
            panic!("GetFocusedElement の E_FAIL が Failed でない");
        };
        assert!(
            m.contains("GetFocusedElement") && m.contains("0x80004005"),
            "{m}"
        );
        // 系の文が空のときは HRESULT だけにし、末尾に空白を残さない (win32-reviewer の nit)
        assert_eq!(
            focus_failed_text(E_FAIL, ""),
            "GetFocusedElement: 0x80004005"
        );
        assert_eq!(
            focus_failed_text(E_FAIL, "Unspecified error"),
            "GetFocusedElement: 0x80004005 Unspecified error"
        );
        assert_eq!(classify_focus(&raw(None, Some(Ok(true)))), Ok(Some(true)));
        assert_eq!(classify_focus(&raw(None, Some(Ok(false)))), Ok(Some(false)));
        for t in timeouts {
            let Err(UiaError::TimedOut(m)) = classify_focus(&raw(None, Some(Err(t)))) else {
                panic!("CurrentIsPassword の {t} が TimedOut でない");
            };
            assert!(
                m.contains("CurrentIsPassword") && m.contains("時間切れ"),
                "{m}"
            );
        }
        let gone = HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
        assert_eq!(classify_focus(&raw(None, Some(Err(gone)))), Ok(None));
        assert_eq!(classify_focus(&raw(None, Some(Err(E_FAIL)))), Ok(None));
        assert_eq!(classify_focus(&raw(None, None)), Ok(None));
    }

    /// 計測の入口は上限 20 秒で、判定とも要素と木とも別の数を使う (D-108)
    #[test]
    fn probe_bounds_are_separate_from_focus_and_read() {
        let p = probe_bounds();
        assert_eq!(p.limit, Duration::from_secs(20));
        assert!(!std::ptr::eq(p.inflight, &UIA_FOCUS_INFLIGHT));
        assert!(!std::ptr::eq(p.inflight, &UIA_READ_INFLIGHT));
    }

    /// 判定の上限は UIA の待ち 3 回分 (接続 1 回と処理 2 回) より長い (D-104)
    #[test]
    fn focus_read_limit_covers_three_uia_waits() {
        assert!(FOCUS_READ_LIMIT > Duration::from_millis(3 * u64::from(FOCUS_TIMEOUT_MS)));
        assert_eq!(FOCUS_READ_LIMIT, Duration::from_secs(4));
    }

    /// 秒は末尾の 0 を出さずに書く
    #[test]
    fn secs_text_drops_trailing_zeros() {
        assert_eq!(secs_text(Duration::from_millis(100)), "0.1");
        assert_eq!(secs_text(Duration::from_millis(4000)), "4");
        assert_eq!(secs_text(Duration::from_secs(30)), "30");
        assert_eq!(secs_text(Duration::from_millis(1250)), "1.25");
    }
}
