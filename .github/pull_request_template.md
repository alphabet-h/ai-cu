## 何を変えたか

<!-- 変更の要点と理由。関係する Issue があれば番号 (#123) -->

## 確かめたこと

- [ ] `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`cargo build --release` が通る
- [ ] 入力、撮影、ウィンドウの扱いに触れる変更なら、手元で `python scripts/verify.py --ignored` (実入力のテスト) も通した
- [ ] 振る舞いを変えたなら、テストと README と docs/reference/ を直した
- [ ] 判断が要る変更なら、docs/decisions.md に `D-xxx` を足した
- [ ] 実デスクトップに触れるテストには `#[ignore]` を付け、入力はフィクスチャのウィンドウの上だけに送る

## 安全への影響

<!-- AI に許す範囲が変わるか。設定の書式、操作列 JSON の書式、終了コードの互換性への影響 -->
