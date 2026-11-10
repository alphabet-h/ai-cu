//! 失敗のとき (と `windows` の空の結果。D-074) の次の手 (hint) を、呼ばれ方 (CLI か MCP か) に合わせて選ぶ (D-056)。
//!
//! 既存の CLI の文 (`run` の `*_HINT` 定数と `ErrorKind::hint`) は一字一句変えない。MCP の文は道具の名前で書き、
//! CLI の書き方 (`ai-cu ...`、`--shot`) を出さない。抜けを検査できるよう、文字列の対応表ではなく enum の総当たりで選ぶ。

use crate::error::ErrorKind;
use crate::run::{
    CAPTURE_OCCLUDED_HINT, CAPTURE_SENSITIVE_HINT, CLOSE_DENIED_HINT, CLOSE_DISABLED_HINT,
    CLOSE_NOT_FOUND_HINT, CLOSE_USER_ACTIVE_HINT, ELEVATED_HINT, FOCUS_DENIED_HINT,
    FOCUS_MINIMIZED_HINT, FOCUS_NOT_FOUND_HINT, FOCUS_REFUSED_HINT, FOCUS_USER_ACTIVE_HINT,
    HOST_CAPTURE_HINT, HOST_WINDOW_HINT, KEYBOARD_GUARD_HINT, MOUSE_GUARD_HINT, SCREEN_WAIT_HINT,
    SELF_WINDOW_HINT,
};

/// 呼ばれ方。TUI は CLI と同じ扱い
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Surface {
    #[default]
    Cli,
    Mcp,
}

/// 状況に合わせた次の手の種類 (終了コードごとの既定より細かいもの)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// クリックする点の下が許可されていない (7)
    MouseGuard,
    /// 前面が許可されていない (7)
    KeyboardGuard,
    /// 管理者権限のウィンドウ (7)
    Elevated,
    /// ai-cu 自身を動かしている端末 (7。TUI からの実行)
    SelfWindow,
    /// ai-cu を起動した側のウィンドウ (7)
    HostWindow,
    /// 撮影と読み取りの対象が ai-cu を起動した側 (7)
    HostCapture,
    /// 撮る対象が別のウィンドウに隠れている (7)
    CaptureOccluded,
    /// 撮る矩形に、設定で拒否されたウィンドウが見えている (7。D-077)
    CaptureSensitive,
    /// 起動した側を特定できないので実入力をすべて断る (7。`reason: host_unknown`)
    HostUnknown,
    /// focus: 見つからない、閉じられた (11)
    FocusNotFound,
    /// focus: 最小化されている (11)
    FocusMinimized,
    /// focus: 人が別のアプリを使っていて断られた (11。`reason: refused_other_app`)
    FocusRefused,
    /// focus: 前面は ai-cu を起動した側なのに断られた (11。`reason: refused_host_front`)
    FocusHostFront,
    /// focus: 断られ、前面を確かめられない (11。`reason: refused_unknown`)
    FocusRefusedUnknown,
    /// focus: 対象が許可されていない (7)
    FocusDenied,
    /// focus: 人が操作中 (10)
    FocusUserActive,
    /// close: 見つからない、WM_CLOSE を置けなかった (12。D-062)
    CloseNotFound,
    /// close: 対象が許可されていない (7)
    CloseDenied,
    /// close: 対象が前面で、人が操作中 (10)
    CloseUserActive,
    /// close: 対象がダイアログを開いていて無効 (7。`reason: target_disabled`)
    CloseDisabled,
    /// 実行の前に、キーかマウスのボタンが押されたまま (10。`reason: keys_held`。D-066)
    KeysHeld,
    /// windows: 一覧が空で、許可済みのプロセスが動いているが見える窓が無い (`hidden_allowed` が空でない。D-074)
    WindowsHiddenAllowed,
    /// windows: 一覧が空で、`hidden_allowed` も空 (許可に無いか、まだ起動していない。D-074)
    WindowsEmpty,
    /// 画面の待ち (`wait_stable` / `wait_change`) の時間切れ (15。D-081)。CLI と MCP で同じ文
    ScreenWait,
    /// キーと文字の届く先のフォーカスがパスワード欄で、`general.password_input` が許していない (4。D-090)
    PasswordField,
    /// パスワード欄かを確かめる間に前面のウィンドウが変わった (7。D-090)。新しい前面は許可された窓でもありうるので、
    /// 「許可されていない」と言う `KeyboardGuard` の文は使わない
    ForegroundChanged,
    /// 予告の間 (枠を出してから動かすまで) に人の入力があった (10。`reason: lead_input`。D-093)
    LeadUserActive,
    /// 1 つの操作のキーの束の途中で、前面が別のプロセスに変わった (7。`reason: target_changed`。D-100)。
    /// 途中まで送ってあるので「何も送っていない」とは言わない
    TargetChanged,
    /// ドラッグの途中 (実行器が押したままのボタンを持つ) のマウスの束がガードに断られた (7。D-132)。
    /// 途中まで送ってあり、窓を動かす drag は窓がカーソルの後から動くので、組み直し方を hint が持つ
    DragGuardMidway,
}

impl Hint {
    pub const ALL: [Hint; 29] = [
        Hint::MouseGuard,
        Hint::KeyboardGuard,
        Hint::Elevated,
        Hint::SelfWindow,
        Hint::HostWindow,
        Hint::HostCapture,
        Hint::CaptureOccluded,
        Hint::CaptureSensitive,
        Hint::HostUnknown,
        Hint::FocusNotFound,
        Hint::FocusMinimized,
        Hint::FocusRefused,
        Hint::FocusHostFront,
        Hint::FocusRefusedUnknown,
        Hint::FocusDenied,
        Hint::FocusUserActive,
        Hint::CloseNotFound,
        Hint::CloseDenied,
        Hint::CloseUserActive,
        Hint::CloseDisabled,
        Hint::KeysHeld,
        Hint::WindowsHiddenAllowed,
        Hint::WindowsEmpty,
        Hint::ScreenWait,
        Hint::PasswordField,
        Hint::ForegroundChanged,
        Hint::LeadUserActive,
        Hint::TargetChanged,
        Hint::DragGuardMidway,
    ];
}

/// ドラッグの途中でガードに断られた (CLI。D-132)。D-065 の前置き (「操作ごと打ち直さない」) の後に読まれる
pub const DRAG_GUARD_MIDWAY_HINT: &str = "途中まで送った。ドラッグの途中の点の下に許可された窓が無かったので、残りの点を送らず、押したボタンを離して止めた (断った点は error.detail.point)。窓を動かす drag (タイトルバーを掴むなど) なら、窓が途中まで動いていることがあるので `ai-cu windows` で rect を読み直す。窓はカーソルの後から動くので、1 歩を、掴んだ点から進む向きの窓の端までの距離の半分以下にする (`--steps` を増やし、`--duration-ms` も 1 歩 50 ms ほどに。操作列なら `steps` と `duration_ms`)。同じ始点と終点で送り直さず、読み直した rect から新しい始点で作る。窓の外へ運ぶ drag なら、途中の点と終点の下が許可された窓になる経路にするか、人間に頼む (設定は書き換えない)。組み直しは各点が同じガードを通すもので、ガードの回避ではない。組み直しは 1 回だけで、また断られたら繰り返さず人間に頼む";
/// ドラッグの途中でガードに断られた (MCP。D-132)
const MCP_DRAG_GUARD_MIDWAY_HINT: &str = "途中まで送った。ドラッグの途中の点の下に許可された窓が無かったので、残りの点を送らず、押したボタンを離して止めた (断った点は error.detail.point)。窓を動かす drag (タイトルバーを掴むなど) なら、窓が途中まで動いていることがあるので windows の道具で rect を読み直す。窓はカーソルの後から動くので、1 歩を、掴んだ点から進む向きの窓の端までの距離の半分以下にする (steps を増やし、duration_ms も 1 歩 50 ms ほどに)。同じ始点と終点で送り直さず、読み直した rect から新しい始点で作る。窓の外へ運ぶ drag なら、途中の点と終点の下が許可された窓になる経路にするか、人間に頼む (設定は書き換えない)。組み直しは各点が同じガードを通すもので、ガードの回避ではない。組み直しは 1 回だけで、また断られたら繰り返さず人間に頼む";

/// 操作の途中で前面が別のプロセスに変わった (CLI。D-100)
pub const TARGET_CHANGED_HINT: &str = "途中まで送った。文字を打っている間に前面が別のアプリの窓に変わったので、残りを送らずに止めた (変わった後の窓は error.detail.window、最初の窓の pid は error.detail.pinned_pid)。どこまで入ったかを画面で確かめ、`ai-cu focus` で対象を前面にしてから残りだけを送る。全文を送り直すと二重に入る";
/// 操作の途中で前面が別のプロセスに変わった (MCP。D-100)
const MCP_TARGET_CHANGED_HINT: &str = "途中まで送った。文字を打っている間に前面が別のアプリの窓に変わったので、残りを送らずに止めた (変わった後の窓は error.detail.window、最初の窓の pid は error.detail.pinned_pid)。どこまで入ったかを画面で確かめ、focus の道具で対象を前面にしてから残りだけを送る (run なら先頭に focus)。全文を送り直すと二重に入る";

/// 予告の間に人の入力があった (CLI。D-093)
pub const LEAD_USER_ACTIVE_HINT: &str = "何も送っていない。予告の間 (画面の縁に枠を出してから動かすまで) に人がマウスかキーボードに触れたので止めた。人が離れるのを数秒待って 1 回だけやり直し、続くなら人間に確かめる";
/// 予告の間に人の入力があった (MCP。D-093)。待つ時間は `status` の `retry_after_ms`
const MCP_LEAD_USER_ACTIVE_HINT: &str = "何も送っていない。予告の間 (画面の縁に枠を出してから動かすまで) に人がマウスかキーボードに触れたので止めた。status の道具の retry_after_ms だけ待って 1 回だけやり直し、だめなら人間に確かめる";

/// フォーカスがパスワード欄 (CLI。D-090)
pub const PASSWORD_FIELD_HINT: &str = "何も送っていない。キーと文字の届く先 (フォーカス) がパスワード欄なので断った。パスワードは AI が打たず、人間に打ってもらう。Enter や Tab などのキーもこの欄では断るので、送信やフォーカスの移動はボタンや別の要素をクリックして行う (位置は `ai-cu find` で確かめる)。打たせてよいなら、人間が設定の general.password_input を true にする (error.fix の文をそのまま人間に伝える。設定は自分で書き換えない)";
/// パスワード欄かを確かめる間に前面のウィンドウが変わった (CLI。D-090)
pub const FOREGROUND_CHANGED_HINT: &str = "何も送っていない。パスワード欄かを確かめる間に前面のウィンドウが変わった (変わった後の窓は error.detail)。`ai-cu windows` や `ai-cu find` で前面とフォーカスを確かめ、必要なら `ai-cu focus` で対象を前面にしてから送り直す";
const MCP_PASSWORD_FIELD_HINT: &str = "何も送っていない。キーと文字の届く先 (フォーカス) がパスワード欄なので断った。パスワードは AI が打たず、人間に打ってもらう。Enter や Tab などのキーもこの欄では断るので、送信やフォーカスの移動はボタンや別の要素を click して行う (位置は find の道具で確かめる)。打たせてよいなら、人間が設定の general.password_input を true にする (error.fix の文をそのまま人間に伝える。設定は自分で書き換えない)";
const MCP_FOREGROUND_CHANGED_HINT: &str = "何も送っていない。パスワード欄かを確かめる間に前面のウィンドウが変わった (変わった後の窓は error.detail)。windows や find の道具で前面とフォーカスを確かめ、必要なら focus の道具で対象を前面にしてから送り直す (run なら先頭に focus)";

/// 押されたままのキーとボタン (D-066)。CLI の文。MCP は再試行の前に `status` を読む文 (`MCP_KEYS_HELD_HINT`。D-073)
pub const KEYS_HELD_HINT: &str = "何も送っていない。キー (Shift、Ctrl、Alt、Win) かマウスのボタンが押されたままなので、入力に混ざらないよう断った (どれかは error.detail)。人が操作中なら数秒待って 1 回だけ再試行する。続くなら、キーが押されたままになっていないか人間に確かめてもらう。代わりに離す操作を送らない";
/// 押されたままのキーの MCP の再試行の手順 (D-073)。10 の `keys_held` の MCP の次の手と、`status` の `note` が共有する
pub const MCP_KEYS_HELD_RETRY: &str = "少し後に status の道具の keys_held を読み直し、空になってから 1 回だけ再試行する。空にならなければ、キーが押されたままになっていないか人間に確かめてもらう";

/// MCP の失敗の結果に、撮った mcp:inline の画像を入れたときに hint の末尾に足す文 (D-136)。
/// 失敗の結果 (isError) の image を捨てるクライアントがあるので、届かない恐れと回り道を伝える
pub const MCP_INLINE_IMAGE_ON_FAILURE_NOTE: &str = "撮った mcp:inline の画像は結果の image に入れたが、失敗の結果 (isError) では image を捨てるクライアントがある (Claude Code など)。見えていなければ、その画像は戻らない。撮り直せない画面は、screenshot の path にファイルの保存先を書くか、失敗しうる操作の前で run を分ける";

/// focus: 前面は ai-cu を起動した側なのに断られた (CLI)
pub const FOCUS_HOST_FRONT_HINT: &str = "何も送っていない。前面は ai-cu を起動した側 (エージェント、シェル、端末) だが、Windows が前面の切り替えを断った (この ai-cu には前面化の権利が無い。起動の後に人が操作した、など)。繰り返さず、人間に対象を前面に出してもらう。タスクバーや Alt+Tab、他のツールで前面にしようとしない";
/// focus: 断られ、前面を確かめられない (CLI)
pub const FOCUS_REFUSED_UNKNOWN_HINT: &str = "何も送っていない。Windows が前面の切り替えを断った (前面のウィンドウを確かめられない)。繰り返さず、人間に対象を前面に出してもらう。タスクバーや Alt+Tab、他のツールで前面にしようとしない";
/// 起動した側を特定できない (CLI)
pub const HOST_UNKNOWN_HINT: &str = "何も送っていない。ai-cu を起動した側のプロセスを特定できないので、実入力をすべて止めている。focus や再試行では変わらない。読むだけのコマンド (windows、shot、find、tree、text) は使える。人間に ai-cu の起動のしかたを確かめてもらう";
/// windows: 一覧が空で、許可済みのプロセスの窓が隠れている (CLI。D-074)
pub const WINDOWS_HIDDEN_ALLOWED_HINT: &str = "許可されたアプリの見えるウィンドウが無い。hidden_allowed のプロセスは動いているが、ウィンドウが隠れている (タスクトレイ、非表示、別の仮想デスクトップ)。起動した直後なら ai-cu wait-for で出るのを待ち、変わらなければ人間にウィンドウを出してもらう。隠れた窓 (見えないか cloaked) の題名とクラスは ai-cu windows --include-hidden で読める。見えているが題名の無い窓やツールウィンドウは出ない";
/// windows: 一覧が空で、隠れた許可済みのプロセスも無い (CLI。D-074)
pub const WINDOWS_EMPTY_HINT: &str = "許可されたアプリの見えるウィンドウが無い。対象が許可 (allow.processes、クラスの規則) に無い (ai-cu config show で確かめ、足すのは人間に頼む) か、まだ起動していない (ai-cu wait-for で出るのを待つ)。ウィンドウを持たないプロセスは hidden_allowed に出ない";

const MCP_MOUSE_GUARD_HINT: &str = "何も送っていない。クリックする点の下に、許可されていないウィンドウ (タスクバーや別のアプリ) がある。先に focus の道具で対象を前面にし (run なら先頭に focus)、windows の道具の rect の中の点を使う。focus が focus_failed なら、繰り返さず人間に前面に出してもらう";
const MCP_KEYBOARD_GUARD_HINT: &str = "何も送っていない。キーと文字は前面のウィンドウに届くが、前面が許可されたウィンドウではない。先に focus の道具で対象を前面にしてから送る (run なら先頭に focus)。focus が focus_failed なら、繰り返さず人間に前面に出してもらう";
const MCP_HOST_WINDOW_HINT: &str = "何も送っていない。届く先が ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウなので、設定にかかわらず送らない。focus の道具で対象を前面にするのを 1 回だけ試し、focus_failed なら繰り返さず人間に前面に出してもらう";
const MCP_CAPTURE_OCCLUDED_HINT: &str = "何も撮っていない。対象のウィンドウの一部が別のウィンドウ (error.detail の window。見えている部分は visible) に隠れていて、矩形で撮るとそのウィンドウの中身が写り込む。中身を読むだけなら find と text の道具で足りる (隠れていても読める)。画像が要るなら focus の道具を 1 回試し、focus_failed なら人間に前面に出してもらう。focus が通っても同じ foreground_guard が続くなら、繰り返さず find と text で読み、画像が要るならその窓を片付けてもらうよう人間に頼む";
/// 撮る矩形に、設定で拒否されたウィンドウが見えている (MCP。D-077)
const MCP_CAPTURE_SENSITIVE_HINT: &str = "何も撮っていない。撮る矩形に、設定で拒否されたウィンドウ (error.detail の window。見えている部分は visible) が見えている。region (と全画面) なら、そのウィンドウにかからないよう region を狭めるか、window で対象のウィンドウだけを撮る。window で撮っていて対象の手前に重なっているなら、先に focus の道具で対象を前面にしてから撮る (focus は 1 回だけ。focus_failed なら繰り返さない)。中身を読むだけなら find と text の道具で足りる。そのウィンドウを閉じる・動かすのは人間に頼む。設定を緩めるよう頼まない";
const MCP_HOST_UNKNOWN_HINT: &str = "何も送っていない。ai-cu を起動した側のプロセスを特定できないので、実入力をすべて止めている。focus や再試行では変わらない。読むだけの道具 (windows、shot、find、tree、text、status) は使える。人間に ai-cu の起動のしかたを確かめてもらう";
const MCP_FOCUS_NOT_FOUND_HINT: &str = "何も送っていない。その実行ファイル名 (と題名) の見えているウィンドウが無い。windows の道具で名前と題名を確かめる。起動した直後なら数秒待って windows を見直す";
const MCP_FOCUS_REFUSED_HINT: &str = "何も送っていない。人が別のアプリ (前面のプロセスは message と error.detail にある) を使っているので、Windows が前面の切り替えを断った。繰り返さず、人間に対象を前面に出してもらう。タスクバーや Alt+Tab、click、他のツールで前面にしようとしない";
const MCP_FOCUS_HOST_FRONT_HINT: &str = "何も送っていない。前面はあなたの端末なのに、Windows が前面の切り替えを断った (前面化の補助プロセスを試したかと、その結果は error.detail の helper と helper_result にある)。focus を繰り返さず、人間に対象のウィンドウを前面に出してもらい (クリックでよい)、windows で対象の foreground が true になったのを確かめてから続ける。タスクバー、Alt+Tab、click、他のツールで前面にしようとしない。find と text は前面でなくても読める";
const MCP_FOCUS_REFUSED_UNKNOWN_HINT: &str = "何も送っていない。Windows が前面の切り替えを断った (前面のウィンドウを確かめられない)。focus を繰り返さず、人間に対象を前面に出してもらい、windows で対象の foreground が true になったのを確かめてから続ける。タスクバー、Alt+Tab、click、他のツールで前面にしようとしない";
const MCP_CLOSE_NOT_FOUND_HINT: &str = "何も閉じていない。その実行ファイル名と題名の見えているウィンドウが無い (もう閉じている場合を含む)。windows の道具で名前と題名を確かめる";
/// 押されたままのキー (MCP。D-073)。`KEYS_HELD_HINT` の再試行の部分を `MCP_KEYS_HELD_RETRY` に替えた文
/// (`concat!` は const を参照できないので文字列で書き、テストで `MCP_KEYS_HELD_RETRY` を含むことを見る)
const MCP_KEYS_HELD_HINT: &str = "何も送っていない。キー (Shift、Ctrl、Alt、Win) かマウスのボタンが押されたままなので、入力に混ざらないよう断った (どれかは error.detail)。少し後に status の道具の keys_held を読み直し、空になってから 1 回だけ再試行する。空にならなければ、キーが押されたままになっていないか人間に確かめてもらう。代わりに離す操作を送らない";
/// focus: 人が操作中 (MCP。D-073)。待つ時間は `status` の `retry_after_ms`
const MCP_FOCUS_USER_ACTIVE_HINT: &str = "何も送っていない。人がマウスやキーボードを使っている最中なので、対象を前面にしなかった (ai-cu 自身の run の直後も含む)。status の道具の retry_after_ms だけ待って 1 回だけ再試行し、だめなら人間に確かめる";
/// close: 対象が前面で人が操作中 (MCP。D-073)
const MCP_CLOSE_USER_ACTIVE_HINT: &str = "何も閉じていない。対象のウィンドウが前面にあり、人がマウスやキーボードを使っている最中 (ai-cu 自身の入力の直後も含む) なので閉じなかった。同じ操作列で対象に入力した直後なら、close を別の呼び出しにするか、前に 3 秒以上の wait を置く。そうでなければ status の道具の retry_after_ms だけ待って 1 回だけ再試行し、だめなら人間に確かめる";
/// 10 の既定 (MCP。D-073)
const MCP_USER_ACTIVE_HINT: &str = "何もしていない (shot は撮らず、focus は前面にしていない)。人がマウスやキーボードを使っている最中 (ai-cu 自身の run の直後も含む)。status の道具の retry_after_ms だけ待って 1 回だけ再試行し、だめなら人間に確かめる";
const MCP_CLOSE_DISABLED_HINT: &str = "何も閉じていない。対象のウィンドウは無効 (多くはダイアログ (保存の確認など) を開いている) で、閉じるボタンが押せない状態なので閉じなかった。windows の道具と撮影でダイアログを確かめ、どう答えるかは人間に聞く";

/// 状況に合わせた次の手。
pub fn hint(kind: Hint, surface: Surface) -> &'static str {
    let mcp = surface == Surface::Mcp;
    match kind {
        Hint::MouseGuard if mcp => MCP_MOUSE_GUARD_HINT,
        Hint::MouseGuard => MOUSE_GUARD_HINT,
        Hint::KeyboardGuard if mcp => MCP_KEYBOARD_GUARD_HINT,
        Hint::KeyboardGuard => KEYBOARD_GUARD_HINT,
        Hint::Elevated => ELEVATED_HINT,
        Hint::SelfWindow => SELF_WINDOW_HINT,
        Hint::HostWindow if mcp => MCP_HOST_WINDOW_HINT,
        Hint::HostWindow => HOST_WINDOW_HINT,
        Hint::HostCapture => HOST_CAPTURE_HINT,
        Hint::CaptureOccluded if mcp => MCP_CAPTURE_OCCLUDED_HINT,
        Hint::CaptureOccluded => CAPTURE_OCCLUDED_HINT,
        Hint::CaptureSensitive if mcp => MCP_CAPTURE_SENSITIVE_HINT,
        Hint::CaptureSensitive => CAPTURE_SENSITIVE_HINT,
        Hint::HostUnknown if mcp => MCP_HOST_UNKNOWN_HINT,
        Hint::HostUnknown => HOST_UNKNOWN_HINT,
        Hint::FocusNotFound if mcp => MCP_FOCUS_NOT_FOUND_HINT,
        Hint::FocusNotFound => FOCUS_NOT_FOUND_HINT,
        Hint::FocusMinimized => FOCUS_MINIMIZED_HINT,
        Hint::FocusRefused if mcp => MCP_FOCUS_REFUSED_HINT,
        Hint::FocusRefused => FOCUS_REFUSED_HINT,
        Hint::FocusHostFront if mcp => MCP_FOCUS_HOST_FRONT_HINT,
        Hint::FocusHostFront => FOCUS_HOST_FRONT_HINT,
        Hint::FocusRefusedUnknown if mcp => MCP_FOCUS_REFUSED_UNKNOWN_HINT,
        Hint::FocusRefusedUnknown => FOCUS_REFUSED_UNKNOWN_HINT,
        Hint::FocusDenied => FOCUS_DENIED_HINT,
        Hint::FocusUserActive if mcp => MCP_FOCUS_USER_ACTIVE_HINT,
        Hint::FocusUserActive => FOCUS_USER_ACTIVE_HINT,
        Hint::CloseNotFound if mcp => MCP_CLOSE_NOT_FOUND_HINT,
        Hint::CloseNotFound => CLOSE_NOT_FOUND_HINT,
        Hint::CloseDenied => CLOSE_DENIED_HINT,
        Hint::CloseUserActive if mcp => MCP_CLOSE_USER_ACTIVE_HINT,
        Hint::CloseUserActive => CLOSE_USER_ACTIVE_HINT,
        Hint::CloseDisabled if mcp => MCP_CLOSE_DISABLED_HINT,
        Hint::CloseDisabled => CLOSE_DISABLED_HINT,
        Hint::KeysHeld if mcp => MCP_KEYS_HELD_HINT,
        Hint::KeysHeld => KEYS_HELD_HINT,
        Hint::WindowsHiddenAllowed if mcp => {
            "許可されたアプリの見えるウィンドウが無い。hidden_allowed のプロセスは動いているが、ウィンドウが隠れている (タスクトレイ、非表示、別の仮想デスクトップ)。起動した直後なら wait_for で出るのを待ち、変わらなければ人間にウィンドウを出してもらう。隠れた窓 (見えないか cloaked) の題名とクラスは windows の include_hidden: true で読める。見えているが題名の無い窓やツールウィンドウは出ない"
        }
        Hint::WindowsHiddenAllowed => WINDOWS_HIDDEN_ALLOWED_HINT,
        Hint::WindowsEmpty if mcp => {
            "許可されたアプリの見えるウィンドウが無い。対象が許可 (allow.processes、クラスの規則) に無い (config で確かめ、足すのは人間に頼む) か、まだ起動していない (wait_for で出るのを待つ)。ウィンドウを持たないプロセスは hidden_allowed に出ない"
        }
        Hint::WindowsEmpty => WINDOWS_EMPTY_HINT,
        // コマンド名を含まない文なので、CLI と MCP で同じ (D-081)
        Hint::ScreenWait => SCREEN_WAIT_HINT,
        Hint::PasswordField if mcp => MCP_PASSWORD_FIELD_HINT,
        Hint::PasswordField => PASSWORD_FIELD_HINT,
        Hint::ForegroundChanged if mcp => MCP_FOREGROUND_CHANGED_HINT,
        Hint::ForegroundChanged => FOREGROUND_CHANGED_HINT,
        Hint::LeadUserActive if mcp => MCP_LEAD_USER_ACTIVE_HINT,
        Hint::LeadUserActive => LEAD_USER_ACTIVE_HINT,
        Hint::TargetChanged if mcp => MCP_TARGET_CHANGED_HINT,
        Hint::TargetChanged => TARGET_CHANGED_HINT,
        Hint::DragGuardMidway if mcp => MCP_DRAG_GUARD_MIDWAY_HINT,
        Hint::DragGuardMidway => DRAG_GUARD_MIDWAY_HINT,
    }
}

/// 終了コードごとの既定の次の手。CLI は `ErrorKind::hint` のまま
pub fn default_hint(kind: ErrorKind, surface: Surface) -> &'static str {
    if surface == Surface::Cli {
        return kind.hint();
    }
    match kind {
        ErrorKind::Usage => "引数を直す。道具の inputSchema (tools/list) と guide の道具を見る",
        ErrorKind::InvalidScript => {
            "操作列の JSON を直す。書式は run の道具の inputSchema。未知のフィールドは書かない"
        }
        ErrorKind::PolicyDenied => {
            "人間の設定で拒否されている。回避せず、何を許可してほしいかを人間に伝える。error.fix があれば、その文をそのまま人間に伝える (設定は自分で書き換えない。今の許可は config の道具で確かめられる)"
        }
        ErrorKind::ForegroundGuard => {
            "何も送っていない。入力が届く先のウィンドウが許可されていないか、前面にない。windows の道具で対象を確かめ、先に focus の道具で対象を前面にしてから再試行する (run なら先頭に focus)。focus が focus_failed なら、繰り返さず人間に前面に出してもらう"
        }
        ErrorKind::WindowNotFound => {
            "対象のウィンドウが見つからない、最小化されている、または画面の外。撮影も読み取りもしていない (操作の後の shot オブジェクトや run の screenshot なら、それより前の操作は済んでいるので、やり直す前に状態を見る)。windows の道具で実行ファイル名と題名を確かめる。起動した直後なら数秒待つ。最小化は人間に復元してもらう"
        }
        ErrorKind::ElementNotFound => {
            "何もしていない。その名前と種類で文字列を持つ要素が無い。find の道具で要素の名前と種類を確かめる (name は部分一致、大文字小文字を無視)"
        }
        // 待つ時間を status の道具の retry_after_ms で読む (D-073。CLI には status が無いので CLI の文は変えない)
        ErrorKind::UserActive => MCP_USER_ACTIVE_HINT,
        // 以下はコマンド名を含まない文なので、CLI と同じ文を返す (14 は D-066、15 は D-068、16 は D-113)
        ErrorKind::Internal
        | ErrorKind::Config
        | ErrorKind::Stopped
        | ErrorKind::OsError
        | ErrorKind::ForbiddenDevEnv
        | ErrorKind::FocusFailed
        | ErrorKind::DesktopUnavailable
        | ErrorKind::WaitTimeout
        | ErrorKind::InputBusy => kind.hint(),
    }
}

/// 実行器が操作に入った後の失敗で、hint の頭に足す「実行済み」の一文 (D-065)。`executed` (k) は完了した操作の数、
/// `partial` は失敗した操作の副作用を一部でも起こしたか、`failed_index` (i) は失敗した操作の添字。
/// 足すかどうかの判定はここだけに置く: k == 0 で partial でなければ空文字 (hint は元のまま)。
/// partial のときの結びは「止まった所から先について」で、続く個々の hint の「何も送っていない」は、
/// 止まった束から先の話として読む (type の途中で断ったときに前半と食い違わないように)。
/// 断った (4、7) 場合だけでなく、停止 (6)、OS の失敗 (8)、focus と close の途中にも付くので、中立の語にする
pub fn done_prefix(
    executed: usize,
    partial: bool,
    failed_index: usize,
    surface: Surface,
) -> String {
    if executed == 0 && !partial {
        return String::new();
    }
    let mut s = String::new();
    match executed {
        0 => {}
        1 => s.push_str("actions[0] は実行済み (executed=1)。"),
        k => s.push_str(&format!(
            "actions[0]〜actions[{}] は実行済み (executed={k})。",
            k - 1
        )),
    }
    if partial {
        s.push_str(&format!(
            "actions[{failed_index}] も途中まで行った (partial=true)。"
        ));
    }
    s.push_str(match surface {
        Surface::Cli => {
            "操作ごと打ち直さない。残りは、以下の hint に従い、ai-cu windows や ai-cu shot で状態を見直してから送る。"
        }
        Surface::Mcp => {
            "操作ごと打ち直さない。残りは、以下の hint に従い、windows や shot の道具で状態を見直してから送る。"
        }
    });
    if partial {
        s.push_str(&format!(
            "以下は actions[{failed_index}] の止まった所から先について (それより前に起こしたことはそのまま): "
        ));
    } else {
        s.push_str(&format!(
            "以下は失敗した actions[{failed_index}] について: "
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実行済みの一文 (D-065): 範囲の書き方は k で変わり、何も済んでいなければ空。MCP の文に CLI の書き方を出さない
    #[test]
    fn done_prefix_ranges_surfaces_and_empty() {
        assert_eq!(done_prefix(0, false, 0, Surface::Cli), "");
        assert_eq!(done_prefix(0, false, 0, Surface::Mcp), "");
        assert_eq!(
            done_prefix(1, false, 1, Surface::Cli),
            "actions[0] は実行済み (executed=1)。操作ごと打ち直さない。残りは、以下の hint に従い、ai-cu windows や ai-cu shot で状態を見直してから送る。以下は失敗した actions[1] について: "
        );
        assert_eq!(
            done_prefix(3, false, 3, Surface::Mcp),
            "actions[0]〜actions[2] は実行済み (executed=3)。操作ごと打ち直さない。残りは、以下の hint に従い、windows や shot の道具で状態を見直してから送る。以下は失敗した actions[3] について: "
        );
        assert_eq!(
            done_prefix(0, true, 0, Surface::Cli),
            "actions[0] も途中まで行った (partial=true)。操作ごと打ち直さない。残りは、以下の hint に従い、ai-cu windows や ai-cu shot で状態を見直してから送る。以下は actions[0] の止まった所から先について (それより前に起こしたことはそのまま): "
        );
        assert_eq!(
            done_prefix(2, true, 2, Surface::Mcp),
            "actions[0]〜actions[1] は実行済み (executed=2)。actions[2] も途中まで行った (partial=true)。操作ごと打ち直さない。残りは、以下の hint に従い、windows や shot の道具で状態を見直してから送る。以下は actions[2] の止まった所から先について (それより前に起こしたことはそのまま): "
        );
        assert!(
            done_prefix(2, true, 2, Surface::Cli)
                .starts_with("actions[0]〜actions[1] は実行済み (executed=2)。actions[2] も途中まで行った (partial=true)。")
        );
        // partial のときは「失敗した…について」を言わない (続く「何も送っていない」と食い違うため)
        assert!(!done_prefix(0, true, 0, Surface::Cli).contains("以下は失敗した"));
        assert!(!done_prefix(2, true, 2, Surface::Mcp).contains("以下は失敗した"));
        for (k, p) in [(1, false), (3, false), (0, true), (2, true)] {
            let s = done_prefix(k, p, k, Surface::Mcp);
            assert!(!s.contains("ai-cu"), "{s}");
            assert!(!s.contains("--"), "{s}");
        }
    }

    const SURFACES: [Surface; 2] = [Surface::Cli, Surface::Mcp];

    /// MCP の文は道具の名前で書く (CLI の書き方を出さない)。既定の文も含めて総当たりで見る
    #[test]
    fn mcp_hints_never_mention_cli_syntax() {
        let all: Vec<&str> = Hint::ALL
            .iter()
            .map(|h| hint(*h, Surface::Mcp))
            .chain(
                ErrorKind::ALL
                    .iter()
                    .map(|k| default_hint(*k, Surface::Mcp)),
            )
            .collect();
        for h in all {
            // `ai-cu windows` のようなサブコマンドの書き方 (「ai-cu を起動した側」のような地の文はよい)
            let subcommand = h
                .match_indices("ai-cu ")
                .any(|(i, m)| h[i + m.len()..].starts_with(|c: char| c.is_ascii_alphabetic()));
            assert!(!subcommand, "{h}");
            assert!(!h.contains("`ai-cu"), "{h}");
            assert!(!h.contains("--"), "{h}");
        }
    }

    /// hidden_allowed の hint は、隠れた窓そのものを読む旗を呼ばれ方の書き方で伝え、
    /// 見えているが題名の無い窓やツールウィンドウは hidden_windows に出ないことを言う (D-099)
    #[test]
    fn windows_hidden_allowed_hint_mentions_include_hidden() {
        let cli = hint(Hint::WindowsHiddenAllowed, Surface::Cli);
        assert!(cli.contains("--include-hidden"), "{cli}");
        let mcp = hint(Hint::WindowsHiddenAllowed, Surface::Mcp);
        assert!(mcp.contains("include_hidden"), "{mcp}");
        assert!(!mcp.contains("--include-hidden"), "{mcp}");
        for h in [cli, mcp] {
            assert!(h.contains("ツールウィンドウ"), "{h}");
        }
    }

    /// CLI の文は D-056 の前と一字一句同じ (定数と `ErrorKind::hint` をそのまま返す)
    #[test]
    fn cli_hints_unchanged() {
        let expected = [
            (Hint::MouseGuard, MOUSE_GUARD_HINT),
            (Hint::KeyboardGuard, KEYBOARD_GUARD_HINT),
            (Hint::Elevated, ELEVATED_HINT),
            (Hint::SelfWindow, SELF_WINDOW_HINT),
            (Hint::HostWindow, HOST_WINDOW_HINT),
            (Hint::HostCapture, HOST_CAPTURE_HINT),
            (Hint::CaptureOccluded, CAPTURE_OCCLUDED_HINT),
            (Hint::FocusNotFound, FOCUS_NOT_FOUND_HINT),
            (Hint::FocusMinimized, FOCUS_MINIMIZED_HINT),
            (Hint::FocusRefused, FOCUS_REFUSED_HINT),
            (Hint::FocusDenied, FOCUS_DENIED_HINT),
            (Hint::FocusUserActive, FOCUS_USER_ACTIVE_HINT),
            (Hint::ScreenWait, SCREEN_WAIT_HINT),
        ];
        for (h, s) in expected {
            assert_eq!(hint(h, Surface::Cli), s, "{h:?}");
        }
        // 文の中身も固定する。D-056 の直前 (0460a92) の src/run.rs から一字一句写した値と比べる
        // (上の比較は hint() が定数を返すことしか見ないので、定数そのものの書き換えはここで捕まえる)
        let snapshot: [(&str, &str); 12] = [
            (
                MOUSE_GUARD_HINT,
                "何も送っていない。クリックする点の下に、許可されていないウィンドウ (タスクバーや別のアプリ) がある。先に focus で対象を前面にし (一手ずつなら `ai-cu focus`、操作列なら先頭に focus)、`ai-cu windows` の rect の中の点を使う。focus が 11 なら、人間に前面に出してもらう",
            ),
            (
                KEYBOARD_GUARD_HINT,
                "何も送っていない。キーと文字は前面のウィンドウに届くが、前面が許可されたウィンドウではない。先に focus で対象を前面にしてから送る (一手ずつなら `ai-cu focus`、操作列なら先頭に focus)。focus が 11 なら、人間に前面に出してもらう",
            ),
            (
                ELEVATED_HINT,
                "何も送っていない。管理者権限のウィンドウ (またはそう判定できないウィンドウ) には送れない。人間に操作してもらう",
            ),
            (
                SELF_WINDOW_HINT,
                "何も送っていない。届く先が ai-cu を動かしている端末のウィンドウなので送らない。カウントダウンの間に対象のウィンドウを前面にするか、操作列の先頭に対象への focus を入れる",
            ),
            (
                FOCUS_NOT_FOUND_HINT,
                "何も送っていない。その実行ファイル名 (と題名) の見えているウィンドウが無い。`ai-cu windows` で名前と題名を確かめる。起動した直後なら数秒待って `windows` を見直す",
            ),
            (
                FOCUS_MINIMIZED_HINT,
                "何も送っていない。対象は最小化されている。ai-cu は復元しない (人が別のアプリを使っていても前面を奪えてしまうため)。人間に復元してもらう",
            ),
            (
                FOCUS_REFUSED_HINT,
                "何も送っていない。Windows が前面の切り替えを断った (人が別のアプリを使っている)。繰り返さず、人間に対象を前面に出してもらう。タスクバーや Alt+Tab、他のツールで前面にしようとしない",
            ),
            (
                FOCUS_DENIED_HINT,
                "何も送っていない。前面にしようとしたウィンドウが許可されていない。何を許可してほしいかを人間に伝える",
            ),
            (
                FOCUS_USER_ACTIVE_HINT,
                "何も送っていない。人がマウスやキーボードを使っている最中なので、対象を前面にしなかった (ai-cu 自身の run の直後も含む)。数秒待って 1 回だけ再試行し、だめなら人間に確かめる",
            ),
            (
                HOST_WINDOW_HINT,
                "何も送っていない。届く先が ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウなので、設定にかかわらず送らない。先に focus で対象のアプリを前面にしてから送る",
            ),
            (
                HOST_CAPTURE_HINT,
                "何もしていない。対象が ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウなので、設定にかかわらず撮らず、読まない。別のアプリを対象にする",
            ),
            (
                CAPTURE_OCCLUDED_HINT,
                "何も撮っていない。対象のウィンドウの一部が別のウィンドウに隠れていて、矩形で撮るとそのウィンドウの中身が写り込む。先に focus で対象を前面にしてから撮る (一手ずつなら `ai-cu focus PROCESS --shot PATH --window PROCESS`)",
            ),
        ];
        for (now, before) in snapshot {
            assert_eq!(now, before);
        }
        for k in ErrorKind::ALL {
            assert_eq!(default_hint(k, Surface::Cli), k.hint(), "{k:?}");
        }
        // 断られた理由が人と断定できないときの CLI の文は、人が別のアプリを使っているとは言わない
        for h in [Hint::FocusHostFront, Hint::FocusRefusedUnknown] {
            assert!(!hint(h, Surface::Cli).contains("人が別のアプリ"), "{h:?}");
        }
    }

    /// 撮影の隠れの MCP の文は、覆う窓を error.detail で読めることと、focus が通っても foreground_guard が続くときの次の手を書く。
    /// 他の MCP の文と同じく失敗は code の名前で書く (終了コードの数は CLI の書き方)。
    /// 写り込みの守りの迂回になる region は勧めず、原因が未確認の IME の名は書かない。CLI の文は D-056 の固定のまま (D-135)
    #[test]
    fn mcp_occluded_hint_says_what_to_do_when_focus_does_not_help() {
        let mcp = hint(Hint::CaptureOccluded, Surface::Mcp);
        for s in ["error.detail", "find", "人間"] {
            assert!(mcp.contains(s), "{s}: {mcp}");
        }
        // 次の手は「focus が通っても」の後ろに、繰り返さないことと、窓を片付けてもらうことを書く
        let (_, after) = mcp
            .split_once("focus が通っても同じ foreground_guard が続くなら")
            .unwrap_or_else(|| panic!("focus が通っても: {mcp}"));
        for s in ["繰り返さず", "find", "片付け", "人間"] {
            assert!(after.contains(s), "{s}: {after}");
        }
        for s in ["region", "IME"] {
            assert!(!mcp.contains(s), "{s}: {mcp}");
        }
        let cli = hint(Hint::CaptureOccluded, Surface::Cli);
        assert!(!cli.contains("focus が通っても"), "{cli}");
    }

    /// `Hint::ALL` が全ての種類を 1 回ずつ並べている (総当たりの検査はこの一覧を回るので、足し忘れを捕まえる)。
    /// 種類を足すと、下の match に腕が要り (網羅の検査)、番号が ALL の長さを超えるので失敗する
    #[test]
    fn hint_all_lists_every_variant_once() {
        fn index(h: Hint) -> usize {
            match h {
                Hint::MouseGuard => 0,
                Hint::KeyboardGuard => 1,
                Hint::Elevated => 2,
                Hint::SelfWindow => 3,
                Hint::HostWindow => 4,
                Hint::HostCapture => 5,
                Hint::CaptureOccluded => 6,
                Hint::HostUnknown => 7,
                Hint::FocusNotFound => 8,
                Hint::FocusMinimized => 9,
                Hint::FocusRefused => 10,
                Hint::FocusHostFront => 11,
                Hint::FocusRefusedUnknown => 12,
                Hint::FocusDenied => 13,
                Hint::FocusUserActive => 14,
                Hint::CloseNotFound => 15,
                Hint::CloseDenied => 16,
                Hint::CloseUserActive => 17,
                Hint::CloseDisabled => 18,
                Hint::KeysHeld => 19,
                Hint::WindowsHiddenAllowed => 20,
                Hint::WindowsEmpty => 21,
                Hint::CaptureSensitive => 22,
                Hint::ScreenWait => 23,
                Hint::PasswordField => 24,
                Hint::ForegroundChanged => 25,
                Hint::LeadUserActive => 26,
                Hint::TargetChanged => 27,
                Hint::DragGuardMidway => 28,
            }
        }
        let mut seen: Vec<usize> = Hint::ALL.iter().map(|h| index(*h)).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..Hint::ALL.len()).collect::<Vec<_>>());
    }

    #[test]
    fn every_kind_reason_surface_has_hint() {
        for s in SURFACES {
            for h in Hint::ALL {
                assert!(!hint(h, s).is_empty(), "{h:?} {s:?}");
            }
            for k in ErrorKind::ALL {
                assert!(!default_hint(k, s).is_empty(), "{k:?} {s:?}");
            }
        }
        // MCP で文を分けた種類は、CLI と違う文になっている
        for h in [
            Hint::MouseGuard,
            Hint::KeyboardGuard,
            Hint::HostWindow,
            Hint::CaptureOccluded,
            Hint::CaptureSensitive,
            Hint::HostUnknown,
            Hint::FocusNotFound,
            Hint::FocusRefused,
            Hint::FocusHostFront,
            Hint::FocusRefusedUnknown,
            Hint::CloseNotFound,
            Hint::CloseDisabled,
            Hint::FocusUserActive,
            Hint::CloseUserActive,
            Hint::KeysHeld,
            Hint::WindowsHiddenAllowed,
            Hint::WindowsEmpty,
            Hint::PasswordField,
            Hint::ForegroundChanged,
            Hint::LeadUserActive,
            Hint::TargetChanged,
            Hint::DragGuardMidway,
        ] {
            assert_ne!(hint(h, Surface::Cli), hint(h, Surface::Mcp), "{h:?}");
        }
        // 起動した側を特定できないときは、focus を次の手として勧めない
        for s in SURFACES {
            let h = hint(Hint::HostUnknown, s);
            assert!(h.contains("focus や再試行では変わらない"), "{h}");
            // 読み取りの tree はホスト不明でも通るので、使えるものの列挙に入れる (D-084)
            assert!(h.contains("find、tree、text"), "{h}");
        }
        // 前面がホストで断られたときは、人間に頼み、windows の foreground で確かめると書く
        let h = hint(Hint::FocusHostFront, Surface::Mcp);
        assert!(h.contains("人間") && h.contains("foreground"), "{h}");
        assert!(!h.contains("人が別のアプリ"), "{h}");
        // 補助プロセスでも断られた後の文 (D-057)。補助を試したことを書く
        assert!(h.contains("補助プロセス"), "{h}");
    }

    /// 人が離れれば解ける 10 の MCP の文は status の道具に向け、CLI の文は status を含まない (D-073)。
    /// keys_held の MCP の文は status の note と同じ再試行の手順 (`MCP_KEYS_HELD_RETRY`) を含む
    #[test]
    fn mcp_user_active_hints_point_to_status_and_cli_hints_do_not() {
        let pairs = [
            (
                default_hint(ErrorKind::UserActive, Surface::Mcp),
                default_hint(ErrorKind::UserActive, Surface::Cli),
            ),
            (
                hint(Hint::FocusUserActive, Surface::Mcp),
                hint(Hint::FocusUserActive, Surface::Cli),
            ),
            (
                hint(Hint::CloseUserActive, Surface::Mcp),
                hint(Hint::CloseUserActive, Surface::Cli),
            ),
            (
                hint(Hint::KeysHeld, Surface::Mcp),
                hint(Hint::KeysHeld, Surface::Cli),
            ),
            (
                hint(Hint::LeadUserActive, Surface::Mcp),
                hint(Hint::LeadUserActive, Surface::Cli),
            ),
        ];
        for (mcp, cli) in pairs {
            assert!(mcp.contains("status"), "{mcp}");
            assert!(!cli.contains("status"), "{cli}");
            assert!(!mcp.contains("数秒待って"), "{mcp}");
        }
        for h in [
            Hint::FocusUserActive,
            Hint::CloseUserActive,
            Hint::LeadUserActive,
        ] {
            assert!(hint(h, Surface::Mcp).contains("retry_after_ms"), "{h:?}");
        }
        assert!(default_hint(ErrorKind::UserActive, Surface::Mcp).contains("retry_after_ms"));
        let keys = hint(Hint::KeysHeld, Surface::Mcp);
        assert!(keys.contains(MCP_KEYS_HELD_RETRY), "{keys}");
        assert!(keys.contains("代わりに離す操作を送らない"), "{keys}");
        assert_eq!(hint(Hint::KeysHeld, Surface::Cli), KEYS_HELD_HINT);
    }

    /// 撮る矩形に deny のウィンドウが見えている 7 (D-077): CLI は定数のまま、MCP は道具の名前で書く。
    /// どちらも機密を見せる向きに人を誘わず (設定を緩めるよう頼まない)、region を狭める手と focus の手の両方を書く
    #[test]
    fn capture_sensitive_hint_offers_region_and_focus_and_never_asks_to_loosen() {
        assert_eq!(
            hint(Hint::CaptureSensitive, Surface::Cli),
            CAPTURE_SENSITIVE_HINT
        );
        for s in SURFACES {
            let h = hint(Hint::CaptureSensitive, s);
            assert!(h.starts_with("何も撮っていない。"), "{h}");
            assert!(h.contains("設定を緩めるよう頼まない"), "{h}");
            assert!(h.contains("狭める") && h.contains("focus"), "{h}");
            assert!(h.contains("visible"), "{h}");
        }
        assert!(CAPTURE_SENSITIVE_HINT.contains("`--region`"));
        assert!(!hint(Hint::CaptureSensitive, Surface::Mcp).contains("--"));
    }

    /// パスワード欄の 4 と、読む間に前面が変わった 7 の文 (D-090): 何も送っていないこと、パスワードは人間に打って
    /// もらうこと、Enter や Tab も断ること、許す設定の名前を書く。前面が変わった文は「許可されていない」と言わない
    #[test]
    fn password_field_and_foreground_changed_hints() {
        for s in SURFACES {
            let h = hint(Hint::PasswordField, s);
            for word in [
                "何も送っていない",
                "人間に打ってもらう",
                "Enter や Tab",
                "general.password_input",
                "自分で書き換えない",
            ] {
                assert!(h.contains(word), "{word}: {h}");
            }
            let h = hint(Hint::ForegroundChanged, s);
            assert!(h.starts_with("何も送っていない。"), "{h}");
            assert!(h.contains("前面のウィンドウが変わった"), "{h}");
            assert!(!h.contains("許可されていない"), "{h}");
        }
    }

    /// 途中で前面が変わった (D-100): 途中まで送ったと言い、残りだけを送るよう勧め、全文の送り直しを止める
    #[test]
    fn target_changed_hint_says_partially_sent() {
        for s in SURFACES {
            let h = hint(Hint::TargetChanged, s);
            assert!(h.starts_with("途中まで送った。"), "{h}");
            assert!(!h.contains("何も送っていない"), "{h}");
            for word in ["pinned_pid", "残りだけを送る", "二重に入る"] {
                assert!(h.contains(word), "{word}: {h}");
            }
        }
    }

    /// ドラッグの途中で断られた (D-132): 途中まで送ったと言い、断った点、離したこと、rect の読み直し、歩幅の目安、
    /// 新しい始点で作ること、窓の外へ運ぶときの手、組み直しの上限を書く。歩幅の引数は呼ばれ方で書き分ける
    #[test]
    fn drag_guard_midway_hint_has_required_parts() {
        for s in SURFACES {
            let h = hint(Hint::DragGuardMidway, s);
            assert!(h.starts_with("途中まで送った"), "{h}");
            assert!(!h.contains("何も送っていない"), "{h}");
            for word in [
                "error.detail.point",
                "windows",
                "rect",
                "離し",
                "半分",
                "50 ms",
                "送り直さず",
                "新しい始点",
                "1 回だけ",
                "人間",
                "書き換えない",
                "ガードの回避ではない",
            ] {
                assert!(h.contains(word), "{word}: {h}");
            }
        }
        let cli = hint(Hint::DragGuardMidway, Surface::Cli);
        assert!(cli.contains("--steps"), "{cli}");
        assert!(cli.contains("--duration-ms"), "{cli}");
        let mcp = hint(Hint::DragGuardMidway, Surface::Mcp);
        assert!(mcp.contains("steps"), "{mcp}");
        assert!(mcp.contains("duration_ms"), "{mcp}");
    }

    /// 14 の既定の文 (D-066): CLI と MCP で同じ文で、コマンド名を含まず、何も送っていないと言い切らない
    #[test]
    fn desktop_unavailable_hint_is_same_on_both_surfaces_without_command_names() {
        let mcp = default_hint(ErrorKind::DesktopUnavailable, Surface::Mcp);
        assert_eq!(
            mcp,
            default_hint(ErrorKind::DesktopUnavailable, Surface::Cli)
        );
        assert!(!mcp.contains("ai-cu "), "{mcp}");
        assert!(!mcp.contains("--"), "{mcp}");
        assert!(mcp.contains("executed が無ければ"), "{mcp}");
        assert!(mcp.contains("再試行しても直らない"), "{mcp}");
        // ロック中でも使える操作に wait_for だけの操作列を含める (D-068)
        assert!(mcp.contains("wait_for"), "{mcp}");
    }

    /// 15 の既定の文 (D-068): CLI と MCP で同じ文で、コマンド名を含まない。何も送っていないこと、同じ条件で
    /// 繰り返し待たないこと、状態と条件を見直すことを書く
    #[test]
    fn wait_timeout_hint_is_same_on_both_surfaces_without_command_names() {
        let mcp = default_hint(ErrorKind::WaitTimeout, Surface::Mcp);
        assert_eq!(mcp, default_hint(ErrorKind::WaitTimeout, Surface::Cli));
        assert_eq!(mcp, ErrorKind::WaitTimeout.hint());
        assert!(!mcp.contains("ai-cu "), "{mcp}");
        assert!(!mcp.contains("--"), "{mcp}");
        assert!(mcp.contains("何も送っていない"), "{mcp}");
        assert!(mcp.contains("繰り返し待たない"), "{mcp}");
        assert!(mcp.contains("executed"), "{mcp}");
        for word in ["windows", "find", "text", "shot"] {
            assert!(mcp.contains(word), "{word}: {mcp}");
        }
    }

    /// 画面の待ちの 15 の文 (D-081): CLI と MCP で同じ文で、コマンド名を含まない。15 の既定の文 (wait_for 向け)
    /// とは違う文で、何も送っていないこと、繰り返し待たないこと、region と threshold の手、基準の注意を書く
    #[test]
    fn screen_wait_hint_is_same_on_both_surfaces_without_command_names() {
        let mcp = hint(Hint::ScreenWait, Surface::Mcp);
        assert_eq!(mcp, hint(Hint::ScreenWait, Surface::Cli));
        assert_eq!(mcp, SCREEN_WAIT_HINT);
        assert_ne!(mcp, ErrorKind::WaitTimeout.hint());
        assert!(!mcp.contains("ai-cu"), "{mcp}");
        assert!(!mcp.contains("--"), "{mcp}");
        for word in [
            "何も送っていない",
            "繰り返し待たない",
            "executed",
            "shot",
            "region",
            "threshold",
            "基準",
        ] {
            assert!(mcp.contains(word), "{word}: {mcp}");
        }
    }

    /// 16 の既定の文 (D-113): 終了コード 16 と `input_busy`、CLI と MCP で同じ文で、空でなく、コマンドの書き方を
    /// 含まない (文に「別の ai-cu (」を含むので、`mcp_hints_never_mention_cli_syntax` と同じく「ai-cu 」の後ろが
    /// 英字でないことで見る)。何も送っていないこと、1 回だけやり直すこと、続くなら人に確かめることを書く
    #[test]
    fn input_busy_exit_code_and_hint() {
        assert_eq!(ErrorKind::InputBusy.exit_code(), 16);
        assert_eq!(ErrorKind::InputBusy.code(), "input_busy");
        let mcp = default_hint(ErrorKind::InputBusy, Surface::Mcp);
        let cli = default_hint(ErrorKind::InputBusy, Surface::Cli);
        assert_eq!(mcp, cli);
        assert_eq!(cli, ErrorKind::InputBusy.hint());
        assert!(!mcp.is_empty());
        let subcommand = mcp
            .match_indices("ai-cu ")
            .any(|(i, m)| mcp[i + m.len()..].starts_with(|c: char| c.is_ascii_alphabetic()));
        assert!(!subcommand, "{mcp}");
        assert!(!mcp.contains("`ai-cu"), "{mcp}");
        assert!(!mcp.contains("--"), "{mcp}");
        for word in ["何も送っていない", "別の ai-cu", "1 回だけ", "人間"] {
            assert!(mcp.contains(word), "{word}: {mcp}");
        }
    }

    #[test]
    fn window_not_found_hint_fits_find_and_text_and_shot_after_action() {
        // 12 は撮影 (D-053) だけでなく find / text (D-054) からも返る。撮影に限った言い方をせず、
        // 操作の後の撮影で返ったときに「何もしていない」と誤らない
        for s in SURFACES {
            let h = default_hint(ErrorKind::WindowNotFound, s);
            assert!(!h.contains("撮る対象"), "{h}");
            assert!(!h.starts_with("何もしていない"), "{h}");
            assert!(h.contains("撮影も読み取りもしていない"), "{h}");
            assert!(h.contains("それより前の操作は済んでいる"), "{h}");
        }
    }
}
