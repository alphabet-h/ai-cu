# セキュリティポリシー

ai-cu は、AI に本物のマウスとキーボードを動かさせ、画面を撮らせる道具です。人が設定ファイルで決めた範囲 (allow / deny) を AI に越えさせないことが、このツールの安全の中心です。その守りを破れる不具合は、脆弱性として扱います。

> **English.** Please report vulnerabilities privately through GitHub:
> **Security → Report a vulnerability** (<https://github.com/alphabet-h/ai-cu/security/advisories/new>).
> Do not open a public issue. Reports in English or Japanese are welcome.

## 報告のしかた

**公開の Issue には書かないでください。** GitHub の非公開の報告を使います。

1. リポジトリの **Security** タブ → **Report a vulnerability** を開く (<https://github.com/alphabet-h/ai-cu/security/advisories/new>)
2. 次を書く (分かる範囲で構いません)
   - 何ができてしまうか (どの守りを、どう越えたか)
   - 再現の手順 (設定ファイル、操作列 JSON か MCP の呼び出し、Windows の版、ai-cu の版 `ai-cu --version`)
   - 期待した動き (終了コードや拒否のメッセージ) と、実際の動き
3. 画面のスクリーンショットを添えるときは、個人の情報が写っていないことを確かめてから添えてください

日本語でも英語でも受け付けます。

## 対応

- 個人で保守しているので、対応は可能な範囲で行います。目安として、1 週間以内に受け取ったことを返し、直すかどうかと見通しを伝えます
- 直したら、GitHub Security Advisory で公開します。希望があれば、報告者のお名前を謝辞に載せます
- 直す前に内容を公開するのは控えてください。公開の時期は相談して決めます

## 対象の版

修正は `main` の最新版にだけ入れます。古い版への取り込み (backport) はしません。

## 脆弱性として扱うもの

ai-cu の守り ([README の「安全機能」](README.md#安全機能)。限界を含む細部は [安全機能の詳細](docs/reference/safety.md)) を、設定や想定に反して越えられるもの。たとえば:

- 設定ファイルが無い、または `general.input_enabled` / `general.capture_enabled` / `capture.full_screen` が false なのに、入力や撮影ができる (既定で拒否する作りを越えられる)
- 設定の `deny` や、`allow` に無い対象 (操作の種類、プロセス、ウィンドウのクラス、画面の領域、キー) に、入力が届く・撮影できる・読める
- 追加の層 (`--config`、環境変数 `AI_CU_CONFIG`) で、人の設定 (基本の層) の `allow` を広げられる
- 組み込みの拒否 (`Win` や `Ctrl+Alt` を含む組み合わせ、`Alt+F4`、`Ctrl+Shift+Esc` など。設定では緩められないキー) を越えられる
- 緊急停止 (マウスの移動、目印のファイル、MCP の取り消し) が効かず、入力が送られ続ける
- フォアグラウンドガード、起動した側の除外、パスワード欄への入力の拒否、機密のウィンドウの撮影の拒否を越えられる
- 撮影の保存先の制限を越えて、許していない場所に書ける
- MCP サーバーとして、受け取った要求から任意のコマンドの実行や、許していないファイルの読み書きができる

## 範囲の外 (既知の限界)

次は README の「注意」と [安全機能の詳細](docs/reference/safety.md) (設定の規則の限界は [設定ファイルの詳細](docs/reference/config.md)) に限界として書いてあるもので、脆弱性としては扱いません (改善の提案は Issue で歓迎します)。

- シェルを持つ AI が、ai-cu を使わずに設定ファイルを書き換える、または別の手段で入力を送る (ai-cu の守りは ai-cu を通る操作に対するもの)
- 管理者権限のウィンドウ (UIPI) には入力が届かない前提の動き
- [安全機能の詳細](docs/reference/safety.md) と [設定ファイルの詳細](docs/reference/config.md) に限界として書いてあるもの (たとえば `capture.save_dirs` がジャンクションやシンボリックリンクをたどらないこと)
- 利用者が自分で広げた設定 (`allow` を空にする、`full_screen = true` にするなど) のとおりに動くこと
