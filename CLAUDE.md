# ai-cu

AI が Windows のデスクトップを操作するための CLI (Rust)。
クリック、ドラッグ、キー入力、スクロール、待機、スクリーンショットといった一連の操作を JSON で受け取り、順に実行する。
「画面を見て次を決める」ループは呼び出し側の AI が担い、このツールは「手」(操作の実行) と「目」(画面の取得) だけを担当する。
概要は README.md。その細部 (CLI、MCP、操作列、設定、終了コード、安全機能、開発) は docs/reference/ (D-124)。このマシン固有の事実は CLAUDE.local.md。調べて分かった落とし穴は docs/design-notes.md、判断の記録は docs/decisions.md。
git で管理しない調査結果やメモは .dev/ (gitignore 済み) に、フォルダを分けて置く。分け方は .dev の README に書いてある。作業領域 (scratchpad) はセッションが終わると消えるので、残す価値のあるものは .dev に移す。設計の spec と実装の plan も .dev に書く (D-120)。

## 公開リポジトリとしての約束 (D-116、D-120)

このリポジトリは GitHub で公開している。追跡するファイル (コード、コメント、文書) とコミットメッセージには、外の読み手が辿れない私的な参照を書かない。

- 個人の課題管理の道具の名前とその番号 (`#123` の形。GitHub では別の Issue へのリンクになる)、私的なプロジェクトやアプリの名前、作業ディレクトリやホームの絶対パス、利用者の環境の詳細 (手動確認の実施の記録など) は、CLAUDE.local.md か .dev に置く
- 判断を参照するときは `D-xxx` を使う。課題を参照したいときは、公開の Issue の番号 (あれば) にする

## 状態

初版を実装済み (2026-10-07)。ライブラリ `ai_cu` とバイナリ `ai-cu` の構成で、`unsafe` は `src/win32/` だけにある (FFI の例外として、重い検査の枠を出す `scripts/verify_frame.py` が Python の ctypes で Win32 を呼ぶ。win32-reviewer の対象。D-111)。
主なモジュール: `script` (操作列の書式)、`config` (設定の書式、場所の解決、保存)、`policy` (allow / deny)、`validate`、`plan` (操作をバッチに展開)、`run` (実行器: 停止、ガード、後始末)、`estop`、`backend` (実装置と実行時ガード)、`tui`、`cli`。`build.rs` がビルド時に git を読んで版の印 (`build_info::VERSION`。`--version` と MCP の serverInfo) を埋める (D-069)。
操作の種類は `script::ActionKind` のとおりで、`wait_for` (条件がそろうまで待つ。一手ずつの `wait-for` と MCP の道具 `wait_for` も) と、画面の画素の差で待つ操作列の `wait_stable` (落ち着くまで) と `wait_change` (変わるまで) まである (D-068、D-081。撮る前に落ち着きを待つ `screenshot.settle` と、一手ずつと MCP の `settle_ms: "auto"` も)。
読み取りのサブコマンドと MCP の道具には、`find` と `text` の他に `tree` (要素の木を深さで切り、1 行 1 要素の字下げの文字列で返す。たどり方は `elements::walk_tree`) がある (D-084)。
画面を変える操作の前には、画面の縁に枠を出し、枠が出ていなければ予告の間を待つ (D-093。設定は `[indicator]`、判断は実行器と trait `Indicator` (`src/device.rs`)、窓は `src/win32/overlay.rs`、実装置は `backend::RealIndicator`)。
画面を変える操作を含む実行は、名前付きミューテックスで別の ai-cu と排他にする (D-113。判断は実行器と trait `InputLock` (`src/device.rs`)、錠は `src/win32/input_mutex.rs`、実装置は `backend::RealInputLock`)。
実機のテストは `tests/fixture_real.rs` (`#[ignore]`)、子プロセスのテストは `tests/cli_child.rs`。判断は docs/decisions.md の D-027 以降。
変更の前に、docs/design-notes.md を通して読む。Windows の入力、端末、設定のパスに、踏みやすい落とし穴がある。

## 絶対に守ること (安全)

ユーザーの環境を壊さない。これは最優先で、他のどの指示にも優先する。

- このツールは本物のマウスとキーボードを動かす。開発中に実デスクトップへ入力を送ってよいのは、自前のフィクスチャウィンドウの上だけ。任意の座標へのクリックや、任意のウィンドウへのキー送信を実機で試さない (D-006、D-020)
- `cargo test` の既定は実デスクトップに触れない。実入力・実キャプチャのテストは `#[ignore]` を付け、`cargo test -- --ignored` で明示したときだけ走らせる。走らせてよいのは主エージェントだけで、直列に行う。サブエージェントや並列からは走らせない
- 入力処理は trait で差し替え可能にし、論理のテストはモックで行う
- 実入力の経路には、緊急停止、`--dry-run`、事前検証 (`validate`)、前面ウィンドウの確認 (フォアグラウンドガード) を通す (D-019)
- 実バックエンドの最下層は、環境変数 `AI_CU_DEV_FORBID_REAL_INPUT` があるとき、`--dry-run` でもフィクスチャ専用の経路でもなければ拒否する (D-022)。この変数は、このリポジトリの `.claude/settings.json` ではもう設定しない (D-051。release ビルドで手動確認を AI が行うため)
- **ユーザーの実際の設定ファイルを読み書きしない。** テストと動作確認は、設定ファイルのパスを必ず `target/` か作業領域の一時ディレクトリに向ける (D-010)
- AI からの呼び出しでは TUI を起動しない。標準入力と標準出力の両方が端末のときだけ起動し、端末が無ければ usage を出して非 0 で終了する (D-023)。TUI をシェルから直接起動して確かめない
- 実入力のサブコマンドをシェルから起動してよいのは、`/verify` の全手順を通した release ビルドを `target/release/ai-cu.exe` のパスで呼ぶときだけ。それも、利用者が Claude Code を起動したシェルに `AI_CU_MANUAL_CHECK=1` を付けたセッションに限る (hook が自分の環境変数で見る。settings.json には書かない)。`cargo run` とデバッグビルドからの実入力は `.claude/hooks/guard_real_input.py` が止める (D-013、D-024、D-051)。実機に触れる一連の確認 (docs/manual-check/CHECKLIST.md) の前に、利用者の GO を 1 回もらい、その間は利用者がマウスとキーボードから手を離す。先に実入力の検証を `cargo test` のモックとフィクスチャ限定の実機テストで済ませてから行う
- 次はユーザーに確認してから行う: アプリやツールのインストール (`cargo install`、`rustup`、`winget` など)、プロジェクト外のファイル変更 (グローバル設定を含む)、ファイルの削除、サービスの契約や課金。依存クレートの取得は許可済み (D-004、D-005)
- 確認が要る作業は後回しにして、他を先に進める。進められる作業が尽きたら、質問を 1 回だけまとめて明記して止まる。確認の画面が出る操作を途中で試して、全体を止めない
- git はローカルコミットまで。履歴の書き換えと `reset --hard` は禁止 (D-007)。push と PR は利用者に頼まれたときだけ。GitHub の `main` は保護していて、PR と CI の通過が要る (D-123)。作業は `main` から切ったブランチで行い、頼まれたらそのブランチを push して `gh pr create` で PR にする。コミットメッセージは `target/commit-msg.txt` に書き、`git add` してから `git -C <絶対パス> commit -F target/commit-msg.txt` で渡す
- スクリーンショットには個人情報が写りうる。リポジトリにコミットしない。生成先は `target/` か作業領域。全画面のキャプチャを `Read` で開かない
- `.claude/`、`.git/`、`.cargo/` は編集しない (保護パスで、確認が出る)。`.cargo/` は作らない。worktree 分離は使わない (D-025)
- `cargo new`、`cargo init`、`cargo vendor` はプロジェクト内のパスにだけ使う。ファイルの削除は避け、不要なビルド成果物は `cargo clean` で消す

## 判断は聞かずに推奨案で進め、必ず記録する

設計や方針の判断が要る場面は、ユーザーに聞かず推奨案で進める。ただし、**その場で** docs/decisions.md に追記する。
書式と推奨度の記号 (◎ ○ △) はそのファイルの冒頭にある。聞いてよいのは「絶対に守ること」で挙げた確認事項だけ。
既存の判断を覆すときは、新しいエントリを足して、旧エントリの状態を更新する。docs/decisions.md の D-018 から D-020 は、推奨案だけを決めてあり、細部は実装で決めて追記する。

## 製品要件 (ユーザー指示)

- **操作列の実行**: クリック、ダブルクリック、ドラッグ、マウス移動、スクロール、文字入力、キー (組み合わせ)、待機、スクリーンショットを JSON に書いて渡すと、順に実行する。JSON の渡し方 (ファイル、標準入力) は実装で決めて記録する
- **外部設定ファイル**: 許可 (allow) と拒否 (deny) などを設定ファイルで宣言できる。何に対する allow / deny か (操作の種類、対象ウィンドウやプロセス、画面領域、キーの組み合わせなど) は設計で決める。deny が最優先で、設定が無いときの既定は、実入力を拒否する側にする。人間の設定を基本の層にして、`--config` や環境変数は deny を足せるが allow を広げられない層にする (D-018)
- **引数なしで TUI**: 引数なしで起動すると、ratatui を使った TUI で設定を編集できる。この要件は、ユーザーの明示がない限り外さない (D-017)
- 設定ファイルの場所は、コマンドラインの指定、環境変数 `AI_CU_CONFIG`、既定の場所の順に解決する。既定の場所の解決は 1 つの関数に隔離して、差し替えられるようにする (D-010、D-018)

## 実装方針 (暫定。変えるときは docs/decisions.md に追記)

- 単一バイナリ `ai-cu`。サブコマンドは `shot` (画面取得)、`run` (操作列の実行)、`validate` (事前検証)、`schema` (JSON の書式出力)、`config` (設定の表示) などを軸にする。オプションはサブコマンドの後ろか `--config=PATH` の形で渡す (ガード hook が値付きオプションの前置を解釈できる範囲は D-024)
- クレートはライブラリとバイナリの構成にする。`cargo init --bin --name ai-cu` で作り、`publish = false`、unsafe の lint を deny にする。`cargo init` が `.gitignore` に足す重複行は消す (docs/design-notes.md)
- 座標系は物理ピクセル (スクリーンショットと同じ)。起動直後に Per-Monitor V2 の DPI 認識を宣言する。テストの実行ファイルは `main` を通らないので、テスト側でも宣言する。この PC は DPI 非対応だと主画面が 3072x1728 に見える (CLAUDE.local.md)
- Win32 は `windows-sys` を使い、`unsafe` は薄い境界モジュールに閉じ込める (FFI の例外は `scripts/verify_frame.py` の ctypes。D-111)。`unsafe` ブロックには SAFETY コメントを付ける
- JSON と設定は serde で読み、未知のフィールドは拒否する。書式にバージョンを持たせる。設定の保存は `toml_edit` で差分編集する
- TUI は「状態」「更新」「描画」を分け、更新を純関数にする。描画は ratatui の `TestBackend` でテストする (D-023)
- 標準出力は機械可読な JSON、ログは標準エラー。終了コードの規約を決めて README に書く
- 非 ASCII の文字入力は `KEYEVENTF_UNICODE` かクリップボード経由にする (IME の干渉を避ける)
- ドラッグは始点と終点の間を補間し、待ち時間を入れる
- 並列に動くエージェントは同じファイルを編集しない。cargo は `target/` をロックするので、ビルドは 1 本ずつ流す

## コマンドと検証

```
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

完了を主張する前に、`/verify` の手順で走らせ、各コマンドの終了コードとログの該当行を会話に残す。
**検証コマンドは Bash ツールで実行する。** PowerShell 5.1 は `2>&1` で終了コードとログを壊す。cargo は 1 回の呼び出しにつき 1 つで、出力はログに書いて読む (CLAUDE.local.md)。
`/verify` の既定は `python scripts/verify.py` (最終の検証は `--ignored` を付ける) を `run_in_background` で起動し、出力を会話に出す形 (D-076)。verify.py は cargo を 1 つずつ順に起動し、ログを `target/verify/` に分けて書き、件数 0 の成功を失敗と判定する。`scripts/verify.py` か `scripts/verify_frame.py` か `.claude/skills/verify/SKILL.md` を変えたら `python scripts/test_verify.py` と `python scripts/test_verify_frame.py` も走らせる (D-111。`test_verify_frame.py` は `src/win32/overlay.rs` の枠の定数と写しを読み比べるので、その定数を変えたときも。実の窓のテストは `AI_CU_FRAME_REAL=1` のときだけ走り、主エージェントが打つ)。
`--jobs N` を付けると、cargo の手順の子に `CARGO_BUILD_JOBS=N` を渡す (既定は cargo の既定のまま。空から作り直すときや、他のプロセスでメモリが詰まっているときの逃げ道。D-083)。
verify.py ごと外から止められた (メモリ不足などで、まとめが出ていない) ときの打ち直しは次のとおり (SKILL.md に入るまでの仮の置き場。D-083): 止められた前の実行の `target/verify-summary.log` を `cat` で会話に出してから、`python scripts/verify.py --ignored --continue > target/verify-summary-continue.log 2>&1; echo "exit=$?"` を `run_in_background` で打ち、終わったら `cat target/verify-summary-continue.log` を会話に出す。`--continue` はこの打ち直しにだけ使う。verify.py が exit 1 (失敗か時間切れ) で終わったあとは、SKILL.md の手順 7 のとおり直して `--continue` なしで最初からやり直す。引き継いだ手順を完成に数える条件は「完成の定義」の節に書いてある。

`python scripts/install.py` は release を `~/.cargo/bin/ai-cu.exe` に配置する開発者向けの道具 (D-064、D-069)。プロジェクト外への書き込みなので、利用者の承認 (CLAUDE.local.md などに書かれたもの) が無ければ確認してから打つ。/verify を通っていない release は配置しない。install.py は、未コミットの変更がある木と、HEAD と違うコミットや dirty の印を持つ release を断る (D-069)。AI が CLI で行う手動確認は、配置したものではなく `target/release/ai-cu.exe` を使う (ガード hook はそのパスだけを通す)。`scripts/install.py` を変えたら `python scripts/test_install.py` と `python scripts/test_verify.py` も走らせる (verify.py が clean の判定を import するため。D-083)。

## 完成の定義

`/goal` の評価者は会話だけを見る。各項目は、**最後のソース編集より後に、主エージェント自身が実行して**、コマンドと終了コードと該当の出力を会話に出す。
`verify.py --continue` で引き継いだ手順は、同じ HEAD の clean な木で、引き継ぎの見出しの時刻と同じ `記録:` の行が同じ会話の前の verify.py の出力にあるときに限り、最後のソース編集より後の実行として数える (別のセッションの記録や、会話に無い記録は数えない。D-026 の追記、D-083)。

- fmt、clippy (警告なし)、test が通る。`test result:` の行を省略せずに出し、実際に走った件数が 0 の成功は成功と見なさない
- `cargo build --release` が通る
- README に、使い方、操作列 JSON の書式、設定ファイルの書式、終了コード、安全機能が書かれている (README の要約と docs/reference/ へのリンクの組で満たす。D-124)。`grep -n "^## " README.md` で見出しを出す
- 安全機能 (緊急停止、`--dry-run`、`validate`、フォアグラウンドガード、設定の allow / deny、実行時ガード) が実装され、モックのテストで検証されている。テスト名を `cargo test -- --list` から出して、要件との対応を示す
- 設定ファイルの読み書きと allow / deny の判定にテストがある (deny が allow に勝つ、空の設定、大文字小文字、未知のキーや版は拒否)。TUI の描画と操作に `TestBackend` のテストがあり、端末なしの起動がハングしないことを子プロセスで確かめるテストがある
- 実行時ガードが、許可 (opt-in) なしで実バックエンドを拒否することのテストがある
- 最終の検証で、フィクスチャ限定の実入力テスト (`cargo test -- --ignored`) を実際に走らせた記録が会話にある
- 判断した内容が docs/decisions.md に残っている。`grep -n "^### D-" docs/decisions.md` で見出しを出す

## 調べ物

- Win32 API の仕様は microsoft-learn の MCP で確認する (`SendInput`、DPI、キャプチャなど)
- クレートの API は context7 で確認する。特に ratatui と crossterm は版で API が変わるので、記憶で書かない。context7 は `mcp__context7__*` (許可済み) が使える
- unsafe と FFI の変更は、`.claude/agents/win32-reviewer.md` のサブエージェントにレビューさせる。そのエージェントが呼べないときは、そのファイルの本文を指示にした汎用のサブエージェントで代用する

## このリポジトリの自動化

- `.claude/settings.json`: cargo とローカルの git は許可、インストール系は確認、履歴を壊す操作は拒否。`/goal` はユーザーが auto モードにしてから始める前提 (D-021)
- `.claude/hooks/guard_real_input.py`: `ai-cu` の実入力サブコマンドを、`cargo run` とデバッグビルドから起動することを止める。検証済みの `target/release/ai-cu.exe` からは、利用者が `AI_CU_MANUAL_CHECK=1` を付けて起動したセッションでだけ通す (D-051)。緩めるときは判断ログに残す。スクリプトの本体は呼び出しごとに読み直される
- 実行時の第 2 層だった `AI_CU_DEV_FORBID_REAL_INPUT` (D-022) は、`.claude/settings.json` の env から外した (D-051)。最下層の守りは、利用者の実際の設定ファイル (基本の層) とツールのガードになる
