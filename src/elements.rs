//! UI Automation の要素 (D-054)。純粋な型と判定だけ。COM は `win32::uia` に閉じ込める。

use serde::Serialize;

use crate::geometry::{Point, Rect};

/// UI Automation の要素 (`find` と `text` の出力)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiElement {
    pub name: String,
    /// 種類 (snake_case。例 `button`、`edit`、`document`、`window`)
    #[serde(rename = "type")]
    pub control_type: String,
    pub automation_id: String,
    pub class: String,
    /// 画面上の矩形 (物理ピクセル)。無ければ None
    pub rect: Option<Rect>,
    pub enabled: bool,
    pub offscreen: bool,
    /// キーボードのフォーカスを持つか (UIA の HasKeyboardFocus)。find の問い合わせの経路でだけ読み、
    /// `tree` (describe) では読まずに false のまま (D-089)
    pub focused: bool,
    /// パスワードの要素か (UIA の IsPassword)。find の経路でだけ読み、tree (describe) では読まずに false のまま。
    /// 読めなければ false (D-090)
    pub password: bool,
    /// トグルの状態 (`"off"` / `"on"` / `"indeterminate"`)。状態はパターンを持つ要素で、find の結果に入るときだけ
    /// 読む。パターンが無いか読めなければ None (欄ごと出さない。D-089)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle: Option<String>,
    /// 選ばれているか (SelectionItemPattern)。読み方は `toggle` と同じ (D-089)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// 開閉の状態 (`"collapsed"` / `"expanded"` / `"partially_expanded"` / `"leaf_node"`)。読み方は `toggle` と同じ (D-089)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<String>,
    /// 文字列 (`with_text` のときだけ読む)。持たない要素は None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl UiElement {
    /// 矩形の中心 (クリックに渡す点)
    pub fn center(&self) -> Option<Point> {
        self.rect.as_ref().map(Rect::center)
    }
}

/// 要素の絞り込み。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementQuery {
    /// 名前の部分一致 (大文字小文字を無視)
    pub name: Option<String>,
    /// 種類 (snake_case)
    pub control_type: Option<String>,
    /// automation_id の完全一致 (大文字小文字を区別。D-089)
    pub automation_id: Option<String>,
    /// キーボードのフォーカスを持つ要素だけ (D-089)
    pub focused: bool,
    /// 一致した要素の上限 (超えた分は捨てて truncated を立てる)
    pub limit: usize,
    /// 文字列を読む。一致した要素のうち、文字列を持つ最初の 1 つだけを (文字列つきで) 返して打ち切る。
    /// パスワードの要素は文字列を持っていても飛ばす (`skips_text_of_password`、D-090)
    pub with_text: bool,
}

impl ElementQuery {
    /// 文字列を読む問い合わせ (`text` の道具と `wait_for` の文字列の条件。D-054、D-068)。
    /// `with_text` では実装置 (src/win32/uia.rs の `consider`) もモックも `limit` を見ずに、文字列を持つ最初の要素で
    /// 打ち切るので、`limit` は形式上の値。パスワードの要素は文字列を持っていても飛ばす
    /// (`skips_text_of_password`、D-090)
    pub fn for_text(name: Option<String>, control_type: Option<String>) -> ElementQuery {
        ElementQuery {
            name,
            control_type,
            automation_id: None,
            focused: false,
            limit: 200,
            with_text: true,
        }
    }

    /// フォーカス以外の判定: 名前の部分一致 (大文字小文字を無視)、種類の一致、automation_id の完全一致
    /// (大文字小文字を区別) の AND。判定の本体はここ 1 か所 (D-089)。実装置はフォーカスを読む前にこれで捨てる
    pub fn matches_except_focus(&self, e: &UiElement) -> bool {
        let name_ok = self
            .name
            .as_deref()
            .is_none_or(|n| e.name.to_lowercase().contains(&n.to_lowercase()));
        let type_ok = self
            .control_type
            .as_deref()
            .is_none_or(|t| e.control_type == t);
        let aid_ok = self
            .automation_id
            .as_deref()
            .is_none_or(|a| e.automation_id == a);
        name_ok && type_ok && aid_ok
    }

    /// 要素の判定 (`matches_except_focus` にフォーカスを足したもの)。実装置もモックもこれで結果に入れるかを決める
    pub fn matches_element(&self, e: &UiElement) -> bool {
        self.matches_except_focus(e) && (!self.focused || e.focused)
    }

    /// 状態 (toggle / selected / expanded) を読む問い合わせか。文字列を読む問い合わせ (`text` と `wait_for` の
    /// 文字列の条件) では読まない。実装置 (src/win32/uia.rs の `consider`) とモックがこれで分ける (D-089)
    pub fn reads_states(&self) -> bool {
        !self.with_text
    }

    /// 文字列を読む問い合わせで、パスワードの要素 (`password` が真) を読まずに飛ばすか。判定の本体はここ 1 か所
    /// (D-090)。実装置 (src/win32/uia.rs の `consider`) とモックがこれを呼び、真なら文字列を読まずに
    /// `Elements::password_skipped` に数える
    pub fn skips_text_of_password(&self, password: bool) -> bool {
        self.with_text && password
    }
}

/// UIA の ToggleState の値を名前にする (Microsoft Learn「ToggleState enumeration (uiautomationcore.h)」:
/// Off 0、On 1、Indeterminate 2)。知らない値は None (D-089)
pub fn toggle_state_name(v: i32) -> Option<&'static str> {
    match v {
        0 => Some("off"),
        1 => Some("on"),
        2 => Some("indeterminate"),
        _ => None,
    }
}

/// UIA の ExpandCollapseState の値を名前にする (Microsoft Learn「ExpandCollapseState enumeration
/// (uiautomationcore.h)」: Collapsed 0、Expanded 1、PartiallyExpanded 2、LeafNode 3)。知らない値は None (D-089)
pub fn expand_collapse_state_name(v: i32) -> Option<&'static str> {
    match v {
        0 => Some("collapsed"),
        1 => Some("expanded"),
        2 => Some("partially_expanded"),
        3 => Some("leaf_node"),
        _ => None,
    }
}

/// `ui_elements` の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Elements {
    pub elements: Vec<UiElement>,
    /// 上限で打ち切った
    pub truncated: bool,
    /// 一覧の取得の後に消えて読めなかった要素の数 (飛ばした)
    pub skipped: usize,
    /// 文字列を読む問い合わせで、パスワードの要素なので読まずに飛ばした数 (D-090)
    pub password_skipped: usize,
}

/// CR / CRLF を LF にそろえる。要素の文字列 (`first_text`) と、`wait_for` の `text` の条件の両方に掛けて、
/// 呼び手が CRLF で書いても画面の表示と比べられるようにする (D-068)
pub fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// 読んだ要素の列から、最初に文字列を持つ要素と、その文字列を CR / CRLF を LF にそろえたもの。無ければ None。
/// `text` の道具と `wait_for` で共有する判定 (D-054、D-068)
pub fn first_text(elements: &[UiElement]) -> Option<(&UiElement, String)> {
    elements
        .iter()
        .find_map(|e| e.text.as_ref().map(|t| (e, t)))
        .map(|(e, t)| (e, normalize_newlines(t)))
}

/// `text` の本文の上限 (字。Rust の `char` の数) の既定 (D-102)
pub const TEXT_MAX_CHARS_DEFAULT: usize = 20_000;
/// `text` の本文の上限の最大 (範囲は 1〜これ。メモリと JSON の大きさの歯止め。D-102)
pub const TEXT_MAX_CHARS_MAX: usize = 1_000_000;
/// 要素の JSON の `name` の上限 (字)。超えたら先頭だけを残し、`name_truncated: true` を足す (「…」は付けない。D-102)
pub const ELEMENT_NAME_MAX_CHARS: usize = 500;

/// 文字列を先頭 `max` 個の `char` に切る。戻り値は (切った文字列、切ったか、元の `char` の数)。
/// `max` 以下ならそのまま返す。`char` 単位なのでサロゲートペアの途中では切れない
/// (結合文字の列の途中では切れうるが、文字列としては壊れない。D-102)
pub fn truncate_chars(s: &str, max: usize) -> (String, bool, usize) {
    let total = s.chars().count();
    if total <= max {
        return (s.to_string(), false, total);
    }
    (s.chars().take(max).collect(), true, total)
}

/// 要素の JSON (`find` / `text` の出力と `wait_for` の `waited[]` で共有)。`rect` の中心を `center` (`{x, y}` か null) で
/// 足し、文字列 (`text`) は落とす (文字列は `text` の道具が別に返す。`waited[]` には入れない)。
/// `name` は先頭 `ELEMENT_NAME_MAX_CHARS` 字に切り、切った要素にだけ `name_truncated: true` を足す (絞り込みは
/// 切る前の全文で済んでいる。D-102)
pub fn element_json(e: &UiElement) -> serde_json::Value {
    let mut v = serde_json::to_value(e).expect("UiElement は JSON にできる");
    v["center"] = e.center().map_or(
        serde_json::Value::Null,
        |p| serde_json::json!({ "x": p.x, "y": p.y }),
    );
    let (name, name_truncated, _) = truncate_chars(&e.name, ELEMENT_NAME_MAX_CHARS);
    v["name"] = serde_json::Value::String(name);
    if name_truncated {
        v["name_truncated"] = serde_json::Value::Bool(true);
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("text");
    }
    v
}

/// UIA の control type の id と snake_case の名前 (UIAutomationClient.h の UIA_*ControlTypeId)。
const CONTROL_TYPES: [(i32, &str); 41] = [
    (50000, "button"),
    (50001, "calendar"),
    (50002, "check_box"),
    (50003, "combo_box"),
    (50004, "edit"),
    (50005, "hyperlink"),
    (50006, "image"),
    (50007, "list_item"),
    (50008, "list"),
    (50009, "menu"),
    (50010, "menu_bar"),
    (50011, "menu_item"),
    (50012, "progress_bar"),
    (50013, "radio_button"),
    (50014, "scroll_bar"),
    (50015, "slider"),
    (50016, "spinner"),
    (50017, "status_bar"),
    (50018, "tab"),
    (50019, "tab_item"),
    (50020, "text"),
    (50021, "tool_bar"),
    (50022, "tool_tip"),
    (50023, "tree"),
    (50024, "tree_item"),
    (50025, "custom"),
    (50026, "group"),
    (50027, "thumb"),
    (50028, "data_grid"),
    (50029, "data_item"),
    (50030, "document"),
    (50031, "split_button"),
    (50032, "window"),
    (50033, "pane"),
    (50034, "header"),
    (50035, "header_item"),
    (50036, "table"),
    (50037, "title_bar"),
    (50038, "separator"),
    (50039, "semantic_zoom"),
    (50040, "app_bar"),
];

/// control type の id を名前にする。未知なら `control_type_<id>`
pub fn control_type_name(id: i32) -> String {
    CONTROL_TYPES
        .iter()
        .find(|(i, _)| *i == id)
        .map_or_else(|| format!("control_type_{id}"), |(_, n)| (*n).to_string())
}

/// 種類の名前を正規化する (大文字小文字と `-` / 空白 / `_` の違いを無視)。未知なら None
pub fn parse_control_type(s: &str) -> Option<&'static str> {
    let norm: String = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let joined = norm.replace('_', "");
    CONTROL_TYPES
        .iter()
        .map(|(_, n)| *n)
        .find(|n| *n == norm || n.replace('_', "") == joined)
}

/// 種類の名前の一覧 (`--help` と schema 用)
pub fn control_type_names() -> Vec<&'static str> {
    CONTROL_TYPES.iter().map(|(_, n)| *n).collect()
}

/// `tree` の深さの既定 (根が 0。D-084)
pub const TREE_DEPTH_DEFAULT: usize = 8;
/// `tree` の深さの上限 (範囲は 1〜これ)
pub const TREE_DEPTH_MAX: usize = 50;
/// `tree` の行の既定の上限
pub const TREE_LIMIT_DEFAULT: usize = 200;
/// `tree` の行の上限の最大 (範囲は 1〜これ)
pub const TREE_LIMIT_MAX: usize = 2000;
/// `tree` が試す要素の上限 (読めた要素と読めなかった要素の和。時間の上限)
pub const TREE_MAX_NODES: usize = 5000;
/// 名前をこの文字数 (char) を超えたら切る (79 文字 + `…`)
const TREE_NAME_MAX_CHARS: usize = 80;

/// `tree` の出力の `note` (CLI と MCP で共有。D-084)
pub const TREE_NOTE: &str = "@x,y は要素の中心 (物理ピクセル) で、そのまま click X Y に渡せる。\
名前の無い pane / group / custom と画面外の要素は畳んである (all で全部)。\
truncated は行 (limit) か、たどる要素の上限で切ったこと。depth_cut が 0 でなければ depth を上げると深く見える。\
skipped が 0 でなければ、読めなかった要素か枝があり、木の一部が欠けている。矩形や automation_id は find で引く";

/// 木の 1 要素 (前順。根が深さ 0)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub depth: usize,
    pub element: UiElement,
}

/// `walk_tree` の結果 (D-084)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiTree {
    /// 読めた要素 (前順。根が深さ 0)
    pub nodes: Vec<TreeNode>,
    /// 深さの上限の要素で、子を持つのにたどらなかったものの数
    pub depth_cut: usize,
    /// 試した要素が上限に達した後に、まだたどる要素があった (そこで止めた)
    pub node_cap_hit: bool,
    /// 読めなかった要素と、子や兄弟を取れずにたどれなかった枝の数
    pub skipped: usize,
}

/// 要素と木の読みの失敗 (D-110)。`TimedOut` は読み元 (UIA) が時間切れを返した (対象のアプリが応答していない) こと、
/// `Failed` はそれ以外の失敗。時間切れは読みの途中でも残りを読まずに打ち切る (`skip_or_abort`)。
/// UIA の層では `UiaError` の同じ名前の腕に写す。`Display` は中の文をそのまま出す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    Failed(String),
    TimedOut(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Failed(m) | ReadError::TimedOut(m) => f.write_str(m),
        }
    }
}

/// 根より下の読みの失敗の振り分け (D-110)。時間切れでない失敗 (`Failed`) は `skipped` に数えて `Ok(())` (続ける)、
/// 時間切れ (`TimedOut`) は数えずに、文の頭に `context` (打ち切った所。空なら足さない) を足した `TimedOut` の Err
/// (打ち切る)。`walk_tree` と UIA の要素の読みの子の繰り返しの両方が呼ぶ
pub fn skip_or_abort(e: ReadError, skipped: &mut usize, context: &str) -> Result<(), ReadError> {
    match e {
        ReadError::Failed(_) => {
            *skipped += 1;
            Ok(())
        }
        ReadError::TimedOut(m) => Err(ReadError::TimedOut(format!("{context}{m}"))),
    }
}

/// 子や兄弟の取得の結果。`End` は「無い」(終わり)、`Failed` は取得の失敗、`TimedOut` は読み元の時間切れ
/// (中は失敗の文。`walk_tree` はそこで打ち切る。D-110)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkStep<N> {
    Next(N),
    End,
    Failed,
    TimedOut(String),
}

/// 木の読み方 (D-084)。実装置は UIA の ControlViewWalker、テストとモックは前順の配列
/// (`testing::VecTreeSource`)
pub trait TreeSource {
    type Node;
    /// 要素を読む。根の失敗は全体の失敗、根以外の `Failed` は `skipped` に数え、根以外でも `TimedOut` は
    /// 全体の失敗 (D-110)
    fn describe(&self, n: &Self::Node) -> Result<UiElement, ReadError>;
    fn first_child(&self, n: &Self::Node) -> WalkStep<Self::Node>;
    fn next_sibling(&self, n: &Self::Node) -> WalkStep<Self::Node>;
}

/// 取得を呼んだ元の要素の深さ `depth` に応じた、時間切れの文の頭 (D-110)。深さ 0 (根の子の取得) は空
fn depth_context(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!("深さ {depth} の要素の読み: ")
    }
}

/// `walk_tree` の手の残り (明示のスタック)。`Visit` はその要素を読む、`Sibling` はその要素の次の兄弟を取る
enum Walk<N> {
    Visit(N, usize),
    Sibling(N, usize),
}

/// 根から前順にたどる (D-084)。根 (深さ 0) は子だけをたどり、兄弟は取らない。深さ `max_depth` の要素の子は
/// たどらず、子を持てば `depth_cut` に数える。試した要素 (読めた数 + 読めなかった数) が `max_nodes` に達した後に、
/// まだたどる要素が見つかったら `node_cap_hit` で止める。根の `describe` の失敗は Err。根以外の `describe` の失敗は
/// `skipped` に数えて子をたどらずに兄弟へ、子の取得の `Failed` は `skipped` に数えて兄弟へ、兄弟の取得の `Failed` は
/// `skipped` に数えてその段を諦める。深さの上限で子の有無を見る取得の `Failed` は数えない。
/// 読み元の時間切れ (`describe` の `ReadError::TimedOut`、取得の `WalkStep::TimedOut`) は、深さを問わず (深さの上限で
/// 子の有無を見る取得も) その場で残りを読まずに `Err(ReadError::TimedOut)` を返す (D-110)。文の頭には、根より下の
/// `describe` ならその要素の深さ、取得なら呼んだ元の要素の深さを「深さ <d> の要素の読み: 」の形で足す (深さ 0 は足さない)。
/// 深い木でスタックがあふれないよう、再帰でなく明示のスタックで回す
pub fn walk_tree<S: TreeSource>(
    src: &S,
    root: S::Node,
    max_depth: usize,
    max_nodes: usize,
) -> Result<UiTree, ReadError> {
    let mut tree = UiTree::default();
    let mut tried = 0usize;
    let mut stack = vec![Walk::Visit(root, 0)];
    while let Some(w) = stack.pop() {
        match w {
            Walk::Visit(n, depth) => {
                if tried >= max_nodes {
                    tree.node_cap_hit = true;
                    break;
                }
                tried += 1;
                let element = match src.describe(&n) {
                    Ok(e) => e,
                    Err(e) if depth == 0 => return Err(e),
                    Err(e) => {
                        skip_or_abort(e, &mut tree.skipped, &depth_context(depth))?;
                        stack.push(Walk::Sibling(n, depth));
                        continue;
                    }
                };
                tree.nodes.push(TreeNode { depth, element });
                let child = if depth >= max_depth {
                    match src.first_child(&n) {
                        WalkStep::Next(_) => tree.depth_cut += 1,
                        // 時間切れは数えずに Err で返る (skip_or_abort は TimedOut を数えない)
                        WalkStep::TimedOut(m) => skip_or_abort(
                            ReadError::TimedOut(m),
                            &mut tree.skipped,
                            &depth_context(depth),
                        )?,
                        WalkStep::End | WalkStep::Failed => {}
                    }
                    None
                } else {
                    match src.first_child(&n) {
                        WalkStep::Next(c) => Some(c),
                        WalkStep::End => None,
                        WalkStep::Failed => {
                            tree.skipped += 1;
                            None
                        }
                        WalkStep::TimedOut(m) => {
                            skip_or_abort(
                                ReadError::TimedOut(m),
                                &mut tree.skipped,
                                &depth_context(depth),
                            )?;
                            None
                        }
                    }
                };
                // 兄弟は子孫の後 (前順)。根の兄弟は取らない
                if depth > 0 {
                    stack.push(Walk::Sibling(n, depth));
                }
                if let Some(c) = child {
                    stack.push(Walk::Visit(c, depth + 1));
                }
            }
            Walk::Sibling(n, depth) => match src.next_sibling(&n) {
                WalkStep::Next(s) => stack.push(Walk::Visit(s, depth)),
                WalkStep::End => {}
                WalkStep::Failed => tree.skipped += 1,
                WalkStep::TimedOut(m) => {
                    skip_or_abort(
                        ReadError::TimedOut(m),
                        &mut tree.skipped,
                        &depth_context(depth),
                    )?;
                }
            },
        }
    }
    Ok(tree)
}

/// `render_tree` の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTree {
    /// 1 行 1 要素 (`\n` 区切り、末尾に改行なし)
    pub text: String,
    pub lines: usize,
    /// 行の上限で捨てたか、`walk_tree` が要素の上限で止めた
    pub truncated: bool,
    /// 畳んだ要素の数 (行の上限で捨てた分は数えない)
    pub folded: usize,
}

/// 畳む要素か (根以外に掛ける): 名前の無い (空白だけを含む) pane / group / custom と、画面外の要素
fn tree_folds(e: &UiElement) -> bool {
    let nameless_container =
        matches!(e.control_type.as_str(), "pane" | "group" | "custom") && e.name.trim().is_empty();
    nameless_container || e.offscreen
}

/// 1 行 (字下げを除く): `種類[ "名前"][ @cx,cy][ disabled]`。名前は 80 char を超えたら 79 char + `…` に切ってから
/// JSON の文字列として引用する
fn tree_line(e: &UiElement) -> String {
    let mut line = e.control_type.clone();
    if !e.name.is_empty() {
        let name: String = if e.name.chars().count() > TREE_NAME_MAX_CHARS {
            e.name
                .chars()
                .take(TREE_NAME_MAX_CHARS - 1)
                .chain(std::iter::once('…'))
                .collect()
        } else {
            e.name.clone()
        };
        line.push(' ');
        line.push_str(&serde_json::to_string(&name).expect("文字列は JSON にできる"));
    }
    if let Some(p) = e.center() {
        line.push_str(&format!(" @{},{}", p.x, p.y));
    }
    if !e.enabled {
        line.push_str(" disabled");
    }
    line
}

/// 木を字下げの文字列にする (D-084)。根は必ず出す。`all` でなければ根以外の畳む要素 (`tree_folds`) は行を出さず、
/// 子孫の見かけの深さを 1 つ浅くする (見かけの深さは畳まなかった祖先の数)。`limit` 行を出した後に、出すはずの行が
/// まだあるときだけ `truncated` (そのとき `limit` 行目より後に畳んだ要素は数えない)。`tree.node_cap_hit` も `truncated`
pub fn render_tree(tree: &UiTree, limit: usize, all: bool) -> RenderedTree {
    let mut out: Vec<String> = Vec::new();
    let mut folded = 0usize;
    // limit 行を出した後に畳んだ数 (打ち切ったら捨てる)
    let mut folded_after_limit = 0usize;
    let mut truncated = false;
    // 祖先の (実の深さ, 畳んだか)
    let mut ancestors: Vec<(usize, bool)> = Vec::new();
    for node in &tree.nodes {
        while ancestors.last().is_some_and(|(d, _)| *d >= node.depth) {
            ancestors.pop();
        }
        let fold = !all && node.depth > 0 && tree_folds(&node.element);
        let visible_depth = ancestors.iter().filter(|(_, f)| !f).count();
        ancestors.push((node.depth, fold));
        if fold {
            if out.len() >= limit {
                folded_after_limit += 1;
            } else {
                folded += 1;
            }
            continue;
        }
        if out.len() >= limit {
            truncated = true;
            break;
        }
        out.push(format!(
            "{}{}",
            "  ".repeat(visible_depth),
            tree_line(&node.element)
        ));
    }
    if !truncated {
        folded += folded_after_limit;
    }
    RenderedTree {
        lines: out.len(),
        text: out.join("\n"),
        truncated: truncated || tree.node_cap_hit,
        folded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_type_names_round_trip_and_unknown_ids_are_labeled() {
        assert_eq!(control_type_name(50000), "button");
        assert_eq!(control_type_name(50030), "document");
        assert_eq!(control_type_name(50032), "window");
        assert_eq!(control_type_name(50099), "control_type_50099");
        assert_eq!(parse_control_type("Button"), Some("button"));
        assert_eq!(parse_control_type("list-item"), Some("list_item"));
        assert_eq!(parse_control_type("ListItem"), Some("list_item"));
        assert_eq!(parse_control_type("menu item"), Some("menu_item"));
        assert_eq!(parse_control_type("bogus"), None);
        assert_eq!(control_type_names().len(), 41);
    }

    /// 名前・種類・automation_id・フォーカスの要素 (矩形なし)
    fn qe(name: &str, ty: &str, aid: &str, focused: bool) -> UiElement {
        UiElement {
            automation_id: aid.into(),
            focused,
            ..el(name, ty, None, None)
        }
    }

    #[test]
    fn element_query_matches_element_by_name_type_automation_id_and_focus() {
        let q = ElementQuery {
            name: Some("エディ".into()),
            control_type: Some("document".into()),
            limit: 10,
            ..Default::default()
        };
        assert!(q.matches_element(&qe("テキスト エディター", "document", "", false)));
        assert!(!q.matches_element(&qe("テキスト エディター", "edit", "", false)));
        assert!(!q.matches_element(&qe("保存", "document", "", false)));
        let any = ElementQuery::default();
        assert!(any.matches_element(&qe("", "pane", "", false)));
        let ci = ElementQuery {
            name: Some("SAVE".into()),
            ..Default::default()
        };
        assert!(
            ci.matches_element(&qe("Save As", "button", "", false)),
            "名前は大文字小文字を無視"
        );

        // automation_id は完全一致 (部分一致しない)
        let aid = |a: &str| ElementQuery {
            automation_id: Some(a.into()),
            ..Default::default()
        };
        assert!(aid("101").matches_element(&qe("", "button", "101", false)));
        for other in ["1011", "10", "A101", ""] {
            assert!(
                !aid("101").matches_element(&qe("", "button", other, false)),
                "\"101\" は {other:?} に当たらない"
            );
        }
        assert!(!aid("10").matches_element(&qe("", "button", "101", false)));
        // automation_id は大文字小文字を区別する
        assert!(!aid("Btn").matches_element(&qe("", "button", "btn", false)));
        assert!(aid("Btn").matches_element(&qe("", "button", "Btn", false)));

        // focused で絞ると、フォーカスを持たない要素は外れる
        let focus = ElementQuery {
            focused: true,
            ..Default::default()
        };
        assert!(!focus.matches_element(&qe("a", "edit", "", false)));
        assert!(focus.matches_element(&qe("a", "edit", "", true)));
        // matches_except_focus はフォーカスを見ない (実装置は読む前の false の要素をこれに渡す)
        assert!(focus.matches_except_focus(&qe("a", "edit", "", false)));

        // 名前と focused の AND: 片方だけ満たす要素は外れる
        let both = ElementQuery {
            name: Some("本文".into()),
            focused: true,
            ..Default::default()
        };
        assert!(both.matches_element(&qe("本文", "edit", "", true)));
        assert!(!both.matches_element(&qe("本文", "edit", "", false)));
        assert!(!both.matches_element(&qe("保存", "button", "", true)));
    }

    /// 状態を読むのは文字列を読まない問い合わせだけ
    #[test]
    fn reads_states_only_for_non_text_queries() {
        assert!(ElementQuery::default().reads_states());
        assert!(!ElementQuery::for_text(None, None).reads_states());
    }

    /// パスワードの要素を読まずに飛ばすのは、文字列を読む問い合わせで password が真のときだけ (D-090)
    #[test]
    fn skips_text_of_password_only_for_text_queries() {
        let text = ElementQuery::for_text(None, None);
        assert!(text.skips_text_of_password(true));
        assert!(!text.skips_text_of_password(false));
        let find = ElementQuery::default();
        assert!(!find.skips_text_of_password(true));
        assert!(!find.skips_text_of_password(false));
    }

    /// 共有の JSON に password は常に出る (真の要素も偽の要素も)。text は落とす (D-090)
    #[test]
    fn element_json_shows_password_flag() {
        let e = UiElement {
            password: true,
            ..el("パスワード", "edit", None, Some("secret"))
        };
        let v = element_json(&e);
        assert_eq!(v["password"], true, "{v}");
        assert!(v.get("text").is_none(), "{v}");
        assert!(!v.to_string().contains("secret"), "{v}");
        let v = element_json(&el("閉じる", "button", None, None));
        assert_eq!(v["password"], false, "{v}");
    }

    /// UIA の列挙の値と名前の対応 (Microsoft Learn の ToggleState / ExpandCollapseState)。知らない値は None
    #[test]
    fn state_names_map_uia_enums_and_unknown_is_none() {
        assert_eq!(toggle_state_name(0), Some("off"));
        assert_eq!(toggle_state_name(1), Some("on"));
        assert_eq!(toggle_state_name(2), Some("indeterminate"));
        assert_eq!(toggle_state_name(3), None);
        assert_eq!(toggle_state_name(-1), None);
        assert_eq!(expand_collapse_state_name(0), Some("collapsed"));
        assert_eq!(expand_collapse_state_name(1), Some("expanded"));
        assert_eq!(expand_collapse_state_name(2), Some("partially_expanded"));
        assert_eq!(expand_collapse_state_name(3), Some("leaf_node"));
        assert_eq!(expand_collapse_state_name(4), None);
    }

    /// 共有の JSON に focused は常に、状態は読めたものだけが出る。text は落とす
    #[test]
    fn element_json_shows_focus_and_present_states_only() {
        let e = UiElement {
            focused: true,
            toggle: Some("on".into()),
            selected: Some(false),
            expanded: None,
            ..el("チェック", "check_box", None, Some("秘密"))
        };
        let v = element_json(&e);
        assert_eq!(v["focused"], true, "{v}");
        assert_eq!(v["toggle"], "on", "{v}");
        assert_eq!(v["selected"], false, "{v}");
        assert!(v.get("expanded").is_none(), "{v}");
        assert!(v.get("text").is_none(), "{v}");
        // 状態の無い要素でも focused (false) は出る
        let v = element_json(&el("閉じる", "button", None, None));
        assert_eq!(v["focused"], false, "{v}");
        for k in ["toggle", "selected", "expanded"] {
            assert!(v.get(k).is_none(), "{k}: {v}");
        }
    }

    #[test]
    fn element_center_is_rect_center() {
        let e = UiElement {
            name: "a".into(),
            control_type: "button".into(),
            automation_id: String::new(),
            class: String::new(),
            rect: Some(Rect::new(10, 20, 40, 20)),
            enabled: true,
            offscreen: false,
            focused: false,
            password: false,
            toggle: None,
            selected: None,
            expanded: None,
            text: None,
        };
        assert_eq!(e.center(), Some(Point::new(30, 30)));
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["type"], "button");
        assert!(json.get("text").is_none(), "無い文字列は出さない");
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

    /// 共有の JSON (D-068): center を足し (rect が無ければ null)、text があっても JSON には出さない
    #[test]
    fn element_json_adds_center_and_drops_text() {
        let v = element_json(&el(
            "本文",
            "document",
            Some(Rect::new(10, 20, 40, 20)),
            Some("秘密の本文"),
        ));
        assert_eq!(v["center"], serde_json::json!({ "x": 30, "y": 30 }));
        assert_eq!(v["type"], "document");
        assert_eq!(v["name"], "本文");
        assert!(v.get("text").is_none(), "{v}");
        assert!(!v.to_string().contains("秘密"), "{v}");
        let v = element_json(&el("閉じる", "button", None, None));
        assert_eq!(v["center"], serde_json::Value::Null);
        assert!(v.get("text").is_none());
    }

    /// `char` 単位で数え、ちょうど上限は切らず、上限 + 1 で切って元の数を返す。サロゲートペアと全角も 1 字 (D-102)
    #[test]
    fn truncate_chars_counts_chars_and_cuts_at_limit() {
        assert_eq!(truncate_chars("abc", 3), ("abc".to_string(), false, 3));
        assert_eq!(truncate_chars("abcd", 3), ("abc".to_string(), true, 4));
        assert_eq!(truncate_chars("", 5), (String::new(), false, 0));
        assert_eq!(truncate_chars("😀あ😀", 2), ("😀あ".to_string(), true, 3));
    }

    /// 要素の JSON の name は 500 字で切り、切った要素にだけ name_truncated。「…」は付けない (D-102)
    #[test]
    fn element_json_cuts_long_name_without_ellipsis() {
        // ちょうど 500 字は切らない
        let exact: String = "あ".repeat(ELEMENT_NAME_MAX_CHARS);
        let v = element_json(&el(&exact, "text", None, None));
        assert_eq!(v["name"], exact.as_str());
        assert!(v.get("name_truncated").is_none(), "{v}");

        // 位置ごとに違う字の 501 字は先頭 500 字にする
        let long: String = (0..=ELEMENT_NAME_MAX_CHARS)
            .map(|i| char::from_u32(0x3042 + (i % 80) as u32).unwrap())
            .collect();
        assert_eq!(long.chars().count(), ELEMENT_NAME_MAX_CHARS + 1);
        let head: String = long.chars().take(ELEMENT_NAME_MAX_CHARS).collect();
        let v = element_json(&el(&long, "text", None, None));
        let name = v["name"].as_str().unwrap();
        assert_eq!(name, head);
        assert_eq!(v["name_truncated"], true, "{v}");
        assert!(!name.contains('…'), "{name}");

        // 短い名前には name_truncated が出ない
        let v = element_json(&el("閉じる", "button", None, None));
        assert_eq!(v["name"], "閉じる");
        assert!(v.get("name_truncated").is_none(), "{v}");
    }

    /// 最初に文字列を持つ要素 (文字列の無い要素は飛ばす)。CR / CRLF / LF の混在を LF にそろえる。無ければ None
    #[test]
    fn first_text_skips_textless_elements_and_normalizes_newlines() {
        let list = vec![
            el("保存", "button", None, None),
            el("本文", "document", None, Some("a\r\nb\rc\nd")),
            el("後ろ", "edit", None, Some("zzz")),
        ];
        let (e, t) = first_text(&list).unwrap();
        assert_eq!(e.name, "本文");
        assert_eq!(t, "a\nb\nc\nd");
        assert!(first_text(&[el("保存", "button", None, None)]).is_none());
        assert!(first_text(&[]).is_none());
        // 条件の側にも掛ける共有の関数。LF だけの文字列は変わらない
        assert_eq!(normalize_newlines("x\r\ny\rz\nw"), "x\ny\nz\nw");
        assert_eq!(normalize_newlines("x\ny"), "x\ny");
    }

    /// 文字列を読む問い合わせは with_text で、名前と種類を写す
    #[test]
    fn for_text_query_reads_text_with_filters() {
        let q = ElementQuery::for_text(Some("本文".into()), Some("document".into()));
        assert!(q.with_text);
        assert_eq!(q.name.as_deref(), Some("本文"));
        assert_eq!(q.control_type.as_deref(), Some("document"));
        assert!(q.limit > 0, "形式上の値だが 0 にはしない");
        let any = ElementQuery::for_text(None, None);
        assert!(any.with_text && any.name.is_none() && any.control_type.is_none());
    }

    use crate::testing::VecTreeSource;

    /// 木の要素 (矩形なし、有効、画面内)
    fn te(ty: &str, name: &str) -> UiElement {
        UiElement {
            name: name.into(),
            control_type: ty.into(),
            automation_id: String::new(),
            class: String::new(),
            rect: None,
            enabled: true,
            offscreen: false,
            focused: false,
            password: false,
            toggle: None,
            selected: None,
            expanded: None,
            text: None,
        }
    }

    /// 深さの並びだけの木 (名前は添字)
    fn src_of_depths(depths: &[usize]) -> VecTreeSource {
        VecTreeSource::new(
            depths
                .iter()
                .enumerate()
                .map(|(i, d)| (*d, te("button", &i.to_string())))
                .collect(),
        )
    }

    fn depths(t: &UiTree) -> Vec<usize> {
        t.nodes.iter().map(|n| n.depth).collect()
    }

    fn names(t: &UiTree) -> Vec<String> {
        t.nodes.iter().map(|n| n.element.name.clone()).collect()
    }

    fn tree_of(items: Vec<(usize, UiElement)>) -> UiTree {
        UiTree {
            nodes: items
                .into_iter()
                .map(|(depth, element)| TreeNode { depth, element })
                .collect(),
            ..Default::default()
        }
    }

    /// 前順で、深さの上限の要素の子はたどらずに depth_cut に数える (D-084)
    #[test]
    fn walk_tree_preorder_cuts_depth_and_counts_children() {
        let src = src_of_depths(&[0, 1, 2, 2, 1, 2]);
        let t = walk_tree(&src, 0, 1, TREE_MAX_NODES).unwrap();
        assert_eq!(depths(&t), vec![0, 1, 1]);
        assert_eq!(names(&t), vec!["0", "1", "4"]);
        assert_eq!(t.depth_cut, 2);
        assert!(!t.node_cap_hit);
        assert_eq!(t.skipped, 0);

        let t = walk_tree(&src, 0, 5, TREE_MAX_NODES).unwrap();
        assert_eq!(depths(&t), vec![0, 1, 2, 2, 1, 2], "入力の前順のまま");
        assert_eq!(names(&t), vec!["0", "1", "2", "3", "4", "5"]);
        assert_eq!(t.depth_cut, 0);
    }

    /// 根の兄弟 (別の窓) は取らない
    #[test]
    fn walk_tree_never_visits_root_siblings() {
        let src = src_of_depths(&[0, 1, 0, 1]);
        let t = walk_tree(&src, 0, 5, TREE_MAX_NODES).unwrap();
        assert_eq!(depths(&t), vec![0, 1]);
        assert_eq!(names(&t), vec!["0", "1"]);
        assert_eq!(t.nodes.iter().filter(|n| n.depth == 0).count(), 1);
    }

    /// 試した要素が上限に達した後に、まだたどる要素があるときだけ node_cap_hit。深さの上限で切った子は数えない
    #[test]
    fn walk_tree_node_cap_boundary() {
        let src = src_of_depths(&[0, 1, 1]);
        let t = walk_tree(&src, 0, 5, 2).unwrap();
        assert_eq!(names(&t), vec!["0", "1"]);
        assert!(t.node_cap_hit);
        let t = walk_tree(&src, 0, 5, 3).unwrap();
        assert_eq!(names(&t), vec!["0", "1", "2"]);
        assert!(!t.node_cap_hit, "ちょうど上限の数の木は打ち切りにしない");

        let src = src_of_depths(&[0, 1, 1, 2]);
        let t = walk_tree(&src, 0, 1, 3).unwrap();
        assert_eq!(names(&t), vec!["0", "1", "2"]);
        assert_eq!(t.depth_cut, 1);
        assert!(!t.node_cap_hit, "深さで切った子はまだたどる要素ではない");
    }

    /// 読めない要素、子や兄弟の取得の失敗は skipped に数えて続ける。根の失敗は Err
    #[test]
    fn walk_tree_skips_failures() {
        // 根以外の describe の失敗: その要素と子が出ず、兄弟は出る
        let mut src = src_of_depths(&[0, 1, 2, 1]);
        src.unreadable = vec![1];
        let t = walk_tree(&src, 0, 5, TREE_MAX_NODES).unwrap();
        assert_eq!(names(&t), vec!["0", "3"]);
        assert_eq!(t.skipped, 1);

        // 子の取得の失敗: 兄弟は出る
        let mut src = src_of_depths(&[0, 1, 2, 1]);
        src.child_fail = vec![1];
        let t = walk_tree(&src, 0, 5, TREE_MAX_NODES).unwrap();
        assert_eq!(names(&t), vec!["0", "1", "3"]);
        assert_eq!(t.skipped, 1);

        // 兄弟の取得の失敗: その段の残りは出ない
        let mut src = src_of_depths(&[0, 1, 1, 1]);
        src.sibling_fail = vec![1];
        let t = walk_tree(&src, 0, 5, TREE_MAX_NODES).unwrap();
        assert_eq!(names(&t), vec!["0", "1"]);
        assert_eq!(t.skipped, 1);

        // 深さの上限で子の有無を見る取得の失敗は数えない
        let mut src = src_of_depths(&[0, 1, 2]);
        src.child_fail = vec![1];
        let t = walk_tree(&src, 0, 1, TREE_MAX_NODES).unwrap();
        assert_eq!(names(&t), vec!["0", "1"]);
        assert_eq!((t.skipped, t.depth_cut), (0, 0));

        // 根の describe の失敗は全体の失敗
        let mut src = src_of_depths(&[0, 1]);
        src.unreadable = vec![0];
        assert!(walk_tree(&src, 0, 5, TREE_MAX_NODES).is_err());

        // 読めない兄弟が上限を超えて続いても、試した数が上限を超えずに止まる
        let mut d = vec![0];
        d.extend([1; 10]);
        let mut src = src_of_depths(&d);
        src.unreadable = (1..=10).collect();
        let t = walk_tree(&src, 0, 5, 5).unwrap();
        assert!(t.node_cap_hit);
        assert_eq!(src.describes.get(), 5, "試した数");
        assert_eq!(names(&t), vec!["0"]);
        assert_eq!(t.skipped, 4);
    }

    /// 時間切れの文が `TimedOut` で、頭が `prefix` で始まるか (`None` なら「深さ」で始まらない)
    fn assert_timed_out(r: Result<UiTree, ReadError>, prefix: Option<&str>) {
        match r {
            Err(ReadError::TimedOut(m)) => match prefix {
                Some(p) => assert!(m.starts_with(p), "文の頭が {p:?} でない: {m}"),
                None => assert!(!m.starts_with("深さ"), "根の文に深さの頭がある: {m}"),
            },
            other => panic!("TimedOut でない: {other:?}"),
        }
    }

    /// 時間切れでない失敗は数えて続け、時間切れは数えずに文の頭を足して打ち切る (D-110)
    #[test]
    fn skip_or_abort_counts_failure_and_stops_on_timeout() {
        let mut skipped = 0;
        assert_eq!(
            skip_or_abort(ReadError::Failed("f".into()), &mut skipped, "頭: "),
            Ok(())
        );
        assert_eq!(skipped, 1);
        assert_eq!(
            skip_or_abort(
                ReadError::TimedOut("x".into()),
                &mut skipped,
                "深さ 2 の要素の読み: "
            ),
            Err(ReadError::TimedOut("深さ 2 の要素の読み: x".into()))
        );
        assert_eq!(skipped, 1, "時間切れは数えない");
        assert_eq!(
            skip_or_abort(ReadError::TimedOut("x".into()), &mut skipped, ""),
            Err(ReadError::TimedOut("x".into()))
        );
        assert_eq!(skipped, 1);
        assert_eq!(ReadError::TimedOut("文".into()).to_string(), "文");
        assert_eq!(ReadError::Failed("文".into()).to_string(), "文");
    }

    /// 根より下の describe の時間切れで打ち切り、後の要素を読まない (D-110)
    #[test]
    fn walk_tree_aborts_on_describe_timeout_below_root() {
        let mut src = src_of_depths(&[0, 1, 1, 2]);
        src.timed_out = vec![1];
        assert_timed_out(
            walk_tree(&src, 0, 5, TREE_MAX_NODES),
            Some("深さ 1 の要素の読み: "),
        );
        assert_eq!(src.describes.get(), 2, "時間切れの後に読まない");
    }

    /// 根の describe の時間切れは文のまま TimedOut (D-110)
    #[test]
    fn walk_tree_aborts_on_root_describe_timeout() {
        let mut src = src_of_depths(&[0, 1]);
        src.timed_out = vec![0];
        assert_timed_out(walk_tree(&src, 0, 5, TREE_MAX_NODES), None);
        assert_eq!(src.describes.get(), 1);
    }

    /// 子と兄弟の取得の時間切れで打ち切る。文の頭は取得を呼んだ元の要素の深さ (深さ 0 は頭なし。D-110)
    #[test]
    fn walk_tree_aborts_on_child_and_sibling_timeout() {
        // 根の子の取得
        let mut src = src_of_depths(&[0, 1, 1]);
        src.child_timeout = vec![0];
        assert_timed_out(walk_tree(&src, 0, 5, TREE_MAX_NODES), None);
        assert_eq!(src.describes.get(), 1);

        // 深さ 1 の兄弟の取得
        let mut src = src_of_depths(&[0, 1, 1]);
        src.sibling_timeout = vec![1];
        assert_timed_out(
            walk_tree(&src, 0, 5, TREE_MAX_NODES),
            Some("深さ 1 の要素の読み: "),
        );
        assert_eq!(src.describes.get(), 2, "添字 2 を読まない");

        // 深さ 1 の子の取得 (頭は元の要素の深さ)
        let mut src = src_of_depths(&[0, 1, 2]);
        src.child_timeout = vec![1];
        assert_timed_out(
            walk_tree(&src, 0, 5, TREE_MAX_NODES),
            Some("深さ 1 の要素の読み: "),
        );
        assert_eq!(src.describes.get(), 2);
    }

    /// 深さの上限で子の有無を見る取得: 失敗は今どおり数えずに Ok、時間切れは打ち切る (D-110)
    #[test]
    fn walk_tree_aborts_on_depth_limit_child_probe_timeout() {
        let mut src = src_of_depths(&[0, 1, 2]);
        src.child_fail = vec![1];
        let t = walk_tree(&src, 0, 1, TREE_MAX_NODES).unwrap();
        assert_eq!(names(&t), vec!["0", "1"]);
        assert_eq!((t.skipped, t.depth_cut), (0, 0));

        let mut src = src_of_depths(&[0, 1, 2]);
        src.child_timeout = vec![1];
        assert_timed_out(
            walk_tree(&src, 0, 1, TREE_MAX_NODES),
            Some("深さ 1 の要素の読み: "),
        );
    }

    /// 字下げは見かけの深さ。@ は矩形の中心で、矩形が無ければ省く
    #[test]
    fn render_tree_indents_by_visible_depth_and_shows_center() {
        let mut root = te("window", "メモ帳");
        root.rect = Some(Rect::new(0, 0, 100, 100));
        let mut item = te("menu_item", "ファイル");
        item.rect = Some(Rect::new(10, 10, 20, 10));
        let t = tree_of(vec![(0, root), (1, te("menu_bar", "アプリ")), (2, item)]);
        let r = render_tree(&t, TREE_LIMIT_DEFAULT, false);
        assert_eq!(
            r.text,
            "window \"メモ帳\" @50,50\n  menu_bar \"アプリ\"\n    menu_item \"ファイル\" @20,15"
        );
        assert_eq!((r.lines, r.folded, r.truncated), (3, 0, false));
    }

    /// 名前の無い (空白だけを含む) 入れ物と画面外の要素は畳み、子を 1 段浅く出す。根は必ず出す。all で全部
    #[test]
    fn render_tree_folds_nameless_containers_and_offscreen_but_keeps_root() {
        let mut hidden = te("text", "隠");
        hidden.offscreen = true;
        let t = tree_of(vec![
            (0, te("pane", "")),
            (1, te("pane", "")),
            (2, te("group", "  ")),
            (3, te("custom", "")),
            (4, te("button", "OK")),
            (1, te("pane", "サイド")),
            (2, hidden),
            (3, te("button", "中")),
        ]);
        let r = render_tree(&t, TREE_LIMIT_DEFAULT, false);
        assert_eq!(
            r.text,
            "pane\n  button \"OK\"\n  pane \"サイド\"\n    button \"中\""
        );
        assert_eq!((r.lines, r.folded, r.truncated), (4, 4, false));

        let r = render_tree(&t, TREE_LIMIT_DEFAULT, true);
        assert_eq!(
            r.text,
            "pane\n  pane\n    group \"  \"\n      custom\n        button \"OK\"\n  pane \"サイド\"\n    text \"隠\"\n      button \"中\""
        );
        assert_eq!((r.lines, r.folded), (8, 0));
    }

    /// 名前は生のまま 80 char で切ってから JSON の文字列として引用する。空の名前は省き、無効は disabled
    #[test]
    fn render_tree_quotes_names_and_truncates_long_ones() {
        let one = |e: UiElement| render_tree(&tree_of(vec![(0, e)]), 10, false).text;

        let line = one(te("button", "a\"b\nc\td"));
        assert_eq!(line, "button \"a\\\"b\\nc\\td\"");
        assert!(!line.contains('\n'), "行の中に生の改行が無い");

        let long = "あ".repeat(100);
        assert_eq!(
            one(te("text", &long)),
            format!("text \"{}…\"", "あ".repeat(79))
        );
        let eighty = "x".repeat(80);
        assert_eq!(one(te("text", &eighty)), format!("text \"{eighty}\""));
        assert_eq!(
            one(te("text", &"x".repeat(81))),
            format!("text \"{}…\"", "x".repeat(79))
        );
        assert_eq!(
            one(te("text", &"\u{1}".repeat(100))),
            format!("text \"{}…\"", "\\u0001".repeat(79)),
            "逃がした後に切らない"
        );

        assert_eq!(one(te("button", "")), "button");
        let mut off = te("button", "保存");
        off.enabled = false;
        off.rect = Some(Rect::new(0, 0, 10, 10));
        assert_eq!(one(off), "button \"保存\" @5,5 disabled");
    }

    /// limit 行の後に出す行があるときだけ truncated。捨てた側の畳んだ要素は数えない。node_cap_hit も truncated
    #[test]
    fn render_tree_limit_boundary_and_node_cap() {
        let five = tree_of(vec![
            (0, te("window", "r")),
            (1, te("button", "a")),
            (1, te("button", "b")),
            (1, te("pane", "")),
            (1, te("button", "c")),
            (1, te("button", "d")),
        ]);
        let r = render_tree(&five, 3, false);
        assert_eq!((r.lines, r.truncated, r.folded), (3, true, 0));
        assert_eq!(r.text, "window \"r\"\n  button \"a\"\n  button \"b\"");
        let r = render_tree(&five, 5, false);
        assert_eq!((r.lines, r.truncated, r.folded), (5, false, 1));

        let three = tree_of(vec![
            (0, te("window", "r")),
            (1, te("button", "a")),
            (1, te("button", "b")),
        ]);
        let r = render_tree(&three, 3, false);
        assert_eq!((r.lines, r.truncated), (3, false), "ちょうど limit 行");

        let tail_folded = tree_of(vec![
            (0, te("window", "r")),
            (1, te("button", "a")),
            (1, te("button", "b")),
            (1, te("pane", "")),
        ]);
        let r = render_tree(&tail_folded, 3, false);
        assert_eq!((r.lines, r.truncated, r.folded), (3, false, 1));

        let mut capped = three.clone();
        capped.node_cap_hit = true;
        let r = render_tree(&capped, 10, false);
        assert_eq!((r.lines, r.truncated), (3, true));
    }
}
