//! キーの組み合わせ (`"ctrl+shift+s"` など) の解析と正規化、仮想キーコードへの対応づけ、組み込みの拒否。
//!
//! 正規化した表記は `ctrl+alt+shift+win+<key>` の順で小文字。設定のキーと操作列のキーは、
//! 必ずこの表記にそろえてから比べる (D-028)。

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Win,
}

impl Modifier {
    pub const ALL: [Modifier; 4] = [
        Modifier::Ctrl,
        Modifier::Alt,
        Modifier::Shift,
        Modifier::Win,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Modifier::Ctrl => "ctrl",
            Modifier::Alt => "alt",
            Modifier::Shift => "shift",
            Modifier::Win => "win",
        }
    }

    pub fn vk(self) -> u16 {
        match self {
            Modifier::Ctrl => 0x11,  // VK_CONTROL
            Modifier::Alt => 0x12,   // VK_MENU
            Modifier::Shift => 0x10, // VK_SHIFT
            Modifier::Win => 0x5B,   // VK_LWIN
        }
    }

    fn parse(s: &str) -> Option<Modifier> {
        Some(match s {
            "ctrl" | "control" | "ctl" => Modifier::Ctrl,
            "alt" | "option" => Modifier::Alt,
            "shift" => Modifier::Shift,
            "win" | "super" | "meta" | "cmd" | "windows" | "lwin" => Modifier::Win,
            _ => return None,
        })
    }
}

/// 修飾キー以外のキー。名前と仮想キーコード。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub vk: u16,
}

/// (正規名, 仮想キーコード, 拡張キーか)。別名は `KEY_ALIASES`。
const NAMED_KEYS: &[(&str, u16, bool)] = &[
    ("backspace", 0x08, false),
    ("tab", 0x09, false),
    ("enter", 0x0D, false),
    ("pause", 0x13, false),
    ("capslock", 0x14, false),
    ("imeon", 0x16, false),
    ("kanji", 0x19, false),
    ("imeoff", 0x1A, false),
    ("esc", 0x1B, false),
    ("convert", 0x1C, false),
    ("nonconvert", 0x1D, false),
    ("space", 0x20, false),
    ("pageup", 0x21, true),
    ("pagedown", 0x22, true),
    ("end", 0x23, true),
    ("home", 0x24, true),
    ("left", 0x25, true),
    ("up", 0x26, true),
    ("right", 0x27, true),
    ("down", 0x28, true),
    ("printscreen", 0x2C, true),
    ("insert", 0x2D, true),
    ("delete", 0x2E, true),
];

const KEY_ALIASES: &[(&str, &str)] = &[
    ("return", "enter"),
    ("escape", "esc"),
    ("del", "delete"),
    ("ins", "insert"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("bs", "backspace"),
    ("arrowleft", "left"),
    ("arrowright", "right"),
    ("arrowup", "up"),
    ("arrowdown", "down"),
    ("hankaku", "kanji"),
    ("zenkaku", "kanji"),
    ("henkan", "convert"),
    ("muhenkan", "nonconvert"),
];

/// 使えるキーの名前の一覧 (正規名だけ。a-z、0-9、f1-f24 は範囲でまとめる)。失敗の文に出す (D-131)。
pub fn usable_key_names() -> String {
    let mut names = vec!["a-z", "0-9", "f1-f24"];
    names.extend(NAMED_KEYS.iter().map(|(n, _, _)| *n));
    names.join(", ")
}

/// 使える修飾キーの一覧 (正規名だけ)。失敗の文に出す (D-131)。
pub fn usable_modifier_names() -> String {
    Modifier::ALL.map(Modifier::name).join(", ")
}

impl Key {
    pub fn parse(s: &str) -> Option<Key> {
        let s = KEY_ALIASES
            .iter()
            .find(|(alias, _)| *alias == s)
            .map(|(_, canon)| *canon)
            .unwrap_or(s);
        if let Some((_, vk, _)) = NAMED_KEYS.iter().find(|(n, _, _)| *n == s) {
            return Some(Key { vk: *vk });
        }
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && (c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Some(Key {
                vk: c.to_ascii_uppercase() as u16,
            });
        }
        if let Some(n) = s.strip_prefix('f').and_then(|d| d.parse::<u16>().ok())
            && (1..=24).contains(&n)
            && !s[1..].starts_with('0')
        {
            return Some(Key { vk: 0x70 + n - 1 });
        }
        None
    }

    pub fn name(&self) -> String {
        if let Some((n, _, _)) = NAMED_KEYS.iter().find(|(_, vk, _)| *vk == self.vk) {
            return (*n).to_string();
        }
        match self.vk {
            0x30..=0x39 | 0x41..=0x5A => ((self.vk as u8) as char).to_ascii_lowercase().to_string(),
            0x70..=0x87 => format!("f{}", self.vk - 0x70 + 1),
            vk => format!("vk{vk:#04x}"),
        }
    }

    /// `KEYEVENTF_EXTENDEDKEY` を付けるべきキーか (矢印、Home/End、Insert/Delete、PageUp/PageDown など)。
    pub fn is_extended(&self) -> bool {
        NAMED_KEYS.iter().any(|(_, vk, ext)| *vk == self.vk && *ext)
    }
}

/// 修飾キーの集合と、修飾キー以外の 1 つのキー。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    /// 正規の順 (ctrl, alt, shift, win) に並べ、重複なし。
    pub modifiers: Vec<Modifier>,
    pub key: Key,
}

impl KeyCombo {
    pub fn has(&self, m: Modifier) -> bool {
        self.modifiers.contains(&m)
    }

    /// 設定で緩められない組み込みの拒否 (D-019、D-028)。拒否の理由を返す。
    pub fn builtin_denied(&self) -> Option<&'static str> {
        const F4: u16 = 0x73;
        const ESC: u16 = 0x1B;
        if self.has(Modifier::Win) {
            return Some("Win キーを含む組み合わせは組み込みで拒否する");
        }
        if self.has(Modifier::Ctrl) && self.has(Modifier::Alt) {
            return Some("Ctrl+Alt+ 系は組み込みで拒否する");
        }
        if self.has(Modifier::Alt) && self.key.vk == F4 {
            return Some("Alt+F4 は組み込みで拒否する");
        }
        if self.has(Modifier::Ctrl) && self.has(Modifier::Shift) && self.key.vk == ESC {
            return Some("Ctrl+Shift+Esc は組み込みで拒否する");
        }
        if self.has(Modifier::Ctrl) && self.key.vk == ESC {
            return Some("Ctrl+Esc (スタートメニュー) は組み込みで拒否する");
        }
        None
    }
}

impl FromStr for KeyCombo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lowered = s.trim().to_ascii_lowercase();
        if lowered.is_empty() {
            return Err("キーが空".into());
        }
        // "ctrl++" のように + 自体を押すことはできない (OEM キーは配列依存なので扱わない)
        let parts: Vec<&str> = lowered.split('+').map(str::trim).collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(format!("キーの書き方が不正: {s:?}"));
        }
        let (last, mods) = parts.split_last().expect("非空");
        let mut modifiers = Vec::new();
        for m in mods {
            let m = Modifier::parse(m).ok_or_else(|| {
                format!(
                    "未知の修飾キー {m:?} (in {s:?})。使える修飾キー: {}",
                    usable_modifier_names()
                )
            })?;
            if modifiers.contains(&m) {
                return Err(format!("修飾キーが重複: {s:?}"));
            }
            modifiers.push(m);
        }
        if Modifier::parse(last).is_some() {
            return Err(format!("修飾キー以外のキーが必要: {s:?}"));
        }
        let key = Key::parse(last).ok_or_else(|| {
            format!(
                "未知のキー {last:?} (in {s:?})。使える名前: {}",
                usable_key_names()
            )
        })?;
        modifiers.sort();
        Ok(KeyCombo { modifiers, key })
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for m in &self.modifiers {
            write!(f, "{}+", m.name())?;
        }
        f.write_str(&self.key.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    #[test]
    fn keys_normalize_aliases_case_and_order() {
        assert_eq!(combo("Shift+CONTROL+s").to_string(), "ctrl+shift+s");
        assert_eq!(combo("ctl + Return").to_string(), "ctrl+enter");
        assert_eq!(combo("option+escape").to_string(), "alt+esc");
        assert_eq!(combo("F12").to_string(), "f12");
        assert_eq!(combo("ctrl+shift+s"), combo("shift+ctrl+S"));
    }

    #[test]
    fn keys_reject_malformed() {
        for bad in [
            "",
            "ctrl+",
            "+a",
            "ctrl+ctrl+a",
            "ctrl",
            "hyper+a",
            "ctrl+foo",
            "f0",
            "f25",
            "f01",
        ] {
            assert!(bad.parse::<KeyCombo>().is_err(), "{bad}");
        }
    }

    #[test]
    fn keys_vk_mapping() {
        assert_eq!(combo("a").key.vk, 0x41);
        assert_eq!(combo("9").key.vk, 0x39);
        assert_eq!(combo("f1").key.vk, 0x70);
        assert_eq!(combo("f24").key.vk, 0x87);
        assert!(combo("left").key.is_extended());
        assert!(!combo("a").key.is_extended());
        assert_eq!(Modifier::Ctrl.vk(), 0x11);
    }

    #[test]
    fn keys_builtin_deny() {
        for denied in [
            "win+r",
            "super+l",
            "alt+f4",
            "shift+alt+f4",
            "ctrl+alt+delete",
            "ctrl+alt+t",
            "ctrl+shift+esc",
            "ctrl+esc",
        ] {
            assert!(combo(denied).builtin_denied().is_some(), "{denied}");
        }
        for ok in ["ctrl+s", "alt+tab", "shift+f4", "esc", "ctrl+shift+t"] {
            assert!(combo(ok).builtin_denied().is_none(), "{ok}");
        }
    }

    /// IME のキーの仮想キーコード。どれも拡張キーではない (D-131)
    #[test]
    fn keys_ime_names_map_to_virtual_keys() {
        for (name, vk) in [
            ("kanji", 0x19),
            ("convert", 0x1C),
            ("nonconvert", 0x1D),
            ("imeon", 0x16),
            ("imeoff", 0x1A),
        ] {
            let k = combo(name).key;
            assert_eq!(k.vk, vk, "{name}");
            assert!(!k.is_extended(), "{name}");
        }
    }

    /// IME のキーの別名は大文字まじりでも修飾キーつきでも正規名に戻る (D-131)
    #[test]
    fn keys_ime_aliases_normalize_to_canonical() {
        assert_eq!(combo("hankaku").to_string(), "kanji");
        assert_eq!(combo("zenkaku").to_string(), "kanji");
        assert_eq!(combo("Henkan").to_string(), "convert");
        assert_eq!(combo("MUHENKAN").to_string(), "nonconvert");
        assert_eq!(combo("KANJI").to_string(), "kanji");
        assert_eq!(combo("Shift+Hankaku").to_string(), "shift+kanji");
    }

    /// 表の行は正規名と `Key::name` で往復する (同じ vk の行が 2 つあると崩れる)
    #[test]
    fn keys_named_table_round_trips() {
        for (name, _, _) in NAMED_KEYS {
            assert_eq!(Key::parse(name).unwrap().name(), *name);
        }
    }

    /// 未知のキーの失敗の文に、表のすべての正規名と範囲が要素として出る (D-131)。
    /// 部分文字列では `convert` が `nonconvert` に隠れるので、要素で比べる
    #[test]
    fn keys_unknown_key_error_lists_usable_names() {
        let e = "ctrl+foo".parse::<KeyCombo>().unwrap_err();
        assert!(e.contains("未知のキー \"foo\""), "{e}");
        let (_, list) = e.split_once("使える名前: ").expect("一覧の見出し");
        let items: Vec<&str> = list.split(", ").collect();
        for want in ["a-z", "0-9", "f1-f24"] {
            assert!(items.contains(&want), "{want}: {e}");
        }
        for (name, _, _) in NAMED_KEYS {
            assert!(items.contains(name), "{name}: {e}");
        }
    }

    /// 未知の修飾キーの失敗の文に、使える修飾キーが正規の順で出る (D-131)
    #[test]
    fn keys_unknown_modifier_error_lists_modifiers() {
        let e = "hyper+a".parse::<KeyCombo>().unwrap_err();
        assert!(e.contains("未知の修飾キー \"hyper\""), "{e}");
        let (_, list) = e.split_once("使える修飾キー: ").expect("一覧の見出し");
        let items: Vec<&str> = list.split(", ").collect();
        assert_eq!(items, ["ctrl", "alt", "shift", "win"]);
    }

    /// 参照の文書の「キーの名前:」の行に、表のすべての正規名がバッククォートつきで出る (D-131)
    #[test]
    fn keys_reference_lists_named_keys() {
        const REFERENCE: &str = include_str!("../docs/reference/actions.md");
        let line = REFERENCE
            .lines()
            .find(|l| l.starts_with("キーの名前:"))
            .expect("「キーの名前:」の行がある");
        for (name, _, _) in NAMED_KEYS {
            assert!(line.contains(&format!("`{name}`")), "{name}: {line}");
        }
    }
}
