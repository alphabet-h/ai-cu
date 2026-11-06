---
name: verify
description: ai-cu の変更が完了と言えるかを検証する。fmt、clippy、テスト、release ビルドを順に走らせ、各コマンドの終了コードとログの該当行を会話に残す。実装やリファクタの区切り、完了を主張する前、コミット前に使う。
---

# verify

「できた」と言う前の検証手順。**走らせて、結果を会話に残す**ところまでが仕事。
`/goal` の評価者は会話だけを見て判定する。コマンドも実行しないし、ファイルも読まない。会話に残っていない成功は、成功として扱われない。

## 前提

- **Bash ツールで実行する。** PowerShell 5.1 は、`2>&1` を付けると成功しても終了コードが偽の失敗になり、ログも壊れる (CLAUDE.local.md)
- **最後のソース編集より後に走らせる。** 途中の成功は証拠にならない。編集したら、最初からやり直す
- cargo は 1 回の呼び出しにつき 1 つ。出力はログに書いて、そのログを読む。ログは `target/verify/` に置く (git 管理外)
- 実入力のテスト (`--ignored`) を走らせてよいのは、主エージェントだけ。直列で、サブエージェントや並列からは走らせない (D-020)

## 既定: scripts/verify.py でまとめて走らせる (D-076)

下の「手順」の 0〜6 を `scripts/verify.py` が順に走らせ、件数 0 の成功、`test result:` の行の無いログ、終了コードを機械で判定する。最初の失敗で止まり、残りを「未実行」と出す。

1. 最終の検証と、実入力に触れる変更をしたとき (手順 0〜6)。走らせる前に、実入力のテストがフィクスチャウィンドウの外へ入力を送らないことを、コードで確かめる (CLAUDE.md の安全ルール)
   `python scripts/verify.py --ignored > target/verify-summary.log 2>&1; echo "exit=$?"`
2. それ以外の区切り (手順 0〜5)
   `python scripts/verify.py > target/verify-summary.log 2>&1; echo "exit=$?"`
3. 速い検査だけ (手順 0〜3。fmt、clippy、test)
   `python scripts/verify.py --quick > target/verify-summary.log 2>&1; echo "exit=$?"`

どれも Bash ツールの `run_in_background` で起動し、終わりの通知を待ってから `cat target/verify-summary.log` で全文を会話に出す (制限時間は手順ごとなので、前景の上限 600 秒を超えうる)。
出力には、手順ごとのコマンド、`exit=`、ログの該当行 (`test result:` の行は全部)、判定と、最後のまとめが入る。これが「報告」の中身になる。
終了コードは 0 (全部通った) / 1 (どれかが失敗か時間切れ) / 2 (引数の誤り)。1 なら、NG の手順のログを読んで原因を直し、最初からやり直す (手順 7)。
配置 (手順 8) は verify.py に入っていないので、通った後に別に打つ。

`scripts/verify.py` かこのファイルを変えたら、`python scripts/test_verify.py` も走らせる。このファイルの「手順」が正で、verify.py は写し。test_verify.py が、コマンド、ログの名前、抜き出しの字面のずれを両向きで見張る。

## 手順 (手で打つとき: verify.py が動かないとき、1 つの手順だけを打ち直すとき)

0. 準備
   `mkdir -p target/verify`
1. 整形の確認
   `cargo fmt --all -- --check > target/verify/fmt.log 2>&1; echo "exit=$?"`
   続けて `tail -n 15 target/verify/fmt.log`
2. 静的解析 (警告もエラー扱い)
   `cargo clippy --all-targets -- -D warnings > target/verify/clippy.log 2>&1; echo "exit=$?"`
   続けて `tail -n 15 target/verify/clippy.log`
3. テスト (既定のテストは実デスクトップに触れない)
   `cargo test > target/verify/test.log 2>&1; echo "exit=$?"`
   続けて `grep -E "test result|FAILED|panicked" target/verify/test.log`
   `test result:` の行は、ライブラリ、バイナリ、各結合テスト、ドキュメントテストごとに出る。出た行は省略せずに会話に残す。ビルドが失敗してログが空のときも、成功と取り違えない
4. ガード hook のテスト
   `python .claude/hooks/test_guard_real_input.py > target/verify/hook.log 2>&1; echo "exit=$?"`
   続けて `tail -n 5 target/verify/hook.log`
5. release ビルド
   `cargo build --release > target/verify/release.log 2>&1; echo "exit=$?"`
   続けて `tail -n 5 target/verify/release.log`
6. 実入力に触れる変更をしたとき、および最終の検証では、フィクスチャ限定の実入力テストを明示的に走らせる
   `cargo test -- --ignored --nocapture --test-threads=1 > target/verify/ignored.log 2>&1; echo "exit=$?"`
   続けて `grep -E "test result|FAILED|panicked|NOT_FOREGROUND|NO_INPUT_DESKTOP" target/verify/ignored.log`
   走らせる前に、そのテストがフィクスチャウィンドウの外へ入力を送らないことを、コードで確かめる (CLAUDE.md の安全ルール)。実際に走った件数が 0 の成功 (すべて飛ばした) は、失敗として扱う
7. 失敗したら、ログを読んで原因を直し、最初からやり直す。「一部だけ直ったはず」で済ませない
8. ここまでがすべて通り、コミットして作業ツリーが clean になったら、`cargo build --release` を打ち直してから、release を `~/.cargo/bin` に配置する (D-064、D-069。利用者が承認済み。install.py は未コミットの変更がある木と、HEAD と印が合わない release を断る)
   `PYTHONIOENCODING=utf-8 python scripts/install.py > target/verify/install.log 2>&1; echo "exit=$?"`
   続けて `cat target/verify/install.log`
   「配置した」の行を会話に残す。1 つでも失敗した手順がある release は配置しない。`scripts/install.py` を変えたときは、先に `python scripts/test_install.py` も走らせる

## 報告

各コマンドについて、次を会話に書く (verify.py を使ったときは、その出力の全文がこれに当たる)。

- 走らせたコマンドそのもの
- 終了コード
- ログの該当行 (テストなら `test result:` の行を省略せずに)

要約だけを書いて、出力を見せずに「通った」と言わない。走らせていないものは「未実行」と書く。
