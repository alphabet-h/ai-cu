//! allow / deny の判定 (D-028、D-029)。操作は、すべての層が許可したときだけ許可する。
//! 層の中の優先順位は、組み込みの拒否 > deny > allow。allow が空の次元は制限しない。

use std::collections::BTreeSet;
use std::path::Path;

use crate::config::{Layer, LayerRole, normalize_class, normalize_process};
use crate::device::WindowInfo;
use crate::error::{AppError, ErrorKind};
use crate::geometry::{Point, Rect};
use crate::image::path_within;
use crate::keys::{KeyCombo, builtin_denied_modifiers};
use crate::script::{ActionKind, ClickModifier};

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub layers: Vec<Layer>,
}

/// 拒否の `rule` に入る設定のキー (D-079)。どれも TUI で項目を選んだとき、右の枠の先頭に灰色で出るキー
/// (`tui::Field::key`) と同じ字句。左の一覧での場所は `tui_label` (`tui::Field::location` と同じ。D-080)
pub const CONFIG_RULES: [&str; 18] = [
    "general.input_enabled",
    "general.capture_enabled",
    "general.password_input",
    "allow.actions",
    "deny.actions",
    "allow.keys",
    "deny.keys",
    "allow.regions",
    "deny.regions",
    "allow.processes",
    "deny.processes",
    "allow.classes",
    "deny.classes",
    "limits.max_actions",
    "limits.max_duration_ms",
    "limits.max_text_len",
    "capture.full_screen",
    "capture.save_dirs",
];

/// 設定のキー `rule` の、TUI の左の一覧での場所「<節> > <項目>」(D-080)。`CONFIG_RULES` の各キーに返し、
/// それ以外 (`builtin`、`save_path`、未知) は None。`tui::Field::location` と一字一句同じことを tui のテストが見張る
pub fn tui_label(rule: &str) -> Option<&'static str> {
    Some(match rule {
        "general.input_enabled" => "全般 (総スイッチ) > 実入力 (マウスとキー)",
        "general.capture_enabled" => "全般 (総スイッチ) > 画面撮影",
        "general.password_input" => "全般 (総スイッチ) > パスワード欄への入力",
        "allow.actions" => "許可リスト (allow) > 操作",
        "deny.actions" => "拒否リスト (deny) > 操作",
        "allow.keys" => "許可リスト (allow) > キー",
        "deny.keys" => "拒否リスト (deny) > キー",
        "allow.regions" => "許可リスト (allow) > 画面領域",
        "deny.regions" => "拒否リスト (deny) > 画面領域",
        "allow.processes" => "許可リスト (allow) > アプリ (プロセス)",
        "deny.processes" => "拒否リスト (deny) > アプリ (プロセス)",
        "allow.classes" => "許可リスト (allow) > ウィンドウクラス",
        "deny.classes" => "拒否リスト (deny) > ウィンドウクラス",
        "limits.max_actions" => "上限 > 最大の操作数",
        "limits.max_duration_ms" => "上限 > 最大の実行時間 (ms)",
        "limits.max_text_len" => "上限 > 最大の文字数 (type)",
        "capture.full_screen" => "撮影の制限 > 全画面の撮影",
        "capture.save_dirs" => "撮影の制限 > 撮影の保存先のフォルダ",
        _ => return None,
    })
}

/// 組み込みの拒否 (キー。設定では許可できない) の `rule` (D-079)
pub const RULE_BUILTIN: &str = "builtin";

/// 設定で緩められない保存先の規則 (形と既存ファイル。D-075) の `rule` (D-079)
pub const RULE_SAVE_PATH: &str = "save_path";

/// 床が無い (床の場所にファイルが無い) ときの直し方 (D-079)。どの層のどの次元で断っても、この文にする
pub const NO_FLOOR_FIX: &str = "設定ファイルがまだ無いので何も許可されていない。端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動し、TUI で設定を作る (手順は ai-cu guide --setup)";

/// 拒否の理由 (どの層の何に当たったか)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    pub message: String,
    /// 当たった設定のキー (`CONFIG_RULES` のどれか)、`RULE_BUILTIN`、`RULE_SAVE_PATH` (D-079)
    pub rule: Option<&'static str>,
    /// 人間向けの直し方の 1 文 (D-079)。設定で直せないもの、床の場所を引けないときは None
    pub fix: Option<String>,
}

/// 組み込みの拒否に当たったときの次の手
pub const BUILTIN_HINT: &str = "組み込みの拒否 (Win+、Alt+F4、Ctrl+Alt+ 系など) は設定でも許可できない。人間に頼まず、別の手段 (メニューやボタン) を使う";

impl Denied {
    /// 組み込みの拒否 (設定でも許可できない) か。判定の正は `rule` の 1 つ (D-079)
    pub fn is_builtin(&self) -> bool {
        self.rule == Some(RULE_BUILTIN)
    }

    /// 失敗の JSON にする (D-079)。message、rule、fix を写す。Denied から AppError を作る経路はこれだけにする
    /// (message の頭に文を足すなら、作った後で `message` を直す)
    pub fn into_error(self, kind: ErrorKind) -> AppError {
        AppError::new(kind, self.message).with_deny(self.rule, self.fix)
    }
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

type Check = Result<(), Denied>;

impl Policy {
    pub fn new(layers: Vec<Layer>) -> Self {
        assert!(!layers.is_empty(), "層は基本の層を含めて 1 つ以上");
        Policy { layers }
    }

    /// 何も許可しない方針 (床の場所を引けないときと同じ)。
    pub fn closed() -> Self {
        Policy::new(vec![Layer::closed("(既定) 何も許可しない")])
    }

    /// 断った層 `layer` と設定のキー `rule` と変え方 `change` (`を true にする` など) から拒否を作る (D-079)。
    /// 床が無ければ (`NoFloor`) どの層で断っても床なしの文、床の場所を引けなければ (`Unresolved`) 直し方は付けない
    fn denied(&self, layer: &Layer, rule: &'static str, change: String, message: String) -> Denied {
        let role = match self.layers[0].role {
            LayerRole::NoFloor | LayerRole::Unresolved => self.layers[0].role,
            _ => layer.role,
        };
        let fix = match role {
            LayerRole::NoFloor => Some(NO_FLOOR_FIX.to_string()),
            LayerRole::Unresolved => None,
            LayerRole::Floor => Some(match tui_label(rule) {
                Some(label) => format!(
                    "許可してよければ、端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動した TUI の「{label}」か、設定ファイル {} の {rule} {change}",
                    layer.source
                ),
                // 起こらない (rule は CONFIG_RULES のどれか) が、文を壊さない側に倒す (D-080)
                None => format!(
                    "許可してよければ、端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動した TUI か、設定ファイル {} の {rule} {change}",
                    layer.source
                ),
            }),
            LayerRole::Overlay => Some(format!(
                "許可してよければ、追加の層の設定ファイル {} (--config か AI_CU_CONFIG) の {rule} {change}",
                layer.source
            )),
        };
        Denied {
            message,
            rule: Some(rule),
            fix,
        }
    }

    /// 上限 (全層の最小値で効く) を断る層: 床から順に見て、値が `need` に足りない最初の層 (D-079)
    fn first_short_layer(&self, need: u64, value: impl Fn(&Layer) -> u64) -> &Layer {
        self.layers
            .iter()
            .find(|l| value(l) < need)
            .unwrap_or(&self.layers[0])
    }

    pub fn check_action_kind(&self, kind: ActionKind) -> Check {
        // wait は何も動かさないので、総スイッチと actions の次元の対象外
        if kind == ActionKind::Wait {
            return Ok(());
        }
        for l in &self.layers {
            // wait_for は読むだけ (要素の名前と文字列は画面上の情報) なので、撮影の総スイッチで見る (D-068)。
            // wait_stable と wait_change は撮影を繰り返すだけなので同じ (D-081)
            if matches!(
                kind,
                ActionKind::Screenshot
                    | ActionKind::WaitFor
                    | ActionKind::WaitStable
                    | ActionKind::WaitChange
            ) {
                if !l.general.capture_enabled {
                    return Err(self.denied(
                        l,
                        "general.capture_enabled",
                        "を true にする".into(),
                        format!(
                            "撮影が許可されていない (general.capture_enabled = false): {}",
                            l.source
                        ),
                    ));
                }
            } else if !l.general.input_enabled {
                return Err(self.denied(
                    l,
                    "general.input_enabled",
                    "を true にする".into(),
                    format!(
                        "実入力が許可されていない (general.input_enabled = false): {}",
                        l.source
                    ),
                ));
            }
            if l.deny.actions.contains(&kind) {
                return Err(self.denied(
                    l,
                    "deny.actions",
                    format!("から {} を外す", kind.name()),
                    format!(
                        "操作 {} は deny.actions に当たる: {}",
                        kind.name(),
                        l.source
                    ),
                ));
            }
            if !l.allow.actions.is_empty() && !l.allow.actions.contains(&kind) {
                return Err(self.denied(
                    l,
                    "allow.actions",
                    format!("に {} を足す", kind.name()),
                    format!("操作 {} は allow.actions に無い: {}", kind.name(), l.source),
                ));
            }
        }
        Ok(())
    }

    pub fn check_key(&self, combo: &KeyCombo) -> Check {
        if let Some(why) = combo.builtin_denied() {
            return Err(Denied {
                message: format!("{combo}: {why}"),
                rule: Some(RULE_BUILTIN),
                fix: None,
            });
        }
        for l in &self.layers {
            if l.deny.keys.contains(combo) {
                return Err(self.denied(
                    l,
                    "deny.keys",
                    format!("から {combo} を外す"),
                    format!("キー {combo} は deny.keys に当たる: {}", l.source),
                ));
            }
            if !l.allow.keys.is_empty() && !l.allow.keys.contains(combo) {
                return Err(self.denied(
                    l,
                    "allow.keys",
                    format!("に {combo} を足す"),
                    format!("キー {combo} は allow.keys に無い: {}", l.source),
                ));
            }
        }
        Ok(())
    }

    /// `click` / `double_click` の `modifiers` (修飾キーを押したままのクリック) の判定 (D-137)。空なら通す。
    /// 順に (a) 修飾キーだけで決まる組み込みの拒否 (Ctrl と Alt の両方。設定で緩められない)、
    /// (b) 操作の種類 `key` の許可 (`deny.actions = ["key"]` をくぐらせない)、
    /// (c) どれかの層の `allow.keys` が空でなければ断る (修飾キーだけは allow.keys に書けないので、許可を広げない側)。
    /// `deny.keys` は見ない (修飾キーだけの組み合わせは書けない。止めるなら deny.actions の key)
    pub fn check_click_modifiers(&self, mods: &[ClickModifier]) -> Check {
        if mods.is_empty() {
            return Ok(());
        }
        let canonical = ClickModifier::canonical(mods);
        let names = canonical
            .iter()
            .map(|m| m.modifier().name())
            .collect::<Vec<_>>()
            .join("+");
        let key_mods: Vec<_> = canonical.iter().map(|m| m.modifier()).collect();
        if let Some(why) = builtin_denied_modifiers(&key_mods) {
            return Err(Denied {
                message: format!("修飾キー {names} のクリック: {why}"),
                rule: Some(RULE_BUILTIN),
                fix: None,
            });
        }
        if let Err(mut d) = self.check_action_kind(ActionKind::Key) {
            d.message = format!(
                "修飾キーつきのクリックは操作の種類 key の許可にも通す: {}",
                d.message
            );
            return Err(d);
        }
        if let Some(l) = self.layers.iter().find(|l| !l.allow.keys.is_empty()) {
            return Err(self.denied(
                l,
                "allow.keys",
                "を空にする".into(),
                format!(
                    "修飾キーつきのクリックは allow.keys に書けないので、allow.keys を使う設定では断る (modifiers なしなら打てる): {}",
                    l.source
                ),
            ));
        }
        Ok(())
    }

    pub fn check_point(&self, p: Point) -> Check {
        for l in &self.layers {
            if let Some(r) = l.deny.regions.iter().find(|r| r.contains(p)) {
                return Err(self.denied(
                    l,
                    "deny.regions",
                    format!("から {r} を外す"),
                    format!(
                        "座標 ({}, {}) は deny.regions の {r} に当たる: {}",
                        p.x, p.y, l.source
                    ),
                ));
            }
            if !l.allow.regions.is_empty() && !l.allow.regions.iter().any(|r| r.contains(p)) {
                return Err(self.denied(
                    l,
                    "allow.regions",
                    format!("に 座標 ({}, {}) を含む矩形 を足す", p.x, p.y),
                    format!(
                        "座標 ({}, {}) は allow.regions のどれにも入らない: {}",
                        p.x, p.y, l.source
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 撮影の矩形: deny の領域と少しでも重なれば拒否、allow の領域があればどれかに丸ごと含まれること。
    pub fn check_capture_rect(&self, rect: &Rect) -> Check {
        for l in &self.layers {
            if let Some(r) = l.deny.regions.iter().find(|r| r.intersects(rect)) {
                return Err(self.denied(
                    l,
                    "deny.regions",
                    format!("から {r} を外す"),
                    format!(
                        "撮影の矩形 {rect} は deny.regions の {r} と重なる: {}",
                        l.source
                    ),
                ));
            }
            if !l.allow.regions.is_empty() && !l.allow.regions.iter().any(|r| r.contains_rect(rect))
            {
                return Err(self.denied(
                    l,
                    "allow.regions",
                    format!("に {rect} を含む矩形 を足す"),
                    format!(
                        "撮影の矩形 {rect} は allow.regions のどれにも収まらない: {}",
                        l.source
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn check_window(&self, w: &WindowInfo) -> Check {
        let process = normalize_process(&w.process);
        let class = normalize_class(&w.class);
        for l in &self.layers {
            if let Some(d) = self.window_deny(l, w, &process, &class) {
                return Err(d);
            }
            if !l.allow.processes.is_empty() && !l.allow.processes.contains(&process) {
                return Err(self.denied(
                    l,
                    "allow.processes",
                    format!("に {:?} を足す", w.process),
                    format!(
                        "プロセス {:?} は allow.processes に無い: {}",
                        w.process, l.source
                    ),
                ));
            }
            if !l.allow.classes.is_empty() && !l.allow.classes.contains(&class) {
                return Err(self.denied(
                    l,
                    "allow.classes",
                    format!("に {:?} を足す", w.class),
                    format!(
                        "ウィンドウクラス {:?} は allow.classes に無い: {}",
                        w.class, l.source
                    ),
                ));
            }
        }
        Ok(())
    }

    /// deny だけの判定 (D-077。撮る矩形に見えている機密のウィンドウ): 層を順に見て、最初に当たった
    /// `deny.processes` か `deny.classes` を返す。allow の一覧は見ない (allow に無いだけの窓は通す)。
    pub fn check_window_deny(&self, w: &WindowInfo) -> Check {
        let process = normalize_process(&w.process);
        let class = normalize_class(&w.class);
        match self
            .layers
            .iter()
            .find_map(|l| self.window_deny(l, w, &process, &class))
        {
            Some(d) => Err(d),
            None => Ok(()),
        }
    }

    /// どれかの層に `deny.processes` か `deny.classes` があるか (無ければ撮影の前に窓を列挙しない。D-077)。
    pub fn has_window_deny(&self) -> bool {
        self.layers
            .iter()
            .any(|l| !l.deny.processes.is_empty() || !l.deny.classes.is_empty())
    }

    /// プロセス名だけの判定 (`focus` の対象を、クラスを知らない検証の段階で見る。D-050)。
    pub fn check_process(&self, process: &str) -> Check {
        let norm = normalize_process(process);
        for l in &self.layers {
            if l.deny.processes.contains(&norm) {
                return Err(self.denied(
                    l,
                    "deny.processes",
                    format!("から {process:?} を外す"),
                    format!(
                        "プロセス {process} は deny.processes に当たる: {}",
                        l.source
                    ),
                ));
            }
            if !l.allow.processes.is_empty() && !l.allow.processes.contains(&norm) {
                return Err(self.denied(
                    l,
                    "allow.processes",
                    format!("に {process:?} を足す"),
                    format!(
                        "プロセス {process:?} は allow.processes に無い: {}",
                        l.source
                    ),
                ));
            }
        }
        Ok(())
    }

    /// どれかの層の `allow.processes` に書かれた名前 (正規化済み) のうち、`check_process` (deny と他の層の allow) を
    /// 通るもの。重ねを除いて名前の順。どの層にも `allow.processes` が無ければ (プロセスの制限なし) 空 (D-074)
    pub fn named_allowed_processes(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .layers
            .iter()
            .flat_map(|l| l.allow.processes.iter().cloned())
            .filter(|n| self.check_process(n).is_ok())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// 文字数の上限: 判定は全層の最小値。断る層は床から順に見て足りない最初の層 (D-079)
    pub fn check_text_len(&self, text: &str) -> Check {
        let n = text.chars().count();
        let max = self.max_text_len();
        if n > max as usize {
            let l = self.first_short_layer(n as u64, |l| u64::from(l.limits.max_text_len));
            return Err(self.denied(
                l,
                "limits.max_text_len",
                format!("を {n} 以上にする"),
                format!("文字数 {n} が limits.max_text_len ({max}) を超える"),
            ));
        }
        Ok(())
    }

    /// 操作の数の上限 (`limits.max_actions`): 判定は全層の最小値。断る層は床から順に見て足りない最初の層 (D-079)
    pub fn check_action_count(&self, n: usize) -> Check {
        let max = self.max_actions();
        if n > max as usize {
            let l = self.first_short_layer(n as u64, |l| u64::from(l.limits.max_actions));
            return Err(self.denied(
                l,
                "limits.max_actions",
                format!("を {n} 以上にする"),
                format!("操作の数 {n} が limits.max_actions ({max}) を超える"),
            ));
        }
        Ok(())
    }

    /// 待ちの合計と実行の余裕の上限 (`limits.max_duration_ms`): 判定は全層の最小値。断る層は床から順に見て
    /// 足りない最初の層 (D-079)
    pub fn check_duration(&self, total_sleep_ms: u64, margin_ms: u64) -> Check {
        let need = total_sleep_ms.saturating_add(margin_ms);
        let limit = self.max_duration_ms();
        if need > limit {
            let l = self.first_short_layer(need, |l| l.limits.max_duration_ms);
            return Err(self.denied(
                l,
                "limits.max_duration_ms",
                format!("を {need} 以上にする"),
                format!(
                    "待ちの合計 {total_sleep_ms} ms (focus、close、wait_for、wait_stable、wait_change、screenshot の settle の待ちの上限を含む) と実行の余裕 {margin_ms} ms が limits.max_duration_ms ({limit}) を超える"
                ),
            ));
        }
        Ok(())
    }

    /// 全画面の撮影 (D-049): `rect` が無い (region の省略) か、画面全体を覆う矩形なら全画面とみなし、
    /// どの層も `capture.full_screen` を許すときだけ通す。画面の大きさを知らないときは、矩形があれば判定しない。
    pub fn check_full_screen(&self, rect: Option<&Rect>, screen: Option<&Rect>) -> Check {
        let full = match (rect, screen) {
            (None, _) => true,
            (Some(r), Some(s)) => r.contains_rect(s),
            (Some(_), None) => false,
        };
        if !full {
            return Ok(());
        }
        for l in &self.layers {
            if !l.capture.full_screen {
                return Err(self.denied(
                    l,
                    "capture.full_screen",
                    "を true にする".into(),
                    format!(
                        "全画面の撮影は許可されていない (capture.full_screen = false)。region で対象のウィンドウだけを指定する: {}",
                        l.source
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 撮影の保存先のフォルダ (D-075): `capture.save_dirs` が空でない層のすべてで、`abs` (絶対化して字面で
    /// 正規化した保存先) がその層の一覧のどれかの下にあるときだけ通す。空の層は判定に加わらない。
    pub fn check_save_dir(&self, abs: &Path) -> Check {
        for l in self.layers.iter().filter(|l| !l.save_dirs.is_empty()) {
            if !l.save_dirs.iter().any(|d| path_within(abs, d)) {
                let list: Vec<String> = l
                    .save_dirs
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect();
                let dir = abs.parent().unwrap_or(abs);
                return Err(self.denied(
                    l,
                    "capture.save_dirs",
                    format!("に {} を足す", dir.display()),
                    format!(
                        "撮影の保存先 {} は capture.save_dirs ({}) のどのフォルダの下でもない: {}",
                        abs.display(),
                        l.source,
                        list.join(", ")
                    ),
                ));
            }
        }
        Ok(())
    }

    /// パスワードの要素 (UIA の IsPassword) にフォーカスがあるときの type / key を許すか (D-090)。
    /// `input_enabled` と同じ合成: どの層も `general.password_input` が真のときだけ許し、床から順に見て
    /// 最初の偽の層で断る (上の層は倒すだけ)
    pub fn password_input_allowed(&self) -> Check {
        match self.layers.iter().find(|l| !l.general.password_input) {
            Some(l) => Err(self.denied(
                l,
                "general.password_input",
                "を true にする".into(),
                format!(
                    "パスワード欄への入力が許可されていない (general.password_input = false): {}",
                    l.source
                ),
            )),
            None => Ok(()),
        }
    }

    /// `shot` の前に求める無操作の時間 (層の中で最も大きい = 最も厳しい値。0 なら求めない)。
    pub fn capture_min_idle_ms(&self) -> u64 {
        self.layers
            .iter()
            .map(|l| l.capture.min_idle_ms)
            .max()
            .unwrap_or(0)
    }

    /// 画面を変える操作の前に枠を出すか (D-093)。どれかの層が true なら出す (すべての層が false の
    /// ときだけ出さない)。知らせる側を安全側とみるので、上の層は出させられるが消せない。
    /// `[indicator]` を書いていない層は既定の true で加わる
    pub fn indicator_enabled(&self) -> bool {
        self.layers.iter().any(|l| l.indicator.enabled)
    }

    /// 予告の間の長さ (ミリ秒。D-093)。層の中で最も大きい値 (上の層は延ばせるが縮められない)。
    /// 書いていない層は既定の 3000 で加わる
    pub fn indicator_lead_ms(&self) -> u64 {
        self.layers
            .iter()
            .map(|l| l.indicator.lead_ms)
            .max()
            .unwrap_or(0)
    }

    /// 予告の間に入るときに通知音を鳴らすか (D-093)。どれかの層が true なら鳴らす
    pub fn indicator_sound(&self) -> bool {
        self.layers.iter().any(|l| l.indicator.sound)
    }

    /// 実入力が有効で、アプリ、クラス、領域の許可リストがどの層でも空か (どのアプリ、どの画面にも送れる)。
    pub fn unrestricted(&self) -> bool {
        self.layers.iter().all(|l| {
            l.general.input_enabled
                && l.allow.processes.is_empty()
                && l.allow.classes.is_empty()
                && l.allow.regions.is_empty()
        })
    }

    /// どれかの層に領域の規則 (allow か deny) があるか。
    pub fn has_region_rules(&self) -> bool {
        self.layers
            .iter()
            .any(|l| !l.allow.regions.is_empty() || !l.deny.regions.is_empty())
    }

    pub fn max_actions(&self) -> u32 {
        self.layers
            .iter()
            .map(|l| l.limits.max_actions)
            .min()
            .unwrap_or(0)
    }

    pub fn max_duration_ms(&self) -> u64 {
        self.layers
            .iter()
            .map(|l| l.limits.max_duration_ms)
            .min()
            .unwrap_or(0)
    }

    pub fn max_text_len(&self) -> u32 {
        self.layers
            .iter()
            .map(|l| l.limits.max_text_len)
            .min()
            .unwrap_or(0)
    }

    /// 停止とみなすマウス移動の閾値 (層の中で最も小さい = 最も止まりやすい値)。
    pub fn mouse_threshold_px(&self) -> u32 {
        self.layers
            .iter()
            .map(|l| l.stop.mouse_threshold_px)
            .min()
            .unwrap_or(1)
    }

    /// 呼ぶたびに客に人の承認を求めさせる MCP の道具 (D-078)。全層の `mcp.require_approval` の和集合で、
    /// 上の層は足せるが外せない。閉じた層 (床なし) は何も足さない。
    pub fn mcp_require_approval(&self) -> BTreeSet<String> {
        self.layers
            .iter()
            .flat_map(|l| l.mcp_require_approval.iter().cloned())
            .collect()
    }

    /// 停止の目印のファイル (どの層のものも有効。足すことしかできない)。
    pub fn sentinel_files(&self) -> Vec<String> {
        self.layers
            .iter()
            .map(|l| l.stop.sentinel_file.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// 層 1 つ分のウィンドウの deny (`check_window` と `check_window_deny` が共有する。D-077)。
    /// `process` と `class` は正規化済みの値で、文には `w` の元の字面を出す。
    fn window_deny(&self, l: &Layer, w: &WindowInfo, process: &str, class: &str) -> Option<Denied> {
        if l.deny.processes.iter().any(|p| p == process) {
            return Some(self.denied(
                l,
                "deny.processes",
                format!("から {:?} を外す", w.process),
                format!(
                    "プロセス {} は deny.processes に当たる: {}",
                    w.process, l.source
                ),
            ));
        }
        if l.deny.classes.iter().any(|c| c == class) {
            return Some(self.denied(
                l,
                "deny.classes",
                format!("から {:?} を外す", w.class),
                format!(
                    "ウィンドウクラス {} は deny.classes に当たる: {}",
                    w.class, l.source
                ),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn layer(toml: &str) -> Layer {
        let cfg = Config::parse(&format!("version = 1\n{toml}")).unwrap();
        Layer::from_config(&cfg, "test").unwrap()
    }

    fn policy(tomls: &[&str]) -> Policy {
        Policy::new(tomls.iter().map(|t| layer(t)).collect())
    }

    const ENABLED: &str = "[general]\ninput_enabled = true\ncapture_enabled = true\n";

    fn win(process: &str, class: &str) -> WindowInfo {
        WindowInfo {
            hwnd: 1,
            pid: 2,
            process: process.into(),
            class: class.into(),
            title: String::new(),
            elevated: Some(false),
        }
    }

    fn key(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    /// プロセス名だけの判定 (D-050): deny が allow に勝つ、正規化 (大文字小文字、.exe、パス)、どの層も通ること
    #[test]
    fn policy_check_process_deny_wins_and_normalizes() {
        let p = policy(&[&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\", \"Code.exe\"]\n[deny]\nprocesses=[\"code\"]\n"
        )]);
        assert!(p.check_process("NOTEPAD.EXE").is_ok());
        assert!(p.check_process("C:\\Windows\\notepad.exe").is_ok());
        let d = p.check_process("code.exe").unwrap_err();
        assert!(d.message.contains("deny.processes"), "{}", d.message);
        let d = p.check_process("chrome.exe").unwrap_err();
        assert!(d.message.contains("allow.processes"), "{}", d.message);
        // 2 層: 上の層が deny を足せば止まる。allow が空の層は制限しない
        let two = policy(&[
            &format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n"),
            &format!("{ENABLED}[deny]\nprocesses=[\"notepad\"]\n"),
        ]);
        assert!(two.check_process("notepad.exe").is_err());
        assert!(policy(&[ENABLED]).check_process("anything.exe").is_ok());
    }

    /// 名前の出る許可済みのプロセス (D-074): 層の allow の積で、deny で落ちたものは出さない。allow がどの層にも無ければ空
    #[test]
    fn named_allowed_processes_intersects_layers_and_drops_denied() {
        let p = policy(&[
            &format!(
                "{ENABLED}[allow]\nprocesses=[\"a\", \"B.exe\", \"c\"]\n[deny]\nprocesses=[\"c\"]\n"
            ),
            &format!("{ENABLED}[allow]\nprocesses=[\"b\", \"c\", \"d\"]\n"),
        ]);
        // c は両方の allow にあり、deny でしか落ちない。a と d は片方の allow にしか無い
        assert_eq!(p.named_allowed_processes(), vec!["b".to_string()]);
        // 1 層: 重ねなしで名前の順
        let one = policy(&[&format!(
            "{ENABLED}[allow]\nprocesses=[\"notepad\", \"alpha\", \"Notepad.EXE\"]\n"
        )]);
        assert_eq!(
            one.named_allowed_processes(),
            vec!["alpha".to_string(), "notepad".to_string()]
        );
        // allow.processes がどの層にも無い (制限なし) なら空。deny だけの層も同じ
        assert!(policy(&[ENABLED]).named_allowed_processes().is_empty());
        assert!(
            policy(&[&format!("{ENABLED}[deny]\nprocesses=[\"x\"]\n")])
                .named_allowed_processes()
                .is_empty()
        );
    }

    /// 全画面の撮影 (D-049): region の省略と、画面全体を覆う矩形が全画面。どの層も許すときだけ通る
    #[test]
    fn policy_full_screen_capture_requires_every_layer_to_allow() {
        let screen = Rect::new(0, 0, 3840, 2160);
        let open = "[capture]\nfull_screen = true\n";
        let both = policy(&[&format!("{ENABLED}{open}"), &format!("{ENABLED}{open}")]);
        assert!(both.check_full_screen(None, Some(&screen)).is_ok());
        assert!(both.check_full_screen(None, None).is_ok());
        let one = policy(&[&format!("{ENABLED}{open}"), ENABLED]);
        let d = one.check_full_screen(None, Some(&screen)).unwrap_err();
        assert!(d.message.contains("full_screen"), "{}", d.message);
        // 画面全体を覆う矩形 (ちょうど同じ、またはより大きい) も全画面
        assert!(one.check_full_screen(Some(&screen), Some(&screen)).is_err());
        assert!(
            one.check_full_screen(Some(&Rect::new(-10, -10, 4000, 2200)), Some(&screen))
                .is_err()
        );
        // 一部なら通る。画面の大きさを知らないときは、矩形があれば判定しない (通す)
        assert!(
            one.check_full_screen(Some(&Rect::new(0, 0, 3839, 2160)), Some(&screen))
                .is_ok()
        );
        assert!(one.check_full_screen(Some(&screen), None).is_ok());
        // 省略は、画面の大きさを知らなくても全画面
        assert!(one.check_full_screen(None, None).is_err());
    }

    /// 撮影の保存先のフォルダ (D-075): save_dirs を書いた層のすべてが許すときだけ通る。書かない層は加わらない
    #[test]
    fn policy_save_dirs_every_declaring_layer_must_contain_path() {
        let ok = |p: &Policy, s: &str| p.check_save_dir(Path::new(s)).is_ok();
        let none = policy(&[ENABLED, ENABLED]);
        assert!(ok(&none, "C:\\anywhere\\a.png"));
        let shots = "[capture]\nsave_dirs = ['C:\\Shots']\n";
        // 床だけ書く
        let floor = policy(&[shots, ENABLED]);
        assert!(ok(&floor, "c:\\shots\\a.png"));
        assert!(ok(&floor, "C:\\Shots\\sub\\a.png"));
        for bad in ["C:\\shots2\\a.png", "C:\\other\\a.png"] {
            let d = floor.check_save_dir(Path::new(bad)).unwrap_err();
            assert!(d.message.contains("capture.save_dirs"), "{}", d.message);
            assert!(d.message.contains("C:\\Shots"), "{}", d.message);
        }
        // 上の層だけ書く: 上の層の一覧で絞られる
        let upper = policy(&[ENABLED, shots]);
        assert!(ok(&upper, "c:\\shots\\a.png"));
        assert!(!ok(&upper, "C:\\other\\a.png"));
        // 両方書く: 交わりだけ通る
        let both = policy(&[
            "[capture]\nsave_dirs = ['C:\\a']\n",
            "[capture]\nsave_dirs = ['C:\\a\\b']\n",
        ]);
        assert!(ok(&both, "C:\\a\\b\\x.png"));
        assert!(!ok(&both, "C:\\a\\x.png"));
    }

    /// 毎回の承認を求める MCP の道具 (D-078) は、全層の和集合。閉じた層は何も足さない
    #[test]
    fn mcp_require_approval_is_union_of_layers() {
        let set =
            |names: &[&str]| -> BTreeSet<String> { names.iter().map(|s| s.to_string()).collect() };
        let click = "[mcp]\nrequire_approval = ['click']\n";
        let typ = "[mcp]\nrequire_approval = ['Type']\n";
        assert_eq!(
            policy(&[click, typ]).mcp_require_approval(),
            set(&["click", "type"])
        );
        // 床だけに書く / 上書き層だけに書く: どちらも効く
        assert_eq!(
            policy(&[click, ENABLED]).mcp_require_approval(),
            set(&["click"])
        );
        assert_eq!(
            policy(&[ENABLED, typ]).mcp_require_approval(),
            set(&["type"])
        );
        // 閉じた層と書いた層の組は、書いた層の分だけ
        let p = Policy::new(vec![Layer::closed("closed"), layer(typ)]);
        assert_eq!(p.mcp_require_approval(), set(&["type"]));
        assert!(Policy::closed().mcp_require_approval().is_empty());
        assert!(policy(&[ENABLED]).mcp_require_approval().is_empty());
    }

    /// 無操作の要求 (D-049) は、層の中で最も大きい値 (最も厳しい値)
    #[test]
    fn policy_capture_min_idle_is_the_largest_across_layers() {
        // 大きい方を先に置く (最後の層の値を返す実装と区別する)
        let p = policy(&[
            &format!("{ENABLED}[capture]\nmin_idle_ms = 5000\n"),
            &format!("{ENABLED}[capture]\nmin_idle_ms = 1000\n"),
        ]);
        assert_eq!(p.capture_min_idle_ms(), 5000);
        let off = policy(&[
            &format!("{ENABLED}[capture]\nmin_idle_ms = 0\n"),
            &format!("{ENABLED}[capture]\nmin_idle_ms = 0\n"),
        ]);
        assert_eq!(off.capture_min_idle_ms(), 0);
        // 既定は 3000
        assert_eq!(policy(&[ENABLED]).capture_min_idle_ms(), 3000);
    }

    /// 枠の設定の合成 (D-093): enabled と sound はどれかの層が true なら true、lead_ms は層の最大。
    /// `[indicator]` の無い層は既定値 (true、3000、false) で加わるので、基本の層だけで弱めても
    /// 追加の層を使う実行では既定に戻る
    #[test]
    fn policy_indicator_combines_layers() {
        let ind = |enabled: bool, lead_ms: u64, sound: bool| {
            format!("[indicator]\nenabled = {enabled}\nlead_ms = {lead_ms}\nsound = {sound}\n")
        };
        // (層ごとの設定, enabled, lead_ms, sound)
        let cases: Vec<(Vec<String>, bool, u64, bool)> = vec![
            // enabled: [false, true] → true、[true, false] → true (順に依らない)、[false, false] → false
            (
                vec![ind(false, 0, false), ind(true, 0, false)],
                true,
                0,
                false,
            ),
            (
                vec![ind(true, 0, false), ind(false, 0, false)],
                true,
                0,
                false,
            ),
            (
                vec![ind(false, 0, false), ind(false, 0, false)],
                false,
                0,
                false,
            ),
            // lead_ms: 大きい方を先に置く (最後の層の値を返す実装と区別する)
            (
                vec![ind(true, 3000, false), ind(true, 500, false)],
                true,
                3000,
                false,
            ),
            (
                vec![ind(true, 0, false), ind(true, 5000, false)],
                true,
                5000,
                false,
            ),
            // sound: [false, true] → true、[true, false] → true、[false, false] → false
            (vec![ind(true, 0, false), ind(true, 0, true)], true, 0, true),
            (vec![ind(true, 0, true), ind(true, 0, false)], true, 0, true),
            (
                vec![ind(true, 0, false), ind(true, 0, false)],
                true,
                0,
                false,
            ),
            // 基本の層だけで弱めても、[indicator] の無い追加の層が既定値で加わる
            (vec![ind(false, 0, false), String::new()], true, 3000, false),
            // 層 1 つ: 書いたまま。書かなければ既定
            (vec![ind(false, 0, false)], false, 0, false),
            (vec![String::new()], true, 3000, false),
        ];
        for (layers, enabled, lead_ms, sound) in cases {
            let tomls: Vec<&str> = layers.iter().map(String::as_str).collect();
            let p = policy(&tomls);
            assert_eq!(p.indicator_enabled(), enabled, "{layers:?}");
            assert_eq!(p.indicator_lead_ms(), lead_ms, "{layers:?}");
            assert_eq!(p.indicator_sound(), sound, "{layers:?}");
        }
        // 閉じた層 (基本の層なし) も既定値で加わる
        let closed = Policy::closed();
        assert!(closed.indicator_enabled());
        assert_eq!(closed.indicator_lead_ms(), 3000);
        assert!(!closed.indicator_sound());
    }

    /// deny だけの判定 (D-077): allow.processes / allow.classes に無いだけなら通る。
    /// 上の層の deny.classes でも当たる。大文字小文字とパスは正規化して比べる
    #[test]
    fn check_window_deny_ignores_allow_lists_and_hits_any_layer() {
        let p = policy(&[
            &format!(
                "{ENABLED}[allow]\nprocesses=[\"notepad\"]\nclasses=[\"Notepad\"]\n[deny]\nprocesses=[\"Zed.exe\"]\n"
            ),
            &format!("{ENABLED}[deny]\nclasses=[\"ConsoleWindowClass\"]\n"),
        ]);
        // allow に無いだけ (check_window は断る) でも deny には当たらない
        let other = win("calc.exe", "CalcFrame");
        assert!(p.check_window(&other).is_err());
        assert!(p.check_window_deny(&other).is_ok());
        // 床の deny.processes (大文字小文字、パス)
        for name in ["zed.exe", "ZED.EXE", "C:\\Apps\\Zed.exe"] {
            let d = p.check_window_deny(&win(name, "x")).unwrap_err();
            assert!(d.message.contains("deny.processes"), "{}", d.message);
        }
        // 上の層の deny.classes (大文字小文字)
        let d = p
            .check_window_deny(&win("cmd.exe", "consolewindowclass"))
            .unwrap_err();
        assert!(d.message.contains("deny.classes"), "{}", d.message);
        assert!(d.message.contains("consolewindowclass"), "{}", d.message);
        // 名前の読めない窓はクラスだけで判定する
        assert!(p.check_window_deny(&win("", "Other")).is_ok());
        assert!(p.check_window_deny(&win("", "ConsoleWindowClass")).is_err());
        // deny の無い設定はどれも通る
        let open = policy(&[&format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n")]);
        assert!(open.check_window_deny(&other).is_ok());
    }

    /// どれかの層に deny.processes か deny.classes があれば真。他の次元の deny は数えない
    #[test]
    fn has_window_deny_reports_any_layer() {
        assert!(!policy(&[ENABLED]).has_window_deny());
        assert!(!policy(&[""]).has_window_deny());
        assert!(
            !policy(&[&format!(
                "{ENABLED}[allow]\nprocesses=[\"notepad\"]\nclasses=[\"Notepad\"]\n[deny]\nactions=[\"click\"]\nkeys=[\"ctrl+w\"]\nregions=[\"0,0,10,10\"]\n"
            )])
            .has_window_deny()
        );
        assert!(policy(&[ENABLED, "[deny]\nprocesses=[\"zed\"]\n"]).has_window_deny());
        assert!(policy(&["[deny]\nclasses=[\"X\"]\n", ENABLED]).has_window_deny());
    }

    /// window_deny を切り出した後も、check_window は層ごとに deny → allow の順で見る
    /// (全層の deny を先に見ると、床の allow の文ではなく上の層の deny の文になる)
    #[test]
    fn check_window_keeps_layer_order_after_window_deny_split() {
        let p = policy(&[
            &format!("{ENABLED}[allow]\nprocesses=[\"notepad.exe\"]\n"),
            &format!("{ENABLED}[deny]\nprocesses=[\"zed.exe\"]\n"),
        ]);
        let d = p.check_window(&win("zed.exe", "Zed::Window")).unwrap_err();
        assert_eq!(
            d.message,
            "プロセス \"zed.exe\" は allow.processes に無い: test"
        );
        // deny だけの判定は上の層の deny に当たる
        let d = p
            .check_window_deny(&win("zed.exe", "Zed::Window"))
            .unwrap_err();
        assert_eq!(
            d.message,
            "プロセス zed.exe は deny.processes に当たる: test"
        );
        assert!(p.check_window(&win("notepad.exe", "Notepad")).is_ok());
    }

    #[test]
    fn policy_empty_config_denies_input_and_capture() {
        let p = policy(&[""]);
        assert!(p.check_action_kind(ActionKind::Click).is_err());
        assert!(p.check_action_kind(ActionKind::Screenshot).is_err());
        assert!(p.check_action_kind(ActionKind::Wait).is_ok());
        assert!(
            Policy::closed()
                .check_action_kind(ActionKind::Type)
                .is_err()
        );
    }

    /// wait_for (D-068) は撮影の総スイッチ (capture_enabled) で見て、input_enabled は要らない。
    /// deny.actions と allow.actions の対象
    #[test]
    fn policy_wait_for_uses_capture_switch_not_input_switch() {
        let capture_only = policy(&["[general]\ncapture_enabled = true\n"]);
        assert!(capture_only.check_action_kind(ActionKind::WaitFor).is_ok());
        assert!(capture_only.check_action_kind(ActionKind::Click).is_err());
        let input_only = policy(&["[general]\ninput_enabled = true\n"]);
        let d = input_only
            .check_action_kind(ActionKind::WaitFor)
            .unwrap_err();
        assert!(d.message.contains("capture_enabled"), "{}", d.message);
        let denied = policy(&[&format!("{ENABLED}[deny]\nactions=[\"wait-for\"]\n")]);
        assert!(denied.check_action_kind(ActionKind::WaitFor).is_err());
        let only_click = policy(&[&format!("{ENABLED}[allow]\nactions=[\"click\"]\n")]);
        assert!(only_click.check_action_kind(ActionKind::WaitFor).is_err());
        assert!(only_click.check_action_kind(ActionKind::Wait).is_ok());
    }

    #[test]
    fn policy_deny_wins_over_allow_in_every_dimension() {
        let p = policy(&[&format!(
            "{ENABLED}[allow]\nactions=[\"click\"]\nprocesses=[\"notepad\"]\nclasses=[\"Edit\"]\nregions=[\"0,0,100,100\"]\nkeys=[\"ctrl+s\"]\n\
             [deny]\nactions=[\"click\"]\nprocesses=[\"notepad\"]\nclasses=[\"edit\"]\nregions=[\"0,0,10,10\"]\nkeys=[\"ctrl+s\"]\n"
        )]);
        assert!(p.check_action_kind(ActionKind::Click).is_err());
        assert!(p.check_window(&win("notepad.exe", "Other")).is_err());
        assert!(p.check_window(&win("other.exe", "EDIT")).is_err());
        assert!(p.check_point(Point::new(5, 5)).is_err());
        assert!(p.check_point(Point::new(50, 50)).is_ok());
        assert!(p.check_key(&key("ctrl+s")).is_err());
    }

    #[test]
    fn policy_nonempty_allow_requires_match() {
        let p = policy(&[&format!(
            "{ENABLED}[allow]\nactions=[\"move\"]\nprocesses=[\"notepad\"]\nregions=[\"0,0,100,100\"]\nkeys=[\"ctrl+s\"]\n"
        )]);
        assert!(p.check_action_kind(ActionKind::Move).is_ok());
        assert!(p.check_action_kind(ActionKind::Click).is_err());
        assert!(p.check_window(&win("Notepad.EXE", "x")).is_ok());
        assert!(p.check_window(&win("calc.exe", "x")).is_err());
        assert!(p.check_point(Point::new(100, 0)).is_err());
        assert!(p.check_key(&key("Control+S")).is_ok());
        assert!(p.check_key(&key("shift+ctrl+s")).is_err());
        assert!(p.check_key(&key("ctrl+a")).is_err());
    }

    #[test]
    fn policy_case_insensitive_matching() {
        let p = policy(&[&format!(
            "{ENABLED}[deny]\nprocesses=[\"C:\\\\Windows\\\\System32\\\\CMD.EXE\"]\nclasses=[\"ConsoleWindowClass\"]\nactions=[\"Double-Click\"]\nkeys=[\"Control+Shift+T\"]\n"
        )]);
        assert!(p.check_window(&win("cmd.exe", "x")).is_err());
        assert!(p.check_window(&win("x.exe", "consolewindowclass")).is_err());
        assert!(p.check_action_kind(ActionKind::DoubleClick).is_err());
        assert!(p.check_key(&key("ctrl+shift+t")).is_err());
    }

    #[test]
    fn policy_builtin_key_deny_cannot_be_allowed() {
        let p = policy(&[&format!(
            "{ENABLED}[allow]\nkeys=[\"alt+f4\", \"win+r\", \"ctrl+alt+delete\"]\n"
        )]);
        for k in ["alt+f4", "win+r", "ctrl+alt+delete", "ctrl+shift+esc"] {
            assert!(p.check_key(&key(k)).is_err(), "{k}");
        }
    }

    /// 別名で書いた deny.keys が正規名の操作に効く (D-131)
    #[test]
    fn deny_keys_alias_matches_canonical_key() {
        let p = policy(&[&format!("{ENABLED}[deny]\nkeys=[\"hankaku\"]\n")]);
        let d = p.check_key(&key("kanji")).unwrap_err();
        assert_eq!(d.rule, Some("deny.keys"), "{}", d.message);
    }

    #[test]
    fn policy_overlay_can_add_deny_but_not_widen_allow() {
        // 床は notepad だけ許可。追加の層が calc を allow しても、床で拒否される
        let floor = format!("{ENABLED}[allow]\nprocesses=[\"notepad\"]\n");
        let overlay = format!(
            "{ENABLED}[allow]\nprocesses=[\"calc\", \"notepad\"]\n[deny]\nkeys=[\"ctrl+w\"]\n"
        );
        let p = policy(&[&floor, &overlay]);
        assert!(p.check_window(&win("calc.exe", "")).is_err());
        assert!(p.check_window(&win("notepad.exe", "")).is_ok());
        assert!(p.check_key(&key("ctrl+w")).is_err());
        // 床で無効なら、追加の層で有効にしても無効
        let p = policy(&["", ENABLED]);
        assert!(p.check_action_kind(ActionKind::Click).is_err());
        // 追加の層で無効にできる
        let p = policy(&[ENABLED, "[general]\ninput_enabled = false\n"]);
        assert!(p.check_action_kind(ActionKind::Click).is_err());
    }

    #[test]
    fn policy_limits_take_the_minimum() {
        let p = policy(&[
            "[limits]\nmax_actions = 10\nmax_text_len = 5\n[stop]\nmouse_threshold_px = 80\nsentinel_file = 'C:\\stop-a'\n",
            "[limits]\nmax_actions = 20\nmax_text_len = 3\n[stop]\nmouse_threshold_px = 30\nsentinel_file = 'C:\\stop-b'\n",
        ]);
        assert_eq!(p.max_actions(), 10);
        assert_eq!(p.max_text_len(), 3);
        assert_eq!(p.mouse_threshold_px(), 30);
        assert_eq!(
            p.sentinel_files(),
            vec!["C:\\stop-a".to_string(), "C:\\stop-b".to_string()]
        );
        assert!(p.check_text_len("abc").is_ok());
        assert!(p.check_text_len("abcd").is_err());
    }

    #[test]
    fn policy_capture_rect_rules() {
        let p = policy(&[&format!(
            "{ENABLED}[allow]\nregions=[\"0,0,100,100\"]\n[deny]\nregions=[\"90,90,20,20\"]\n"
        )]);
        assert!(p.check_capture_rect(&Rect::new(0, 0, 50, 50)).is_ok());
        assert!(p.check_capture_rect(&Rect::new(0, 0, 95, 95)).is_err());
        assert!(p.check_capture_rect(&Rect::new(50, 50, 60, 10)).is_err());
    }

    /// 出所を決めた床の層 (D-079 の fix の <どこで> を照合する)
    fn floor_at(toml: &str, source: &str) -> Layer {
        let mut l = layer(toml);
        l.source = source.into();
        l
    }

    /// 出所を決めた追加の層
    fn overlay_at(toml: &str, source: &str) -> Layer {
        let mut l = floor_at(toml, source);
        l.role = LayerRole::Overlay;
        l
    }

    /// 床の場所はあるがファイルが無い閉じた層 (`load_layers` が作るものと同じ役割)
    fn no_floor() -> Layer {
        let mut l = Layer::closed("(基本の層なし) target/nope.toml が無い");
        l.role = LayerRole::NoFloor;
        l
    }

    const FLOOR_FIX_HEAD: &str = "許可してよければ、端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動した TUI の「";

    /// TUI の左の一覧での場所 (D-080) は CONFIG_RULES のどれにもあり、設定のキーでないものには無い
    #[test]
    fn tui_label_covers_config_rules_only() {
        for rule in CONFIG_RULES {
            assert!(tui_label(rule).is_some(), "{rule}");
        }
        for other in [RULE_BUILTIN, RULE_SAVE_PATH, "nope"] {
            assert_eq!(tui_label(other), None, "{other}");
        }
    }

    /// 各次元で 1 つずつ断らせ、rule が当たった設定のキーになる (D-079)。出た rule は CONFIG_RULES と一致する
    #[test]
    fn denied_carries_config_key_as_rule() {
        let p = |t: &str| policy(&[&format!("{ENABLED}{t}")]);
        let rect = |x, y, w, h| Rect::new(x, y, w, h);
        let cases: Vec<(Check, &str)> = vec![
            (
                policy(&[""]).check_action_kind(ActionKind::Click),
                "general.input_enabled",
            ),
            (
                policy(&[""]).check_action_kind(ActionKind::Screenshot),
                "general.capture_enabled",
            ),
            (p("").password_input_allowed(), "general.password_input"),
            (
                p("[deny]\nactions=[\"click\"]\n").check_action_kind(ActionKind::Click),
                "deny.actions",
            ),
            (
                p("[allow]\nactions=[\"move\"]\n").check_action_kind(ActionKind::Click),
                "allow.actions",
            ),
            (
                p("[deny]\nkeys=[\"ctrl+w\"]\n").check_key(&key("ctrl+w")),
                "deny.keys",
            ),
            (
                p("[allow]\nkeys=[\"ctrl+s\"]\n").check_key(&key("ctrl+a")),
                "allow.keys",
            ),
            (
                p("[deny]\nregions=[\"0,0,10,10\"]\n").check_point(Point::new(5, 5)),
                "deny.regions",
            ),
            (
                p("[deny]\nregions=[\"0,0,10,10\"]\n").check_capture_rect(&rect(5, 5, 20, 20)),
                "deny.regions",
            ),
            (
                p("[allow]\nregions=[\"0,0,100,100\"]\n").check_point(Point::new(200, 5)),
                "allow.regions",
            ),
            (
                p("[allow]\nregions=[\"0,0,100,100\"]\n")
                    .check_capture_rect(&rect(50, 50, 100, 100)),
                "allow.regions",
            ),
            (
                p("[deny]\nprocesses=[\"zed\"]\n").check_window(&win("zed.exe", "x")),
                "deny.processes",
            ),
            (
                p("[deny]\nprocesses=[\"zed\"]\n").check_process("zed.exe"),
                "deny.processes",
            ),
            (
                p("[deny]\nprocesses=[\"zed\"]\n").check_window_deny(&win("zed.exe", "x")),
                "deny.processes",
            ),
            (
                p("[allow]\nprocesses=[\"calc\"]\n").check_window(&win("notepad.exe", "x")),
                "allow.processes",
            ),
            (
                p("[allow]\nprocesses=[\"calc\"]\n").check_process("notepad.exe"),
                "allow.processes",
            ),
            (
                p("[deny]\nclasses=[\"Edit\"]\n").check_window(&win("a.exe", "edit")),
                "deny.classes",
            ),
            (
                p("[allow]\nclasses=[\"Edit\"]\n").check_window(&win("a.exe", "Other")),
                "allow.classes",
            ),
            (p("").check_full_screen(None, None), "capture.full_screen"),
            (
                p("[capture]\nsave_dirs = ['C:\\Shots']\n")
                    .check_save_dir(Path::new("C:\\other\\a.png")),
                "capture.save_dirs",
            ),
            (
                p("[limits]\nmax_text_len = 3\n").check_text_len("abcd"),
                "limits.max_text_len",
            ),
            (
                p("[limits]\nmax_actions = 1\n").check_action_count(2),
                "limits.max_actions",
            ),
            (
                p("[limits]\nmax_duration_ms = 1500\n").check_duration(600, 1000),
                "limits.max_duration_ms",
            ),
        ];
        let mut seen = BTreeSet::new();
        for (r, want) in cases {
            let d = r.expect_err(want);
            assert_eq!(d.rule, Some(want), "{}", d.message);
            assert!(d.fix.is_some(), "基本の層で断ったので直し方がある: {want}");
            assert!(CONFIG_RULES.contains(&want), "{want}");
            seen.insert(want);
        }
        let all: BTreeSet<&str> = CONFIG_RULES.into_iter().collect();
        assert_eq!(seen, all, "どの設定のキーも一度は出る");
    }

    /// fix の <どこで> は層の役割で変わる (D-079): 床、床が通して追加の層が断る、床なし、場所を引けない
    #[test]
    fn fix_names_floor_overlay_and_missing_floor() {
        let notepad = win("notepad.exe", "Notepad");
        let floor_only = Policy::new(vec![floor_at(
            &format!("{ENABLED}[allow]\nprocesses=[\"calc.exe\"]\n"),
            "C:\\cfg\\config.toml",
        )]);
        let d = floor_only.check_window(&notepad).unwrap_err();
        assert_eq!(d.rule, Some("allow.processes"));
        assert_eq!(
            d.fix.as_deref(),
            Some(
                "許可してよければ、端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動した TUI の「許可リスト (allow) > アプリ (プロセス)」か、設定ファイル C:\\cfg\\config.toml の allow.processes に \"notepad.exe\" を足す"
            )
        );
        // 床が通して追加の層が断る: 追加の層のパスを指す
        let overlay = Policy::new(vec![
            floor_at(ENABLED, "C:\\cfg\\config.toml"),
            overlay_at(
                &format!("{ENABLED}[allow]\nprocesses=[\"calc.exe\"]\n"),
                "D:\\o.toml",
            ),
        ]);
        let d = overlay.check_window(&notepad).unwrap_err();
        assert_eq!(
            d.fix.as_deref(),
            Some(
                "許可してよければ、追加の層の設定ファイル D:\\o.toml (--config か AI_CU_CONFIG) の allow.processes に \"notepad.exe\" を足す"
            )
        );
        // 床なし: 別の文で、rule は当たったキーのまま
        let d = Policy::new(vec![no_floor()])
            .check_action_kind(ActionKind::Click)
            .unwrap_err();
        assert_eq!(d.rule, Some("general.input_enabled"));
        assert_eq!(
            d.fix.as_deref(),
            Some(
                "設定ファイルがまだ無いので何も許可されていない。端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動し、TUI で設定を作る (手順は ai-cu guide --setup)"
            )
        );
        // 場所を引けない閉じた層 (Unresolved): rule は同じで、直し方は付けない
        let d = Policy::closed()
            .check_action_kind(ActionKind::Click)
            .unwrap_err();
        assert_eq!(d.rule, Some("general.input_enabled"));
        assert_eq!(d.fix, None);
    }

    /// パスワード欄への入力 (D-090) は、どの層も password_input が真のときだけ許す。上の層は倒すだけ
    #[test]
    fn password_input_allowed_needs_every_layer() {
        const PW: &str = "[general]\ninput_enabled = true\npassword_input = true\n";
        // 床だけで真なら許す
        let floor_only = Policy::new(vec![floor_at(PW, "C:\\cfg\\config.toml")]);
        assert!(floor_only.password_input_allowed().is_ok());
        // 床が真でも上の層が偽なら、上の層で断る
        let upper_off = Policy::new(vec![
            floor_at(PW, "C:\\cfg\\config.toml"),
            overlay_at(ENABLED, "D:\\o.toml"),
        ]);
        let d = upper_off.password_input_allowed().unwrap_err();
        assert_eq!(d.rule, Some("general.password_input"));
        assert!(d.message.contains("D:\\o.toml"), "{}", d.message);
        assert!(
            d.message.contains("general.password_input = false"),
            "{}",
            d.message
        );
        assert_eq!(
            d.fix.as_deref(),
            Some(
                "許可してよければ、追加の層の設定ファイル D:\\o.toml (--config か AI_CU_CONFIG) の general.password_input を true にする"
            )
        );
        // 床が偽なら、上の層が真でも床で断る
        let floor_off = Policy::new(vec![
            floor_at(ENABLED, "C:\\cfg\\config.toml"),
            overlay_at(PW, "D:\\o.toml"),
        ]);
        let d = floor_off.password_input_allowed().unwrap_err();
        assert_eq!(d.rule, Some("general.password_input"));
        assert!(d.message.contains("C:\\cfg\\config.toml"), "{}", d.message);
        assert_eq!(
            d.fix.as_deref(),
            Some(
                "許可してよければ、端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動した TUI の「全般 (総スイッチ) > パスワード欄への入力」か、設定ファイル C:\\cfg\\config.toml の general.password_input を true にする"
            )
        );
        // 閉じた方針 (床を引けない) も断る
        assert!(Policy::closed().password_input_allowed().is_err());
    }

    /// 組み込みの拒否は rule が builtin で、設定では直せないので fix が無い
    #[test]
    fn builtin_key_has_rule_but_no_fix() {
        let d = policy(&[ENABLED]).check_key(&key("win+r")).unwrap_err();
        assert_eq!(d.rule, Some(RULE_BUILTIN));
        assert_eq!(d.rule, Some("builtin"));
        assert_eq!(d.fix, None);
        assert!(d.is_builtin());
        // 総スイッチが閉じた設定でも組み込みの拒否は builtin (総スイッチより先に決まる)
        let d = policy(&[""]).check_key(&key("win+r")).unwrap_err();
        assert!(d.is_builtin());
        // 設定のキーの拒否は builtin ではない
        let d = policy(&[&format!("{ENABLED}[deny]\nkeys=[\"ctrl+w\"]\n")])
            .check_key(&key("ctrl+w"))
            .unwrap_err();
        assert!(!d.is_builtin());
    }

    /// 床が無ければ、追加の層で断っても fix は床なしの文。床の場所を引けなければ fix は無い
    #[test]
    fn missing_floor_overrides_fix_of_any_layer() {
        let tight = overlay_at(
            &format!("{ENABLED}[limits]\nmax_actions = 1\n"),
            "D:\\o.toml",
        );
        let d = Policy::new(vec![no_floor(), tight.clone()])
            .check_action_count(2)
            .unwrap_err();
        assert_eq!(d.rule, Some("limits.max_actions"));
        assert_eq!(d.fix.as_deref(), Some(NO_FLOOR_FIX));
        let d = Policy::new(vec![Layer::closed("(基本の層なし) 引けない"), tight])
            .check_action_count(2)
            .unwrap_err();
        assert_eq!(d.rule, Some("limits.max_actions"));
        assert_eq!(d.fix, None);
    }

    /// 上限は全層の最小値で効くが、fix は床から順に見て値が足りない最初の層を指す
    #[test]
    fn limit_fix_points_at_first_short_layer() {
        let two = |floor: &str, upper: &str| {
            Policy::new(vec![
                floor_at(&format!("{ENABLED}[limits]\n{floor}\n"), "F.toml"),
                overlay_at(&format!("{ENABLED}[limits]\n{upper}\n"), "O.toml"),
            ])
        };
        let text = "x".repeat(60);
        let d = two("max_text_len = 100", "max_text_len = 50")
            .check_text_len(&text)
            .unwrap_err();
        let fix = d.fix.unwrap();
        assert!(
            fix.contains("追加の層の設定ファイル O.toml (--config か AI_CU_CONFIG) の limits.max_text_len を 60 以上にする"),
            "{fix}"
        );
        let d = two("max_text_len = 50", "max_text_len = 40")
            .check_text_len(&text)
            .unwrap_err();
        assert_eq!(
            d.fix.as_deref(),
            Some(
                format!("{FLOOR_FIX_HEAD}上限 > 最大の文字数 (type)」か、設定ファイル F.toml の limits.max_text_len を 60 以上にする").as_str()
            )
        );
        // 判定は今までどおり最小値 (40) で、文もそれを言う
        assert!(d.message.contains("(40)"), "{}", d.message);
        let d = two("max_actions = 10", "max_actions = 2")
            .check_action_count(3)
            .unwrap_err();
        let fix = d.fix.unwrap();
        assert!(
            fix.contains("追加の層の設定ファイル O.toml")
                && fix.ends_with("limits.max_actions を 3 以上にする"),
            "{fix}"
        );
        let d = two("max_duration_ms = 5000", "max_duration_ms = 1500")
            .check_duration(600, 1000)
            .unwrap_err();
        let fix = d.fix.unwrap();
        assert!(
            fix.contains("追加の層の設定ファイル O.toml")
                && fix.ends_with("limits.max_duration_ms を 1600 以上にする"),
            "{fix}"
        );
        // 足りていれば通る (判定は変えない)
        assert!(
            two("max_actions = 10", "max_actions = 3")
                .check_action_count(3)
                .is_ok()
        );
        assert!(
            two("max_duration_ms = 5000", "max_duration_ms = 1600")
                .check_duration(600, 1000)
                .is_ok()
        );
    }
}
