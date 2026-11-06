# 開発に参加するには

Issue と Pull Request を歓迎します。文書とコメントは日本語で書いていますが、Issue と PR は英語でも構いません。

- **脆弱性は Issue に書かず**、[SECURITY.md](SECURITY.md) の非公開の報告を使ってください
- 大きな変更は、先に Issue で方針を相談してもらえると助かります

## 安全の約束 (いちばん大事)

ai-cu は本物のマウスとキーボードを動かします。開発中に、自分や他人の環境を壊さないための約束です。

- `cargo test` の既定は実デスクトップに触れません。実入力と実際の撮影をするテストには `#[ignore]` を付け、`cargo test -- --ignored` で明示したときだけ走らせます。実入力を送ってよいのは、テストが自分で開くフィクスチャのウィンドウの上だけです
- 実入力のテストを走らせる間は、マウスとキーボードから手を離します。画面の縁に青い枠が出ます
- 任意の座標へのクリックや、任意のウィンドウへのキー送信を、実機で試すコードやテストを書かないでください
- 入力の論理は trait 越しにモックでテストします (`src/device.rs`、`src/testing.rs`)
- テストと動作の確認では、設定ファイルのパスを `target/` か一時ディレクトリに向けます。自分の実際の設定 (`%APPDATA%\ai-cu\config.toml`) を読み書きするテストを書かないでください
- スクリーンショットには個人の情報が写りえます。リポジトリにコミットしないでください (`.gitignore` で画像は無視しています)

## 開発の環境

- Windows 10 / 11 (x64)、Rust 1.95 以上 (stable、`x86_64-pc-windows-msvc`)、Visual Studio の C++ ビルドツール
- Python 3 (検査のスクリプトとフックのテストに使う。標準ライブラリだけ)

## 変更の前に

- [docs/design-notes.md](docs/design-notes.md) を通して読んでください。Windows の入力、端末、DPI、設定のパスに、踏みやすい落とし穴がまとめてあります
- 設計の判断は [docs/decisions.md](docs/decisions.md) に `D-xxx` の番号で記録しています。判断を変えるときは、新しいエントリを足し、古いエントリの「状態」を更新します (書式はファイルの冒頭)
- `unsafe` は `src/win32/` だけに置き、各 `unsafe` ブロックに `SAFETY:` のコメントを付けます。Win32 の API の約束は Microsoft Learn で確かめます

## 検査

PR の前に、次がすべて通ることを確かめてください。CI (GitHub Actions) でも同じものが走ります (実入力のテストは CI では走りません)。

```
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

まとめて走らせるなら `python scripts/verify.py` (ログは `target/verify/`)。入力、撮影、ウィンドウの扱いに触れる変更では、手元で `python scripts/verify.py --ignored` (実入力のテストを含む) も走らせてください。

実機で目で見て確かめる手順は [docs/manual-check/CHECKLIST.md](docs/manual-check/CHECKLIST.md) にあります。

## Pull Request

- 1 つの PR には 1 つのまとまった変更を入れます
- 振る舞いを変えたら、テストと README (使い方、書式、終了コード、安全機能) と、その細部を置いた [docs/reference/](docs/reference/) の文書を合わせて直します
- 終了コード (`ErrorKind`) を変えたら、README の表に加えて [docs/reference/exit-codes.md](docs/reference/exit-codes.md) の名前の節も直します (テストは README の表を見ますが、このファイルは見ません)
- 設定の書式、操作列 JSON の書式、終了コードを変えるときは、互換性への影響を PR に書きます
- コミットメッセージとファイルに、外の人が辿れない私的な参照 (個人の課題管理の番号、手元の絶対パスなど) を書かないでください

## ライセンス

送ってもらった変更は、このリポジトリと同じ MIT ライセンスで公開されます。
