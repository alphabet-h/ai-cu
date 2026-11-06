//! ビルド時に git を読んで、版の印を環境変数で埋める (D-069)。
//!
//! 読むのは `git rev-parse HEAD` (コミット) と `git status --porcelain -z --untracked-files=normal`
//! (作業ツリーに未コミットの変更か、無視されていない未追跡のファイルがあるか) の 2 つ。埋めるのは
//! `AI_CU_GIT_COMMIT` (40 桁か空)、`AI_CU_GIT_DIRTY` (`1` / `0` / 空)、`AI_CU_BUILD_META`
//! (`g<先頭 12 桁>[.dirty]` か `unknown`) の 3 つで、`src/build_info.rs` が
//! `VERSION` (`<CARGO_PKG_VERSION>+<AI_CU_BUILD_META>`) に組む。
//!
//! どちらかが読めなければ (git が無い、リポジトリでない)、読めたコミットも捨てて 3 つとも unknown 側に
//! そろえ (dirty かどうか分からないものを clean とは言わない。`scripts/install.py` は unknown を断る)、
//! `cargo::warning` を 1 行出す。
//!
//! 再実行の引き金 (`cargo::rerun-if-changed`) の考え方: cargo は引き金が 1 つも無いとパッケージ全体
//! (`.git` を含まない) を見るので、コミットしただけでは再実行されず、印が「古い HEAD + dirty」のまま残る。
//! だから、ソースと文書の固定の一覧に加えて、git ディレクトリの `HEAD` と `index` (この PC では
//! `git commit` が index も書き直す)、あるときだけ `packed-refs` と HEAD が指す loose な ref
//! (存在しないパスを書くと cargo が毎回再実行する)、それに dirty の原因の各パス (こちらは存在しない
//! (消した) パスも出す。cargo はそのパスが無い間は毎回再実行するので、固定の一覧に無い場所の dirty を
//! git を介さずに消した次の build で印が clean に戻る) を出す。`git` は `GIT_OPTIONAL_LOCKS=0` で呼び、
//! `status` が index を書き直して自分の引き金を引かないようにする。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 固定の引き金 (クレートの根からの相対パス)。ディレクトリは cargo が中を走査する
const FIXED_TRIGGERS: [&str; 9] = [
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
    "src",
    "tests",
    "scripts",
    "docs",
    "README.md",
    "CLAUDE.md",
];

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    for p in FIXED_TRIGGERS {
        rerun_if_changed(&root.join(p));
    }
    git_triggers(&root);

    let commit = commit(&root);
    let status = tree_status(&root);
    if let Some((_, paths)) = &status {
        for p in paths {
            rerun_if_changed(&root.join(p));
        }
    }
    let (commit, dirty, meta) = match (commit, status) {
        (Some(c), Some((dirty, _))) => {
            let meta = format!("g{}{}", &c[..12], if dirty { ".dirty" } else { "" });
            (c, if dirty { "1" } else { "0" }, meta)
        }
        _ => {
            println!(
                "cargo::warning=ai-cu: git のコミットか作業ツリーの状態を読めないので、版の印は +unknown になる (D-069)"
            );
            (String::new(), "", "unknown".to_string())
        }
    };
    println!("cargo::rustc-env=AI_CU_GIT_COMMIT={commit}");
    println!("cargo::rustc-env=AI_CU_GIT_DIRTY={dirty}");
    println!("cargo::rustc-env=AI_CU_BUILD_META={meta}");
}

fn rerun_if_changed(p: &Path) {
    println!("cargo::rerun-if-changed={}", p.display());
}

/// あるときだけ引き金にする (無いパスを書くと cargo が毎回再実行する)
fn rerun_if_file(p: &Path) {
    if p.is_file() {
        rerun_if_changed(p);
    }
}

/// `git -C <root> <args>` を呼び、終了コードが 0 なら標準出力の生のバイト列を返す。
/// `GIT_OPTIONAL_LOCKS=0` で、`status` が index を書き直さないようにする
fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// `git` の標準出力を、前後の空白を落とした文字列で返す
fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    git(root, args).map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

/// git ディレクトリの `HEAD` と `index`、あるときだけ `packed-refs` と HEAD が指す loose な ref を引き金にする
fn git_triggers(root: &Path) {
    let Some(dir) = git_text(root, &["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    let dir = PathBuf::from(dir);
    rerun_if_changed(&dir.join("HEAD"));
    rerun_if_changed(&dir.join("index"));
    rerun_if_file(&dir.join("packed-refs"));
    // detached HEAD では `HEAD` が返る (上で出している)
    if let Some(r) = git_text(root, &["rev-parse", "--symbolic-full-name", "HEAD"])
        && r.starts_with("refs/")
    {
        rerun_if_file(&dir.join(r));
    }
}

/// `git rev-parse HEAD` が 40 桁の小文字 16 進ならそれ
fn commit(root: &Path) -> Option<String> {
    let c = git_text(root, &["rev-parse", "HEAD"])?;
    (c.len() == 40 && c.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(c)
}

/// `git status --porcelain -z --untracked-files=normal` の結果 (dirty か、挙がった各パス)。`status` が失敗したら None。
/// dirty は生の出力が空でないこと (spec の定義) で、解析器 (`parse_status`) の読み落としで clean 側に倒れないようにする。
/// パス (リポジトリの根からの相対) は引き金に出すためだけに使う
fn tree_status(root: &Path) -> Option<(bool, Vec<String>)> {
    let out = git(
        root,
        &["status", "--porcelain", "-z", "--untracked-files=normal"],
    )?;
    Some((!out.is_empty(), parse_status(&out)))
}

/// `-z` の出力は NUL 区切りで、各項目は `XY<空白><パス>`。`X` か `Y` が `R` か `C` (改名・複製) の項目は、
/// 次の 1 つが元のパスなので項目として読まない (元のパスはもう無いので引き金にもしない)
fn parse_status(out: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut items = out.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let Some(item) = items.next() {
        if item.len() < 4 {
            continue;
        }
        paths.push(String::from_utf8_lossy(&item[3..]).into_owned());
        if matches!(item[0], b'R' | b'C') || matches!(item[1], b'R' | b'C') {
            let _ = items.next();
        }
    }
    paths
}
