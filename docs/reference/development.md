# 開発者向け (詳細)

README の「開発者向け」の詳細。[README に戻る](../../README.md#開発者向け)

文中の `D-xxx` は、[判断の記録](../decisions.md) の番号。

- 参加のしかた (安全の約束、検査、PR): CONTRIBUTING.md。脆弱性の報告: SECURITY.md
- 作業ルールと安全ルール (AI のエージェント向け): CLAUDE.md
- 設計判断の記録: docs/decisions.md
- 調べて分かった落とし穴 (Windows の入力、端末、設定のパス): docs/design-notes.md
- 構成: ライブラリ `ai_cu` (純粋な論理は trait 越しにモックでテストする) とバイナリ `ai-cu`。`unsafe` は `src/win32/` だけに置く (FFI の例外: 重い検査の枠を出す `scripts/verify_frame.py` は Python の ctypes で Win32 を呼ぶ。win32-reviewer のレビューの対象に入れる。D-111)
- 版の印: `build.rs` がビルド時に git (`rev-parse HEAD` と `status --porcelain`) を読んで `build_info::VERSION` に埋める。再実行の引き金は、ソースと文書の固定の一覧 (`build.rs`、`Cargo.toml`、`Cargo.lock`、`src`、`tests`、`scripts`、`docs`、`README.md`、`CLAUDE.md`) と git の `HEAD` / `index`、それに dirty の原因のパス。git が読めなければ `+unknown` に倒れる (D-069)
- 検証: `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` (実デスクトップに触れない)、`cargo build --release`。`cargo test` は `tests/policy_props.rs` で、追加の層が基本の層より緩められないこと (D-018、D-029) を proptest で任意の層の組について確かめる (D-071)。README と `src/guide.md` の終了コードの表、`--help` の終了コードの行は、`cargo test` が `ErrorKind` と突き合わせる (D-076)
- 検証をまとめて走らせる: `python scripts/verify.py`。既定で fmt、clippy、test、ガード hook のテスト、release を順に走らせる。`--quick` は fmt、clippy、test だけ、`--ignored` は実入力のテストを足す (主エージェントだけが走らせる)。ログは `target/verify/` に書き、件数 0 の成功と `test result:` の行の無いログを失敗にし、最初の失敗で止まる。終了コードは 0 (全部通った) / 1 (どれかが失敗か時間切れ) / 2 (引数の誤り)。Claude Code の Bash ツールから打つときは、`--quick` も含めて `run_in_background` で起動し、終わりの通知を待つ (制限時間は手順ごとなので、前景の上限 600 秒を超えうる)。`scripts/verify.py`、`scripts/verify_frame.py`、`.claude/skills/verify/SKILL.md` のどれかを変えたら `python scripts/test_verify.py` と `python scripts/test_verify_frame.py` も走らせる (D-076、D-111。`test_verify_frame.py` は `src/win32/overlay.rs` の枠の定数と写しを読み比べるので、その定数を変えたときも。実の窓を作るテストは `AI_CU_FRAME_REAL=1` のときだけ走り、主エージェントが打つ)
- verify.py の並列数と打ち直し (D-083): `--jobs N` (1 以上) は cargo の手順 (fmt、clippy、test、release、ignored) の子に `CARGO_BUILD_JOBS=N` を渡す。付けなければ cargo の既定のまま (呼び出し元の環境に `CARGO_BUILD_JOBS` があればそれが効く)。出力の頭の `jobs=` の行 (`--ignored` では枠の知らせの行 (D-111) の後) に、どれが効いたかを出す。通った手順は `target/verify/state.json` に記録し、`--continue` を付けると、同じ HEAD の clean な木で通った手順をそこから引き継ぎ (抜き出しを出し直し、判定は `ok (引き継ぎ)`)、残りだけを走らせる。記録が無い、壊れている、HEAD が違う、作業ツリーが clean でない、のように引き継げないときは、`引き継がない:` の行に理由を出して全部を走らせる。`--continue` は verify.py ごと外から止められた実行の打ち直しに使い、exit 1 (失敗か時間切れ) のあとは直して `--continue` なしで最初からやり直す
- 実機のテスト: `cargo test -- --ignored --nocapture --test-threads=1`。自前のフィクスチャウィンドウ (最前面) のクライアント領域の中だけに入力を送り、前面がフィクスチャでなければキーを送らずに失敗にする。テストの間は画面の縁に ai-cu の枠 (D-093) が出て、最初のフィクスチャを開く前に 3 秒待つ (始まりの知らせ。音は鳴らさず、その間の入力では止めない。枠の間に触れるとテストが落ちることがある。D-094)。予告の間 (D-093) の止まり方も実機のテスト 2 本が確かめる: 実行器に実の枠を渡して予告の間に入り、別のスレッドからフィクスチャの上でマウスを動かすと 6、Shift を打つと 10 (`lead_input`) で何も送らずに止まり、枠がすぐ消える (D-095。手動確認の手順 29 から人が触れる項目を外した)。`python scripts/verify.py --ignored` (`--continue` つきも) は、最初の段 (fmt) の前から最後の段の後まで (失敗で止まったときも)、別の枠 (`scripts/verify_frame.py` の子プロセス。一様な青の帯と、下の中央の札「ai-cu の重い検査中 — 実入力のテストの間はマウスとキーに触れない」。音は鳴らさない) を出し、`枠: 画面の縁に枠を出した。3000 ms 後に始める (D-111)` の行を出して 3 秒待ってから段に入る。出せなければ `枠: 出せない (<理由>)。枠なしで続ける` を出して待たずに続ける。この枠も撮影に写らず、クリックを素通しし、前面を取らない (D-111)
- 手動確認: `docs/manual-check/CHECKLIST.md`。本物の端末とメモ帳で、TUI、フォアグラウンドガード、緊急停止、deny、撮影を確かめる手順 (実施の記録は公開しない)
