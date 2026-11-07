//! TUI の配色 (D-129)。役割 (`Theme` のスロット) ごとに色を持つ表と、設定の値と環境からテーマを選ぶ純関数。
//! 環境の読み取りは `ThemeEnv::detect` だけが行い (呼び出し側の `tui::run` が 1 回呼ぶ)、
//! `resolve` と `fell_back` は値を受け取るだけにする (テストが環境に依らない)。
//!
//! 色の値と役割は D-129 (docs/decisions.md) のとおり。`calm` と `calm-light` の比は WCAG 2 で、
//! 暗い背景 (#0C0C0C、#1E2228、選択の面 #2A3442) と明るい背景 (白、#F5F7FA) に対して `tests` が確かめる。

use ratatui::style::Color;

use crate::config::ThemeName;

/// 役割ごとの色。選んでいる行は色を持たず、`▶` と REVERSED で示す。
/// `mono` のように色を持たないテーマは、すべて `Color::Reset` にして、BOLD、REVERSED、枠の太さ、記号で区別する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// この表が表すテーマ。`Auto` は `resolve` で解決済みなので、ここには現れない
    pub name: ThemeName,
    /// 本文、節の見出し
    pub text: Color,
    /// 補足、キー名、基本の層の札
    pub muted: Color,
    /// 前面でないペインの枠 (飾りなので比は問わない)
    pub border: Color,
    /// 前面のペインと入力欄の枠
    pub focus: Color,
    /// 現在の値、追加の操作、Help のダイアログ、allow
    pub accent: Color,
    /// 変更の印 ●
    pub changed: Color,
    /// ✓、無効 (安全側)、成功
    pub ok: Color,
    /// ⚠、確認と終了のダイアログ、追加の層の札
    pub warn: Color,
    /// ✗、実入力の有効、失敗、deny
    pub danger: Color,
}

/// 暗い背景用 (既定)。名前付きの色でなく RGB を使うので、VT が使える端末が要る
pub const CALM: Theme = Theme {
    name: ThemeName::Calm,
    text: Color::from_u32(0x00DADEE5),
    muted: Color::from_u32(0x00979FAB),
    border: Color::from_u32(0x00454D5B),
    focus: Color::from_u32(0x004F84DF),
    accent: Color::from_u32(0x008DB2F1),
    changed: Color::from_u32(0x00B6A98F),
    ok: Color::from_u32(0x0093C69D),
    warn: Color::from_u32(0x00DBB87E),
    danger: Color::from_u32(0x00DC8C85),
};

/// 明るい背景用。`CALM` と同じく RGB
pub const CALM_LIGHT: Theme = Theme {
    name: ThemeName::CalmLight,
    text: Color::from_u32(0x002A2E36),
    muted: Color::from_u32(0x005D646F),
    border: Color::from_u32(0x00B6BECB),
    focus: Color::from_u32(0x002963C9),
    accent: Color::from_u32(0x002B5AAB),
    changed: Color::from_u32(0x0075603B),
    ok: Color::from_u32(0x00346C42),
    warn: Color::from_u32(0x00845922),
    danger: Color::from_u32(0x00A03F3C),
};

/// 名前付きの 16 色。端末の配色に従う。VT なしの conhost でも出る
pub const ANSI: Theme = Theme {
    name: ThemeName::Ansi,
    text: Color::Reset,
    muted: Color::DarkGray,
    border: Color::DarkGray,
    focus: Color::Cyan,
    accent: Color::Cyan,
    changed: Color::Yellow,
    ok: Color::Green,
    warn: Color::Yellow,
    danger: Color::Red,
};

/// 色なし。区別は BOLD、REVERSED、枠の太さ、記号 (✓ ✗ ⚠ ● ▶) に任せる
pub const MONO: Theme = Theme {
    name: ThemeName::Mono,
    text: Color::Reset,
    muted: Color::Reset,
    border: Color::Reset,
    focus: Color::Reset,
    accent: Color::Reset,
    changed: Color::Reset,
    ok: Color::Reset,
    warn: Color::Reset,
    danger: Color::Reset,
};

/// テーマの選び方が頼る環境。読み取りは `detect` に隔離し、`resolve` へは値で渡す
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeEnv {
    /// 環境変数 `NO_COLOR` が空でない (no-color.org)
    pub no_color: bool,
    /// 端末が VT のエスケープ (RGB の色を含む) を処理できる。Windows 以外は常に true
    pub vt: bool,
}

impl Default for ThemeEnv {
    /// 色が使えて VT もある環境 (テストの既定)
    fn default() -> Self {
        ThemeEnv {
            no_color: false,
            vt: true,
        }
    }
}

impl ThemeEnv {
    /// 今の環境を読む。`NO_COLOR` は空でないときだけ数える。
    /// Windows では `crossterm::ansi_support::supports_ansi()` で VT の可否を見る
    /// (標準出力のコンソールで VT の処理を有効にしてみる副作用がある。`COLORTERM` は見ない。D-129)。
    /// Windows 以外では VT を常に使えるものとする
    pub fn detect() -> ThemeEnv {
        let no_color = no_color_from(std::env::var_os("NO_COLOR").as_deref());
        #[cfg(windows)]
        let vt = ratatui::crossterm::ansi_support::supports_ansi();
        #[cfg(not(windows))]
        let vt = true;
        ThemeEnv { no_color, vt }
    }
}

/// `NO_COLOR` の値から「色を使わない」かを決める。未設定と空文字列は数えない
fn no_color_from(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// 設定の値と環境から、使うテーマを選ぶ。
///
/// - `auto` 以外は、それを使う (`NO_COLOR` より優先。no-color.org の約束どおり)。
///   ただし RGB のテーマ (`calm`、`calm-light`) は、VT が使えない端末では `ansi` に落とす
/// - `auto` は、`NO_COLOR` が空でなければ `mono`、VT が使えなければ `ansi`、それ以外は `calm`
pub fn resolve(name: ThemeName, env: ThemeEnv) -> &'static Theme {
    match name {
        ThemeName::Auto => {
            if env.no_color {
                &MONO
            } else if !env.vt {
                &ANSI
            } else {
                &CALM
            }
        }
        ThemeName::Calm if env.vt => &CALM,
        ThemeName::CalmLight if env.vt => &CALM_LIGHT,
        ThemeName::Calm | ThemeName::CalmLight | ThemeName::Ansi => &ANSI,
        ThemeName::Mono => &MONO,
    }
}

/// RGB のテーマを明示したのに、VT が使えず `ansi` に落ちたとき true (利用者へ知らせるため)。
/// `auto` が `ansi` になった場合は、利用者が選んだ色ではないので false
pub fn fell_back(name: ThemeName, env: ThemeEnv) -> bool {
    matches!(name, ThemeName::Calm | ThemeName::CalmLight) && !env.vt
}

/// sRGB の 1 チャンネル (0 から 255) を線形の値にする (WCAG 2 の相対輝度の式)
#[cfg(test)]
fn linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 2 の相対輝度
#[cfg(test)]
fn luminance(r: u8, g: u8, b: u8) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// 2 色の WCAG 2 のコントラスト比 (1 から 21)。両方が `Color::Rgb` のときだけ測れる。
/// テーマの表が基準を満たすことを確かめるための道具で、描画では使わない
#[cfg(test)]
pub(crate) fn contrast(a: Color, b: Color) -> Option<f64> {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return None;
    };
    let (la, lb) = (luminance(ar, ag, ab), luminance(br, bg, bb));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(no_color: bool, vt: bool) -> ThemeEnv {
        ThemeEnv { no_color, vt }
    }

    #[test]
    fn resolve_auto_follows_env() {
        assert_eq!(
            resolve(ThemeName::Auto, env(false, true)).name,
            ThemeName::Calm
        );
        assert_eq!(
            resolve(ThemeName::Auto, env(true, true)).name,
            ThemeName::Mono
        );
        assert_eq!(
            resolve(ThemeName::Auto, env(false, false)).name,
            ThemeName::Ansi
        );
        assert_eq!(
            resolve(ThemeName::Auto, env(true, false)).name,
            ThemeName::Mono
        );
    }

    #[test]
    fn explicit_theme_beats_no_color() {
        assert_eq!(
            resolve(ThemeName::Calm, env(true, true)).name,
            ThemeName::Calm
        );
        assert_eq!(
            resolve(ThemeName::CalmLight, env(true, true)).name,
            ThemeName::CalmLight
        );
        assert_eq!(
            resolve(ThemeName::Ansi, env(true, true)).name,
            ThemeName::Ansi
        );
    }

    #[test]
    fn rgb_theme_falls_back_without_vt() {
        let no_vt = env(false, false);
        assert_eq!(resolve(ThemeName::Calm, no_vt).name, ThemeName::Ansi);
        assert!(fell_back(ThemeName::Calm, no_vt));
        assert_eq!(resolve(ThemeName::CalmLight, no_vt).name, ThemeName::Ansi);
        assert!(fell_back(ThemeName::CalmLight, no_vt));
        assert_eq!(resolve(ThemeName::Mono, no_vt).name, ThemeName::Mono);
        assert!(!fell_back(ThemeName::Mono, no_vt));
        assert!(!fell_back(ThemeName::Ansi, no_vt));
        assert!(!fell_back(ThemeName::Auto, no_vt));
        assert!(!fell_back(ThemeName::Calm, env(false, true)));
    }

    #[test]
    fn theme_name_matches_table() {
        assert_eq!(CALM.name, ThemeName::Calm);
        assert_eq!(CALM_LIGHT.name, ThemeName::CalmLight);
        assert_eq!(ANSI.name, ThemeName::Ansi);
        assert_eq!(MONO.name, ThemeName::Mono);
    }

    #[test]
    fn calm_text_roles_meet_contrast() {
        // 暗い側は、端末の地 2 つと選択の面 (REVERSED でなく面で選ぶ端末の配色を想定)
        let dark_bgs = [
            Color::Rgb(0x0C, 0x0C, 0x0C),
            Color::Rgb(0x1E, 0x22, 0x28),
            Color::Rgb(0x2A, 0x34, 0x42),
        ];
        let light_bgs = [Color::Rgb(0xFF, 0xFF, 0xFF), Color::Rgb(0xF5, 0xF7, 0xFA)];
        for (theme, bgs) in [(&CALM, &dark_bgs[..]), (&CALM_LIGHT, &light_bgs[..])] {
            for &bg in bgs {
                let roles = [
                    ("text", theme.text),
                    ("muted", theme.muted),
                    ("accent", theme.accent),
                    ("changed", theme.changed),
                    ("ok", theme.ok),
                    ("warn", theme.warn),
                    ("danger", theme.danger),
                ];
                for (role, fg) in roles {
                    let c = contrast(fg, bg).expect("both Rgb");
                    assert!(c >= 4.5, "{:?} {role} on {bg:?}: {c:.2}", theme.name);
                }
                let c = contrast(theme.focus, bg).expect("both Rgb");
                assert!(c >= 3.0, "{:?} focus on {bg:?}: {c:.2}", theme.name);
            }
        }
    }

    #[test]
    fn contrast_matches_known_value() {
        let c = contrast(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255)).unwrap();
        assert!((c - 21.0).abs() < 0.01, "{c}");
        // 順序に依らない
        let r = contrast(Color::Rgb(255, 255, 255), Color::Rgb(0, 0, 0)).unwrap();
        assert!((r - 21.0).abs() < 0.01, "{r}");
        // 同じ色は 1
        let same = contrast(Color::Rgb(0x12, 0x34, 0x56), Color::Rgb(0x12, 0x34, 0x56)).unwrap();
        assert!((same - 1.0).abs() < 1e-9, "{same}");
    }

    /// 配色を選んだときに別の計算で出しておいた比と、`contrast` が合う
    #[test]
    fn contrast_matches_spec_ratios() {
        let near = |a: Color, b: Color, want: f64| {
            let got = contrast(a, b).unwrap();
            assert!(
                (got - want).abs() < 0.02,
                "{a:?} on {b:?}: {got:.3} != {want}"
            );
        };
        let black = Color::Rgb(0x0C, 0x0C, 0x0C);
        let slate = Color::Rgb(0x1E, 0x22, 0x28);
        near(CALM.text, black, 14.50);
        near(CALM.text, slate, 11.84);
        near(CALM.focus, slate, 4.33);
        near(CALM.danger, black, 7.58);
        let white = Color::Rgb(0xFF, 0xFF, 0xFF);
        let mist = Color::Rgb(0xF5, 0xF7, 0xFA);
        near(CALM_LIGHT.text, white, 13.61);
        near(CALM_LIGHT.danger, mist, 5.98);
    }

    #[test]
    fn contrast_is_none_for_non_rgb() {
        assert_eq!(contrast(Color::Reset, Color::Rgb(0, 0, 0)), None);
        assert_eq!(contrast(Color::Rgb(0, 0, 0), Color::Cyan), None);
        assert_eq!(contrast(Color::Indexed(1), Color::Rgb(0, 0, 0)), None);
    }

    #[test]
    fn mono_uses_no_color() {
        let m = &MONO;
        for c in [
            m.text, m.muted, m.border, m.focus, m.accent, m.changed, m.ok, m.warn, m.danger,
        ] {
            assert_eq!(c, Color::Reset);
        }
    }

    #[test]
    fn ansi_uses_named_colors_only() {
        let a = &ANSI;
        for c in [
            a.text, a.muted, a.border, a.focus, a.accent, a.changed, a.ok, a.warn, a.danger,
        ] {
            assert!(
                !matches!(c, Color::Rgb(..) | Color::Indexed(..)),
                "ansi に Rgb か Indexed が混じっている: {c:?}"
            );
        }
    }

    #[test]
    fn ansi_roles_match_spec() {
        let a = &ANSI;
        assert_eq!(a.text, Color::Reset);
        assert_eq!(a.muted, Color::DarkGray);
        assert_eq!(a.border, Color::DarkGray);
        assert_eq!(a.focus, Color::Cyan);
        assert_eq!(a.accent, Color::Cyan);
        assert_eq!(a.changed, Color::Yellow);
        assert_eq!(a.ok, Color::Green);
        assert_eq!(a.warn, Color::Yellow);
        assert_eq!(a.danger, Color::Red);
    }

    #[test]
    fn rgb_tables_match_spec() {
        assert_eq!(CALM.text, Color::from_u32(0x00DADEE5));
        assert_eq!(CALM.muted, Color::from_u32(0x00979FAB));
        assert_eq!(CALM.border, Color::from_u32(0x00454D5B));
        assert_eq!(CALM.focus, Color::from_u32(0x004F84DF));
        assert_eq!(CALM.accent, Color::from_u32(0x008DB2F1));
        assert_eq!(CALM.changed, Color::from_u32(0x00B6A98F));
        assert_eq!(CALM.ok, Color::from_u32(0x0093C69D));
        assert_eq!(CALM.warn, Color::from_u32(0x00DBB87E));
        assert_eq!(CALM.danger, Color::from_u32(0x00DC8C85));
        assert_eq!(CALM_LIGHT.text, Color::from_u32(0x002A2E36));
        assert_eq!(CALM_LIGHT.muted, Color::from_u32(0x005D646F));
        assert_eq!(CALM_LIGHT.border, Color::from_u32(0x00B6BECB));
        assert_eq!(CALM_LIGHT.focus, Color::from_u32(0x002963C9));
        assert_eq!(CALM_LIGHT.accent, Color::from_u32(0x002B5AAB));
        assert_eq!(CALM_LIGHT.changed, Color::from_u32(0x0075603B));
        assert_eq!(CALM_LIGHT.ok, Color::from_u32(0x00346C42));
        assert_eq!(CALM_LIGHT.warn, Color::from_u32(0x00845922));
        assert_eq!(CALM_LIGHT.danger, Color::from_u32(0x00A03F3C));
    }

    #[test]
    fn no_color_counts_only_when_non_empty() {
        use std::ffi::OsStr;
        assert!(!no_color_from(None));
        assert!(!no_color_from(Some(OsStr::new(""))));
        assert!(no_color_from(Some(OsStr::new("1"))));
        assert!(no_color_from(Some(OsStr::new("0"))));
    }

    #[test]
    fn default_env_is_color_with_vt() {
        assert_eq!(ThemeEnv::default(), env(false, true));
    }
}
