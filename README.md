# ai-cu

**AI がコンピュータ (Windows のデスクトップ) を操作するための CLI。**

[![CI](https://github.com/alphabet-h/ai-cu/actions/workflows/ci.yml/badge.svg)](https://github.com/alphabet-h/ai-cu/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6)

クリック、ドラッグ、キー入力、スクロール、待機、スクリーンショットといった一連の操作を JSON に書いて渡すと、その順に実行する。
Codex の computer use のような「スクリーンショットを見て次の操作を決めるループ」は、呼び出し側の AI (Claude Code など) が担当する。
このツールが担当するのは、そのループの「手」(操作の実行) と「目」(画面の取得) だけ。

本物のマウスとキーボードを動かすので、既定ではすべての実入力と撮影を拒否する。人間が設定ファイルで許可した範囲でだけ動く ([安全機能](#安全機能))。

[導入](#導入) · [はじめかた](#はじめかた) · [使い方](#使い方) · [MCP](#mcp-サーバーモード) · [操作列 JSON](#操作列-json) · [設定ファイル](#設定ファイル) · [終了コード](#終了コード) · [安全機能](#安全機能)

> **English summary.** ai-cu is a Windows CLI and MCP server that lets an AI agent operate the desktop:
> click, double-click, drag, move, scroll, type, press keys, wait, and take screenshots, either one step at a time
> or as a JSON action list. The agent decides what to do by looking at screenshots; ai-cu only acts ("hands")
> and observes ("eyes"). Real input and capture are denied by default. A human-owned config file declares what the
> AI may touch (actions, processes, window classes, screen regions, keys); deny always wins, and layers added by the
> caller can only narrow it. The documentation is in Japanese.

## 注意

> [!WARNING]
> - 本物のマウスとキーボードを動かす。AI に任せた操作で、誤った送信、データの消失、意図しないアプリの操作が起こりうる
> - 無保証 (MIT ライセンスの「AS IS」。[LICENSE](LICENSE) を参照)。自分の責任で使う。最初は、許可するアプリ (`allow.processes`) と画面の領域 (`allow.regions`) を狭くして試す
> - 人と AI の境界は、人が持つ設定ファイル。AI がシェルから設定ファイルを書き換えられる環境では、境界にならない ([安全機能](#安全機能) の「設定ファイルの守り」)
>
> 各安全機能の限界は [docs/reference/safety.md](docs/reference/safety.md)。

## 導入

要件:

- Windows 10 / 11 (x64)。管理者権限は要らない (管理者権限で動くウィンドウには入力を送らない)
- ビルドに Rust 1.95 以上 (stable、`x86_64-pc-windows-msvc`) と、Visual Studio の C++ ビルドツール

```sh
cargo install --git https://github.com/alphabet-h/ai-cu --locked
```

`~/.cargo/bin/ai-cu.exe` に入る。手元に clone したなら `cargo install --path . --locked`。

## はじめかた

1. 端末で `ai-cu` を引数なしで起動すると、TUI で設定ファイル (既定は `%APPDATA%\ai-cu\config.toml`) を作って編集できる
   - 新しく作った設定は、入力も撮影も無効で、端末とシェル、管理ツール、パスワード管理ソフトを deny に並べてある
   - 使うアプリを `allow.processes` に足し、`input_enabled` と `capture_enabled` を有効にする
2. `ai-cu guide --setup` が、MCP の登録 (Claude Code、Codex) と、Claude Code の権限の文例を、この実行ファイルのパスと設定の場所から組んで出す
3. AI には、最初に `ai-cu guide` (MCP なら道具の `guide`) を読ませる

## 使い方

| コマンド | 何をするか |
|---|---|
| `ai-cu guide [--full \| --setup]` | AI 向けの手引き (既定は要点版、`--full` で全文) か、人間向けの導入の文例を出す |
| `ai-cu windows` | 許可されたアプリのウィンドウの一覧 (矩形、前面か、前面にできるクリック点) を出す |
| `ai-cu shot --out <PNG> [--region X,Y,W,H \| --window PROCESS]` | 画面の矩形か、対象のウィンドウを png に撮る |
| `ai-cu find` / `tree` / `text` `--window PROCESS` | 許可されたウィンドウの要素 (UI Automation) を探す、木で見る、文字列を読む |
| `ai-cu validate <FILE\|->` | 操作列 JSON を検証する (装置に触れない) |
| `ai-cu run <FILE\|-> [--dry-run]` | 操作列 JSON を実行する。`--dry-run` は装置に触れず、検証と展開した手順だけを出す |
| `ai-cu schema` | 操作列 JSON の書式を JSON で出す |
| `ai-cu click X Y` など | 一手ずつの操作 (`click`、`double-click`、`drag`、`move`、`scroll`、`type`、`key`、`focus`、`close`、`wait-for`)。`--shot` で操作の後に撮る |
| `ai-cu config show\|path\|validate` | 設定の層、場所、検証結果を出す (読み取り専用) |
| `ai-cu mcp [--log PATH]` | MCP サーバー (stdio) として待ち受ける ([MCP サーバーモード](#mcp-サーバーモード)) |
| `ai-cu` (引数なし) | 設定を編集する TUI (端末のときだけ)。`r` で操作列 JSON を選んで検証・実行できる |

AI からの典型的な使い方:

```sh
ai-cu windows                                      # 対象のウィンドウの矩形を知る
ai-cu shot --out shot.png --region 100,80,1280,720  # そのウィンドウだけを見る (全画面は既定で拒否)
ai-cu validate steps.json                          # 実行前に検証する
ai-cu run steps.json                               # 実行する (先頭の focus で対象を前面にしてから送る)
```

画面を見ながら 1 手ずつ進めるとき (押して、撮る):

```sh
ai-cu focus notepad.exe --shot a.png --window notepad.exe   # 前面にして、そのウィンドウを撮る
ai-cu tree --window notepad.exe                           # 何があるか分からなければ、先に要素の木を見る (行の @x,y は center)
ai-cu find --window notepad.exe --type button              # 押す要素の位置 (center) を UI Automation で取る
ai-cu click 640 360 --shot a.png --window notepad.exe       # クリックして撮る
ai-cu type "こんにちは" --shot a.png --window notepad.exe   # 文字を入れて撮る
ai-cu text --window notepad.exe --type document            # 入った文字列を読んで確かめる
```

- 標準出力には、結果を 1 つの JSON で出す。成功は `{"ok": true, ...}`、失敗は `{"ok": false, "error": {...}}`
- 失敗の `hint` は、終了コードごとの次の手。設定で断られたときの `error.fix` は人間向けの直し方で、AI はそれを人に伝え、設定を書き換えない
- 共通のオプション `--config=PATH` で、追加の設定ファイルを重ねる。追加の層は deny を足せるが、allow は広げられない
- 座標はすべて物理ピクセルのスクリーン座標 (スクリーンショットと同じ)。表示スケーリングの影響を受けない

全部のオプション、出力の形、エラーの理由と `detail`、版の文字列は [docs/reference/cli.md](docs/reference/cli.md)。

## MCP サーバーモード

`ai-cu mcp` は、標準入出力で待ち受ける MCP サーバー。Claude Code や Codex に登録すると、CLI と同じ操作を道具として呼べる。

```sh
claude mcp add ai-cu -- C:\Users\<名前>\.cargo\bin\ai-cu.exe mcp   # Claude Code (--scope project なら .mcp.json)
codex mcp add ai-cu -- C:\Users\<名前>\.cargo\bin\ai-cu.exe mcp    # Codex CLI (~/.codex/config.toml の [mcp_servers.ai-cu])
```

- 登録するのは、配置した release の絶対パス (`target/release` を直に登録すると、常駐の間 `cargo build --release` が失敗する)
- 道具は 19 個: 読み取りの `guide`、`config`、`windows`、`shot`、`find`、`tree`、`text`、`wait_for`、`status`、一手ずつの操作、操作列の `run`
- 設定は CLI と同じ解決で、呼び出しごとに読み直す
- 撮った画像は結果に `image` (png、base64) として入り、既定で長辺を 1568 に縮める
- クライアントの取り消し (Claude Code の Esc) と標準入力の EOF で、実行を止める
- Claude Code の権限は、読み取りの道具を allow、実入力の道具を ask にするのを推奨する。毎回の人の承認は `mcp.require_approval` で求められる

対応する MCP の版、`status` の読み方、権限と auto モードの注意は [docs/reference/mcp.md](docs/reference/mcp.md)。

## 操作列 JSON

最上位は `{"version": 1, "actions": [...]}`。各操作は `"type"` で種類を表し、未知のフィールド、種類、版は拒否する (終了コード 3)。

| type | 何をするか |
|---|---|
| `click` / `double_click` | 座標へ移動してクリック / ダブルクリックする |
| `drag` | 始点で押し、補間して移動し、終点で離す |
| `move` | カーソルを動かす |
| `scroll` | ホイールを 1 ノッチずつ回す (縦か横) |
| `type` | 文字を 1 文字ずつ入れる (既定で文字の間を 20 ミリ秒空ける) |
| `key` | キーの組み合わせ (`ctrl+s` など) を送る |
| `wait` | 決まった時間だけ待つ |
| `screenshot` | 画面の矩形か対象のウィンドウを png に保存する |
| `focus` | 許可されたアプリのウィンドウを前面にする |
| `close` | 許可されたアプリのウィンドウに `WM_CLOSE` を置く (× ボタンと同じ) |
| `wait_for` | ウィンドウ、要素、文字列が出る (消える) まで待つ |
| `wait_stable` | 画面が落ち着くまで待つ |
| `wait_change` | 画面が変わるまで待つ |

```json
{
  "version": 1,
  "actions": [
    { "type": "click", "x": 640, "y": 360 },
    { "type": "type", "text": "こんにちは\n" },
    { "type": "key", "keys": "ctrl+s" },
    { "type": "wait", "ms": 500 },
    { "type": "screenshot", "path": "after.png", "region": { "x": 0, "y": 0, "w": 1280, "h": 720 } }
  ]
}
```

各 type のフィールドと既定値、キーの名前、`run` と `--dry-run` の出力、文字入力の速さは [docs/reference/actions.md](docs/reference/actions.md)。

## 設定ファイル

TOML。allow と deny を、操作の種類、プロセス、ウィンドウクラス、画面領域、キーの 5 つの次元で宣言する。

```toml
version = 1

[general]
input_enabled = false    # マウスとキーの実入力を許すか (既定 false)
capture_enabled = false  # 画面撮影を許すか (既定 false)
password_input = false   # パスワード欄 (UIA の IsPassword) にフォーカスがあるときの type / key を許すか (既定 false)

# allow のリストは、空ならその次元を制限しない。1 件でもあれば一致が必須。
[allow]
actions = ["click", "type", "key", "screenshot"]
processes = ["notepad.exe"]          # 大文字小文字と .exe の有無は無視
classes = []                         # ウィンドウクラス名 (大文字小文字は無視)
regions = ["0,0,1920,1080"]          # "x,y,w,h" (物理ピクセル)
keys = ["ctrl+s", "enter"]           # 別名と修飾キーの順序はそろえて比べる

# deny は allow より強い
[deny]
actions = []
processes = ["cmd.exe", "powershell.exe", "WindowsTerminal.exe"]
classes = []
regions = []
keys = ["ctrl+w"]

[limits]
max_actions = 1000        # 1 回の操作列の最大の操作数
max_duration_ms = 600000  # 1 回の実行の最大時間 (操作の前の予告の間は数えず、予告の後から数える)
max_text_len = 10000      # type の最大の文字数

[stop]
sentinel_file = ""        # このファイルが存在したら停止する (絶対パス。空なら使わない)
mouse_threshold_px = 50   # 利用者がマウスをこれ以上動かしたら停止する

[capture]
full_screen = false       # 全画面 (region の省略、画面全体を覆う矩形) の撮影を許すか (既定 false)
min_idle_ms = 3000        # shot、focus、前面にある対象への close の前に求める、人の最後の入力からの無操作 (ミリ秒。0 なら求めない)
save_dirs = []            # 撮影の保存先として許すフォルダ (絶対パス)。空なら制限なし。書いた層はすべてが許すときだけ通す

[mcp]
require_approval = []     # 呼ぶたびにクライアント (Claude Code) に人の承認を求めさせる MCP の道具の名前 (例 ["click", "type", "run"])

[indicator]
enabled = true            # 画面を変える操作の前に、画面の縁に青い枠と案内の札を出すか (既定 true)
lead_ms = 3000            # 枠を出してから最初の操作までの予告の間 (ミリ秒。0 なら待たない。上限 30000)
sound = false             # 予告の間に入るときに通知音を 1 回鳴らすか (既定 false)
```

- 優先順位は、組み込みの拒否 > `deny` > `allow`。組み込みの拒否は設定では緩められない
- 人間の設定ファイルが基本の層。追加の層 (`AI_CU_CONFIG`、`--config=PATH`) は deny を足せるが、allow を広げられない
- 未知のキー、未知の版、解析できない値は、既定値に落とさず設定エラー (終了コード 5) にする
- 全画面の撮影は `capture.full_screen = true` のときだけ通す。撮影の保存先は `.png` に限り、`capture.save_dirs` で絞れる

基本の層は `%APPDATA%\ai-cu\config.toml` で、無ければ何も許可しない。
解決した場所は `ai-cu config path`、各層の中身は `ai-cu config show` で確かめられる。
引数なしで起動する TUI (端末のときだけ) で、この設定を項目ごとに編集し、保存の前に許可を広げる変更を確かめられる。

判定の規則の全部、場所と層、TUI のキー操作は [docs/reference/config.md](docs/reference/config.md)。

## 終了コード

| コード | 名前 | 意味 |
|--------|------|------|
| 0 | - | 成功 |
| 1 | `internal` | 内部エラー |
| 2 | `usage` | 使い方の誤り (引数、端末なしでの引数なし起動) |
| 3 | `invalid_script` | 操作列の誤り (JSON、未知のフィールド、形、画面の外の座標) |
| 4 | `policy_denied` | 設定による拒否 (総スイッチ、allow / deny、組み込みの拒否、上限、保存先、パスワード欄) |
| 5 | `config` | 設定ファイルの誤り (構文、未知のキー、未知の版、指定したファイルが無い) |
| 6 | `stopped` | 緊急停止 (目印のファイル、最大実行時間、利用者のマウス操作) |
| 7 | `foreground_guard` | フォアグラウンドガード (届く先のウィンドウが不許可、管理者権限、ウィンドウが無い、ai-cu を起動した側のウィンドウなど) |
| 8 | `os_error` | OS の呼び出しの失敗 (`SendInput`、撮影、保存、UI Automation の時間切れ) |
| 9 | `forbidden_dev_env` | 開発シェル (`AI_CU_DEV_FORBID_REAL_INPUT`) での実入力と実画面の撮影の拒否 |
| 10 | `user_active` | 人が操作中なので、撮らなかった、前面にしなかった、閉じなかった、送らなかった |
| 11 | `focus_failed` | `focus` の対象を前面にできなかった |
| 12 | `window_not_found` | 対象のウィンドウが見つからない、最小化されている、または画面の外 |
| 13 | `element_not_found` | `text` の条件で、文字列を持つ要素が無い |
| 14 | `desktop_unavailable` | 画面のロック、スクリーンセーバー、UAC などの別の画面が前面にある |
| 15 | `wait_timeout` | `wait_for`、`wait_stable`、`wait_change` の条件が時間内に満たされなかった |
| 16 | `input_busy` | 別の ai-cu が画面を変える操作の最中で、30 秒待っても終わらなかった |

コードごとの理由、何を送ったか、次の手は [docs/reference/exit-codes.md](docs/reference/exit-codes.md)。

## 安全機能

- **既定で拒否 (fail-closed)**: 設定が無い、または `input_enabled` / `capture_enabled` が false なら、実入力と撮影を拒否する
- **設定の allow / deny**: 人間の設定が基本の層で、`--config` や環境変数は deny を足せるが allow を広げられない
- **事前検証 (`validate`)**: 操作列の形、ポリシー、上限、画面の範囲を、装置に触れずに確かめる
- **`--dry-run`**: 入力を送らず、検証の結果と、各操作がどの入力のバッチに展開されるかを出す
- **フォアグラウンドガード**: `SendInput` の各バッチの直前に、入力が届く先のウィンドウを取り直してポリシーに通す
- **起動した側の除外**: ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウには、設定にかかわらず送らない
- **緊急停止**: 目印のファイル、最大実行時間、利用者のマウス操作 (MCP では取り消しも) で止め、押したままのキーとボタンを離す
- **操作の前の枠と予告の間**: 画面を変える操作の前に、画面の縁に青い枠を出し、枠が出ていなければ予告の間を待つ
- **入力の排他**: 画面を変える操作を含む実行は、別の ai-cu と交ざらないよう、入力の錠を取ってから動く
- **入力の前の画面とキーの確認**: 画面のロックや、キーとボタンの押されたままを見つけたら、何も送らずに断る
- **パスワード欄への入力の拒否**: フォーカスがパスワードの要素にあれば、既定で `type` と `key` を断る
- **組み込みの拒否**: `Win` を含む組み合わせ、`Alt+F4`、`Ctrl+Alt+` 系、`Ctrl+Shift+Esc`、`Ctrl+Esc` は送らない
- **撮影の制限**: 全画面の撮影は設定で許したときだけ。人が操作中なら `shot` は撮らない
- **機密のウィンドウを撮らない**: `deny` に当たるウィンドウが撮る矩形に見えていれば、撮らずに断る
- **保存先の制限**: 撮影の保存先は `.png` に限り、`capture.save_dirs` で保存のフォルダを絞れる
- **設定ファイルの守り**: 設定ファイルが人と AI の境界なので、AI に書き換えさせない
- **毎回の承認 (MCP)**: `mcp.require_approval` に書いた道具は、Claude Code が呼ぶたびに人の確認を出す
- **前面化の制限**: `focus` は `SetForegroundWindow` を 1 回呼ぶだけで、前面ロックの回避策は使わない
- **閉じる操作の制限**: `close` は題名を必須にして `WM_CLOSE` を 1 回置くだけで、強制終了はしない
- **実行時ガード (開発用)**: 環境変数 `AI_CU_DEV_FORBID_REAL_INPUT` があると、入力、撮影、前面化、閉じる操作の最下層が拒否する
- **DPI の確認**: Per-Monitor V2 の DPI 認識を宣言できなければ、座標を扱う `run` と `shot` を拒否する
- **後始末**: 停止や失敗のときも、押したかもしれないキーとボタンを離す
- **TUI からの実行**: CLI の `run` と同じ経路を通り、判定は保存済みの設定ファイルで行う
- **原子的な送信**: 修飾キーを含む組み合わせとクリックは、1 回の `SendInput` で送る

どの仕組みにも限界がある (読めないアプリでのパスワード欄の判定、撮影の判定の競合など)。
各項目の細部と限界は [docs/reference/safety.md](docs/reference/safety.md)。

## 名前の由来

<details>
<summary>cu と ai-cu の由来</summary>

`cu` は computer use の略。本当は `cu` だけにしたかったが、Linux に同じ名前のコマンド (`cu`、シリアル回線の接続) があるので、ぶつからないよう `ai-cu` にした。

偶然、「アイ・シー・ユー」(I see you、あなたを見ている) とも読める。ちょっと怖いが、画面を見て (`shot`) 手を出す (`run`) このツールをよく表している。ICU (集中治療室) にも少しかかる。常に見張っていて、いざとなれば止める (緊急停止とガード) という意味で、そう遠くない。

アイコンなどを作るときの参考に。

</details>

## 開発者向け

- 参加のしかた (安全の約束、検査のコマンド、PR): [CONTRIBUTING.md](CONTRIBUTING.md)
- 脆弱性の報告: [SECURITY.md](SECURITY.md)
- 作業ルールと安全ルール (AI のエージェント向け): [CLAUDE.md](CLAUDE.md)
- 設計判断の記録: [docs/decisions.md](docs/decisions.md)
- 調べて分かった落とし穴 (Windows の入力、端末、設定のパス): [docs/design-notes.md](docs/design-notes.md)
- 構成、版の印、検証の手順、実機のテスト、手動確認: [docs/reference/development.md](docs/reference/development.md)

## ライセンス

MIT ([LICENSE](LICENSE))。不具合や要望は GitHub の Issues へ。守り (allow / deny、ガード、緊急停止など) を越えられる不具合は、公開の Issue ではなく [SECURITY.md](SECURITY.md) の非公開の報告へ。
