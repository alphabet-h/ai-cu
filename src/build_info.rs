//! ビルド時に埋めた版の印 (D-069)。
//!
//! `build.rs` がビルド時に git を読んで `AI_CU_BUILD_META` (と `AI_CU_GIT_COMMIT`、`AI_CU_GIT_DIRTY`) を埋め、
//! ここで `VERSION` に組む。文法:
//!
//! ```text
//! <version> = <CARGO_PKG_VERSION> "+" <meta>
//! <meta>    = "g" <hex>{12} [ ".dirty" ] | "unknown"
//! ```
//!
//! - `<CARGO_PKG_VERSION>`: Cargo.toml の `version` (semver。`+` は含めない)
//! - `g<hex>{12}`: ビルド時の `git rev-parse HEAD` の先頭 12 桁
//! - `.dirty`: ビルド時に `git status --porcelain --untracked-files=normal` が空でなかった (未コミットの変更か、
//!   無視されていない未追跡のファイルがある木で作った)
//! - `unknown`: `rev-parse` か `status` が失敗した (git が無い、リポジトリでない)
//!
//! `ai-cu --version`、MCP の serverInfo の `version` (旧版の `initialize` の結果と現行版の `_meta` の両方)、
//! `ai-cu mcp` の起動ログの 3 か所が、この `VERSION` を出す。実行時に印を読んで判定するのは
//! `scripts/install.py` (clean な木で HEAD と一致する release だけを配置する) の 1 か所だけで、
//! Rust 側には読む関数を置かない。

/// 版の文字列 (`<CARGO_PKG_VERSION>+<meta>`)。`--version`、serverInfo、起動ログの 3 か所が出す
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("AI_CU_BUILD_META"));

/// ビルド時の `git rev-parse HEAD` (40 桁)。git を読めなければ空
#[cfg(test)]
const GIT_COMMIT: &str = env!("AI_CU_GIT_COMMIT");
/// ビルド時に作業ツリーが dirty なら `"1"`、clean なら `"0"`。git を読めなければ空
#[cfg(test)]
const GIT_DIRTY: &str = env!("AI_CU_GIT_DIRTY");

#[cfg(test)]
mod tests {
    use super::*;

    /// build.rs が埋めた 3 つの値の整合。unknown ⇔ commit が空 ⇔ dirty が空 (build.rs はどちらかが
    /// 失敗したら 3 つとも unknown 側にそろえる) なので、この 2 分岐で尽きる。
    /// このリポジトリは git 管理なので、コミットは 40 桁 (git の無い環境では落ちる)
    #[test]
    fn version_matches_commit_and_dirty_from_build_script() {
        let expected = if GIT_COMMIT.is_empty() {
            assert_eq!(GIT_DIRTY, "", "unknown なら dirty も空");
            format!("{}+unknown", env!("CARGO_PKG_VERSION"))
        } else {
            assert_eq!(GIT_COMMIT.len(), 40, "{GIT_COMMIT:?}");
            assert!(
                GIT_COMMIT
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{GIT_COMMIT:?}"
            );
            assert!(GIT_DIRTY == "1" || GIT_DIRTY == "0", "{GIT_DIRTY:?}");
            format!(
                "{}+g{}{}",
                env!("CARGO_PKG_VERSION"),
                &GIT_COMMIT[..12],
                if GIT_DIRTY == "1" { ".dirty" } else { "" }
            )
        };
        assert_eq!(VERSION, expected);
        assert_eq!(
            GIT_COMMIT.len(),
            40,
            "このリポジトリでは印が unknown にならない"
        );
    }
}
