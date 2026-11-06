# ai-cu

AI がコンピュータ (Windows のデスクトップ) を操作するための CLI。
クリック、ドラッグ、キー入力、スクロール、待機、スクリーンショットといった一連の操作を JSON に書いて渡すと、その順に実行する。

Codex の computer use のような「スクリーンショットを見て次の操作を決めるループ」は、呼び出し側の AI (Claude Code など) が担当する。
このツールが担当するのは、そのループの「手」(操作の実行) と「目」(画面の取得) だけ。

本物のマウスとキーボードを動かすので、既定ではすべての実入力と撮影を拒否する。人間が設定ファイルで許可した範囲でだけ動く (「安全機能」を参照)。

> **English summary.** ai-cu is a Windows CLI and MCP server that lets an AI agent operate the desktop:
> click, double-click, drag, move, scroll, type, press keys, wait, and take screenshots, either one step at a time
> or as a JSON action list. The agent decides what to do by looking at screenshots; ai-cu only acts ("hands")
> and observes ("eyes"). Real input and capture are denied by default. A human-owned config file declares what the
> AI may touch (actions, processes, window classes, screen regions, keys); deny always wins, and layers added by the
> caller can only narrow it. The documentation is in Japanese.

文中の `D-xxx` は、判断の記録 (docs/decisions.md) の番号。

## 注意

- 本物のマウスとキーボードを動かす。AI に任せた操作で、誤った送信、データの消失、意図しないアプリの操作が起こりうる
- 無保証 (MIT ライセンスの「AS IS」。LICENSE を参照)。自分の責任で使う。最初は、許可するアプリ (`allow.processes`) と画面の領域 (`allow.regions`) を狭くして試す
- 人と AI の境界は、人が持つ設定ファイル。AI がシェルから設定ファイルを書き換えられる環境では、境界にならない (「安全機能」の「設定ファイルの守り」)

## 要件

- Windows 10 / 11 (x64)。管理者権限は要らない (管理者権限で動くウィンドウには入力を送らない)
- ビルドに Rust 1.95 以上 (stable、`x86_64-pc-windows-msvc`) と、Visual Studio の C++ ビルドツール

## 導入

```
cargo install --git https://github.com/alphabet-h/ai-cu --locked
```

`~/.cargo/bin/ai-cu.exe` に入る。手元に clone したなら `cargo install --path . --locked`。

## はじめかた

1. 端末で `ai-cu` を引数なしで起動すると、TUI で設定ファイル (既定は `%APPDATA%\ai-cu\config.toml`) を作って編集できる。新しく作った設定は、入力も撮影も無効で、端末とシェル、管理ツール、パスワード管理ソフトを deny に並べてある (D-117)。使うアプリを `allow.processes` に足し、`input_enabled` と `capture_enabled` を有効にする
2. `ai-cu guide --setup` が、MCP の登録 (Claude Code、Codex) と、Claude Code の権限の文例を、この実行ファイルのパスと設定の場所から組んで出す
3. AI には、最初に `ai-cu guide` (MCP なら道具の `guide`) を読ませる

## 使い方

```
ai-cu run <FILE|->            操作列 JSON を実行する (- で標準入力)
ai-cu run <FILE|-> --dry-run  装置に触れず、検証と展開した手順だけを出す
ai-cu validate <FILE|->       操作列 JSON を検証する (装置に触れない)
ai-cu guide                   AI 向けの使い方の手引きの要点版 (Markdown) を出す (規則の節だけ。D-097)
ai-cu guide --full            手引きの全文 (基本の流れ、一手ずつの操作、find と text、操作列 JSON を含む)
ai-cu guide --setup           人間向けの導入の文例 (この実行ファイルのパスを埋める)
ai-cu windows [--include-hidden]
                              許可されたアプリのウィンドウの一覧 (矩形、前面か、前面にできるクリック点。撮影の許可が要る)。--include-hidden で許可済みのプロセスの隠れた窓 (hidden_windows) も返す (読むだけ。D-099)
ai-cu schema                  操作列 JSON の書式を JSON で出す
ai-cu shot --out <PNG> [--region X,Y,W,H | --window PROCESS [--window-title TEXT]] [--scale F] [--max-edge N]
                              画面の矩形、または対象のウィンドウを png に撮る (どちらも省略 = 全画面は、既定の設定では拒否)
ai-cu find --window PROCESS [--window-title TEXT] [--name TEXT] [--type TYPE] [--automation-id ID] [--focused] [--limit N]
                              許可されたウィンドウの要素 (UI Automation) を名前・種類・automation_id・フォーカスで探し、矩形と中心点を出す。--automation-id は完全一致 (大文字小文字を区別)、--focused はキーボードのフォーカスを持つ要素だけ (窓の中のフォーカス。前面でない窓でも当たり、複数ありうる。0 件でも成功)。各要素に focused と password (パスワードの要素か。UIA の IsPassword。D-090) と、そのパターンを持つ要素だけ toggle (on / off / indeterminate)、selected (bool)、expanded (expanded / collapsed / partially_expanded / leaf_node) が出る。name は 500 字で切り、切った要素にだけ name_truncated: true が出る (「…」は付けないので、そのまま --name に渡せる。D-102) (読み取りだけ。撮影の許可が要る。D-089)
ai-cu tree --window PROCESS [--window-title TEXT] [--depth N] [--limit N] [--all]
                              要素の木を深さで切って、1 行 1 要素の字下げの文字列 (種類 "名前" @x,y。@x,y は center) で出す。名前の無い入れ物と画面外は畳む。skipped は読めなかった要素と取れなかった枝の数で、find の skipped と数え方が違う (読み取りだけ。撮影の許可が要る。D-084)
ai-cu text --window PROCESS [--window-title TEXT] [--name TEXT] [--type TYPE] [--max-chars N]
                              要素の文字列 (文書の本文、入力欄の値) を読む。本文は既定 20000 字で切り、出力の truncated と total_chars (切る前の字数) で分かる (--max-chars で 1〜1000000。D-102)。出力の element には focused と password が出る (状態は出ない。ほかの要素の欄は find の行と同じで、name_truncated を含む)。パスワードの要素 (UIA の IsPassword) は文字列を読まずに飛ばして次の要素へ進み、文字列を持つ要素が無ければ 13 で、飛ばした数を message の末尾に「(パスワードの要素 N 個は読まない)」と添える (読み取りだけ。撮影の許可が要る。D-090)
ai-cu config show|path|validate            設定の層、場所、検証結果を出す (読み取り専用)
ai-cu mcp [--log PATH]        MCP サーバー (stdio) として待ち受ける。Claude Code や Codex の設定に登録して、道具として呼ぶ (「MCP サーバーモード」)
ai-cu                         引数なし: 設定を編集する TUI (端末のときだけ)。r で操作列 JSON を選んで検証・実行できる

一手ずつの操作 (D-052)。各サブコマンドは 1 操作の操作列として run と同じ経路 (検証、ポリシー、ガード、緊急停止) を通り、出力も run と同じ (`--shot` を付けると、操作、待ち、撮影の 3 操作になり、`executed` は 3。`--settle-ms 0` と `--settle-ms auto` なら 2):
ai-cu click X Y [--button right]           クリック          ai-cu double-click X Y     ダブルクリック
ai-cu drag X1 Y1 X2 Y2 [--steps N]         ドラッグ          ai-cu move X Y             マウスを動かす
ai-cu scroll CLICKS [--at X,Y]             スクロール        ai-cu type TEXT|-          文字を入れる (- で標準入力)
ai-cu key ctrl+s                           キーの組み合わせ  ai-cu focus notepad.exe    ウィンドウを前面にする
ai-cu close notepad.exe --title TEXT       ウィンドウを閉じる (WM_CLOSE。前面は変えない)
ai-cu wait-for PROCESS [--title TEXT] [--name TEXT] [--type TYPE] [--text TEXT] [--absent] [--timeout-ms MS]
                                           条件がそろうまで待つ (ウィンドウ、要素、文字列が出る / 消えるまで。読むだけ。D-068)。--text はパスワードの要素を読まずに飛ばす (D-090)
  共通: --shot PATH [--window PROCESS [--window-title TEXT] | --region X,Y,W,H] [--scale F] [--max-edge N] [--settle-ms 300|auto] で操作の後に撮る
        (操作列の screenshot と同じ扱い)。--dry-run で装置に触れない
        --settle-ms auto は固定の待ちの代わりに、画面が落ち着くまで (最長 3000 ミリ秒) 待ってから撮る (screenshot の settle。MCP は shot.settle_ms: "auto"。D-081)
```

- 共通のオプション `--config=PATH` で、追加の設定ファイルを重ねる。追加の層は deny を足せるが、allow は広げられない
- focus / close / wait-for の対象は位置引数 (`ai-cu close notepad.exe --title メモ`)。`--window` は撮る画像の対象で `--shot` と組む。対象を `--window` で渡して usage の誤り (2) になったときは、hint がそれを伝える (D-086)
- `windows` の結果の `hidden_allowed` は、`allow.processes` で許可済みのプロセスのうち、動いているが見えるウィンドウが無い (タスクトレイ、非表示、別の仮想デスクトップ) ものの実行ファイル名 (許可に無い名前は出さない)。一覧 (`windows`) が空のときだけ、`hint` が「隠れている」か「許可に無いか、まだ起動していない」かを分けて次の手を言う (D-074)。ただし `--include-hidden` で `hidden_windows` に窓が出て、`hidden_allowed` が空のときは `hint` を出さない (隠れた窓が結果にそのまま出ているため。D-099)
- `windows --include-hidden` (MCP は `include_hidden: true`) のときだけ、結果に `hidden_windows` と `hidden_windows_truncated` が加わる (旗が無ければ、どちらのキーも出ない)。`hidden_windows` は許可済みのプロセスの隠れたトップレベルの窓を Z 順に並べたもので、1 件は `{"hwnd", "process", "class", "title", "state"}`。`state` は `hidden` (見えない) か `cloaked` (見えるが別の仮想デスクトップなどで cloaked)。許可は `windows` と同じ (設定の `allow` / `deny` のプロセスとクラスの規則と、ai-cu を起動した側の除外) で、許可されない窓は出さず数えもしない。最大 200 件で、超えた分は捨てて `hidden_windows_truncated` を true にする。矩形と座標は返さず、隠れた窓は操作 (focus、close、撮影) の対象にならない。見えているが題名の無い窓やツールウィンドウ、ai-cu 自身の窓 (枠など。別の ai-cu のプロセスの枠も)、OS がスレッドごとに作る IME の窓は出ない。読む間に見え方が変わった窓は、`windows` と `hidden_windows` の両方に出るか、どちらにも出ないことがある。プロセスの制限 (`allow.processes`) が無い設定では、どのアプリの隠れた窓も並び、上限で切られやすい (D-099)
- 標準出力には、結果を 1 つの JSON で出す。成功は `{"ok": true, ...}`、失敗は `{"ok": false, "error": {"code", "exit_code", "message", "hint", "action_index"}}`。`hint` は終了コードごとの次の手。同じ終了コードの中で理由を分けるときは、機械が読む `error.reason` (`focus` の 11 の `not_found` / `minimized` / `closed` / `refused_host_front` / `refused_other_app` / `refused_unknown`、MCP の 7 の `host_unknown`、`close` の 7 の `target_disabled`、10 の `keys_held` (キーかマウスのボタンが押されたまま)、10 の `lead_input` (予告の間に人の入力があった。D-093)、7 の `target_changed` (文字を打つ途中で前面が別のプロセスに変わった。途中まで送ってある。D-100)、14 の `screensaver` / `other_desktop` / `locked`) を足す。`error.detail` は理由の補足と、ガード・停止・無操作の状況 (D-065): 11 の断られた場合は `set_foreground` (0 か 1)、`foreground_process`、`foreground_is_host`、MCP の補助プロセスを使ったかの `helper` (true / false) とその結果の `helper_result` (`accepted` / `refused` / `timeout` / `failed`、使わなければ null。`failed` のときは理由が `helper_error` に入る。D-057)。7 (フォアグラウンドガード) は対象のウィンドウの `window` (`process`、`class`、`pid`、`hwnd`。入力のガードでは届く先、`focus` では前面にする対象 (前面化の後に対象が開いたダイアログが前面になって断ったときは、そのダイアログ)、`close` では閉じる対象。届く先が無ければ null) と、入力のガードのマウスでは点の `point` (`x`、`y`)。撮る矩形に deny のウィンドウが見えていて断ったときは、そのウィンドウの `window` と、見えている部分の外接矩形の `visible` (`x`、`y`、`w`、`h`)。応答しない deny のウィンドウの代わりに OS が出したゴーストで断ったときは、`window` はゴーストのウィンドウ (`class` が `Ghost`) で、`message` の末尾にその旨が付く (D-115)。パスワード欄かを確かめる間に前面のウィンドウが変わった 7 は、変わった後の前面の窓の `window`。4 (パスワード欄への入力の拒否) は、最初のガードが読んだ前面の窓の `window` (7 と同じ形。D-090)。6 の利用者のマウスでの停止は `cursor` (読んだカーソル)、`from` と `expected` (直前の移動の線分)、`distance_px` (その線分からの距離)。10 (`focus` と `close`) は `idle_ms` と `min_idle_ms`、10 の `keys_held` は押されたままのキーの `keys` (`shift`、`ctrl`、`alt`、`win`) とボタンの `buttons` (`left`、`right`、`middle`、`x1`、`x2`)、10 の `lead_input` は予告の終わりに読んだ無操作の長さの `idle_ms` と、設定の予告の間の `lead_ms`。14 は入力デスクトップの名前の `input_desktop` (読めなければ null)。15 (`wait_for`、`wait_stable`、`wait_change` の時間切れ) は `timeout_ms`、`checks` (確かめた回数。画面の待ちでは撮った回数)、`unmet` (最後の判定で満たされなかった段。`wait_for` は `window` / `element` / `text`、`wait_stable` は `stable`、`wait_change` は `change`)、`window` (最後の判定で見た対象のウィンドウ。7 と同じ `process`、`class`、`pid`、`hwnd`。その判定で無ければ null。画面の待ちでは `window` で待ったときだけ値が入る)。画面の待ちの 15 には、最後に撮った矩形の `rect` (`x`、`y`、`w`、`h`) と、最後の比較の差の `last_changed_ratio` (`stable` は直前のフレームとの差、`change` は待ち始めの基準との差。比べる前なら null) も入る (D-081)。16 (別の ai-cu の操作の終わりを待ちきれなかった) は待った長さの `waited_ms` (D-113)。題名は入れない。設定による拒否 (4 と、許可の外れで断る 7) には、当たった設定のキーの `error.rule` と、人間向けの直し方の `error.fix` を足す (D-079)。`rule` の値は設定のキーそのもの (`general.input_enabled`、`general.capture_enabled`、`general.password_input`、`allow.actions`、`deny.actions`、`allow.keys`、`deny.keys`、`allow.regions`、`deny.regions`、`allow.processes`、`deny.processes`、`allow.classes`、`deny.classes`、`limits.max_actions`、`limits.max_duration_ms`、`limits.max_text_len`、`capture.full_screen`、`capture.save_dirs`。TUI で項目を選んだとき、右の枠の先頭に灰色で出るキーと同じ) で、組み込みのキーの拒否は `builtin`、保存先の形と既存ファイルの規則は `save_path`。値は変えない (名前を変えるときは別名を足す)。`fix` は「許可してよければ、<どこで> の <キー> <変え方>」の形の 1 文で、<変え方> はキーで変わる (allow の一覧なら「に <値> を足す」、deny の一覧なら「から <値> を外す」、上限 (`limits.*`) なら「を <n> 以上にする」、スイッチ (`general.input_enabled`、`general.capture_enabled`、`general.password_input`、`capture.full_screen`) なら「を true にする」)。<どこで> は断った層で変わる: 基本の層 (人間の設定ファイルの層。コードと過去の判断ログでは floor / 床と呼ぶ) なら TUI (引数なしの起動。左の一覧での場所「<節> > <項目>」を添える) か基本の層の設定ファイルのパス、追加の層ならその層の設定ファイル (`--config` か `AI_CU_CONFIG`)。基本の層の設定ファイルがまだ無いときは、どの層で断っても「設定ファイルがまだ無いので何も許可されていない。端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動し、TUI で設定を作る (手順は ai-cu guide --setup)」の別の文になり、設定の場所を引けないときは `fix` を付けない。`builtin`、`save_path`、撮る矩形に deny のウィンドウが見えている 7 (D-077) にも `fix` は付けない (設定では直せないか、緩めるよう頼ませない)。AI は `fix` をそのまま人に伝え、設定を書き換えない。`validate` と `--dry-run` の `issues[]` の各項目にも同じ `rule` と `fix` が付き、最上位の `error` の 2 つは最初の問題 (`action_index` と同じ) のもの。`reason`、`detail`、`rule`、`fix` は値があるときだけ出し、既存の項目は変えない (D-056、D-079)。
- 実行器が操作に入った後の失敗 (`run` と一手ずつの操作) では、成功と同じ `executed`、`screenshots`、`focused`、`closed`、`elapsed_ms` と、失敗した操作の副作用 (入力、前面化の依頼、`WM_CLOSE`) を一部でも起こしたかの `partial` が、失敗の JSON の最上位に入る (D-065)。`executed` は済んだ操作の数で、失敗した操作の添字 (`action_index`) と等しい。`executed` が無い失敗は、実行器が操作に入る前のもの (引数、設定、検証、装置の用意、画面とカーソルの読み、入力の錠の待ち (6 と 16。D-113)、入力デスクトップと押されたままのキーの確認、予告の間 (6 と、10 の `lead_input`。D-093)) で、何も送っていない。`executed` が 1 以上か `partial` が true なら、`hint` の頭に「actions[0]〜actions[k-1] は実行済み (executed=k)。」(k が 1 なら「actions[0] は実行済み (executed=1)。」、k が 0 なら無し) と、`partial` なら「actions[i] も途中まで行った (partial=true)。」、続けて「操作ごと打ち直さない。…以下は失敗した actions[i] について: 」(`partial` なら「…以下は actions[i] の止まった所から先について (それより前に起こしたことはそのまま): 」。続く次の手の「何も送っていない」は止まった所から先の話。OS の失敗 (8) では、止まった束も一部だけ届いていることがある) の一文が付き、元の次の手が続く。そのときは操作列を頭から打ち直さず、`windows` や `shot` で状態を見直してから残りを送る
- 人間向けの説明とログは標準エラーに出す。例外は `guide` (Markdown) と `--help`
- AI に使わせるときは、使う側のプロジェクトの CLAUDE.md などに、次のように書いておく:「デスクトップの操作は ai-cu だけで行う。最初に `ai-cu guide` を読む」。`ai-cu guide --setup` は、この実行ファイルのパスを埋めた人間向けの導入の文例 (設定ファイルの場所と、無ければ TUI で作る手順、MCP の登録、CLAUDE.md の文例の CLI 版と MCP 版、Claude Code の権限の例) を出す。設定ファイルは中身を読まず、有無だけを見る。何も書かない
- 座標はすべて物理ピクセルのスクリーン座標 (仮想デスクトップ。スクリーンショットと同じ)。起動時に Per-Monitor V2 の DPI 認識を宣言するので、表示スケーリングの影響を受けない

AI からの典型的な使い方:

```
ai-cu windows                                      # 対象のウィンドウの矩形を知る
ai-cu shot --out shot.png --region 100,80,1280,720  # そのウィンドウだけを見る (全画面は既定で拒否)
ai-cu validate steps.json                          # 実行前に検証する
ai-cu run steps.json                               # 実行する (先頭の focus で対象を前面にしてから送る)
```

画面を見ながら 1 手ずつ進めるとき (押して、撮る):

```
ai-cu focus notepad.exe --shot a.png --window notepad.exe   # 前面にして、そのウィンドウを撮る
ai-cu tree --window notepad.exe                           # 何があるか分からなければ、先に要素の木を見る (行の @x,y は center)
ai-cu find --window notepad.exe --type button              # 押す要素の位置 (center) を UI Automation で取る
ai-cu click 640 360 --shot a.png --window notepad.exe       # クリックして撮る
ai-cu type "こんにちは" --shot a.png --window notepad.exe   # 文字を入れて撮る
ai-cu text --window notepad.exe --type document            # 入った文字列を読んで確かめる
```

ビルドは `cargo build --release`。成果物は `target/release/ai-cu.exe`。対象は x86_64-pc-windows-msvc。
`ai-cu --version` は `ai-cu 0.1.0+g<コミットの先頭 12 桁>` を出す (`0.1.0` は Cargo.toml の `version`)。未コミットの変更がある木で作った release は `.dirty` が付き、git が読めなければ `+unknown`。MCP の serverInfo の `version` も同じ文字列 (D-069)。版の文字列の文法:

```
<version> = <CARGO_PKG_VERSION> "+" <meta>
<meta>    = "g" <hex>{12} [ ".dirty" ] | "unknown"
```

- `<CARGO_PKG_VERSION>`: Cargo.toml の `version` (semver。pre-release の `-rc1` などを付けてもよい。`+` は含めない)
- `g<hex>{12}`: ビルド時の `git rev-parse HEAD` の先頭 12 桁
- `.dirty`: ビルド時に `git status --porcelain --untracked-files=normal` が空でなかった (未コミットの変更か、無視されていない未追跡のファイルがある木で作った。`target/` と `.dev/` は無視されているので数えない)
- `unknown`: `rev-parse` か `status` が失敗した (git が無い、リポジトリでない)

使う側 (MCP の登録や PATH からの呼び出し) には、`~/.cargo/bin/ai-cu.exe` に入れたもの (「導入」の `cargo install`。開発者は下の `python scripts/install.py`) を使う。`scripts/install.py` は、作業中の release を配置する開発者向けの道具で、動いている MCP サーバーがあっても配置でき (古い版は `ai-cu.old-<時刻>.exe` に改名して残り、次の配置で消える)、サーバーは再接続で新しい版になる。配置は、作業ツリーが clean で、release の印が HEAD と一致するときだけ通る (未コミットの変更、git が読めない、`--version` に印が無い (この変更より前の release)、`.dirty`、`+unknown`、HEAD と違うコミットは断り、条件ごとの次の手を出す)。順は /verify → コミット → `cargo build --release` の打ち直し → `python scripts/install.py` (D-064、D-069)。配置は「.dev/install-log.jsonl」に 1 行ずつ記録される

## MCP サーバーモード

`ai-cu mcp` は、標準入出力の JSON-RPC (改行区切り) で待ち受ける MCP サーバー。MCP 対応のエージェント (Claude Code、Codex、Claude Desktop など) の設定に登録すると、CLI と同じ操作を道具として直接呼べる (D-055)。
MCP の現行版 (2026-07-28。`initialize` の無い stateless 方式。`server/discover` に応える) と、旧版 (2025-11-25 以前。`initialize` の握手) の両方に応える。

登録の例 (配置した release の絶対パスを書く。`target/release` を直に登録すると、常駐の間 `cargo build --release` が実行ファイルを消せずに失敗する。debug ビルドの `mcp` は実入力の道具を終了コード 9 で断る):

```
claude mcp add ai-cu -- C:\Users\<名前>\.cargo\bin\ai-cu.exe mcp   # Claude Code (--scope project なら .mcp.json)
codex mcp add ai-cu -- C:\Users\<名前>\.cargo\bin\ai-cu.exe mcp    # Codex CLI (~/.codex/config.toml の [mcp_servers.ai-cu])
```

- 設定は CLI と同じ解決 (基本の層 = 利用者の設定ファイル、`AI_CU_CONFIG`、`--config=PATH` は `mcp` の前に書く)。層は呼び出しごとに読み直すので、常駐中に設定を狭めれば次の呼び出しから効く (ただし `mcp.require_approval` は客が `tools/list` を取り直したとき (Claude Code では `/mcp` の再接続) に効く)。処理中の実行は開始時の設定で走る
- 道具 (19 個): 読み取りの `guide` (手引きの要点版。`full: true` で全文。D-097)、`config`、`windows` (`include_hidden: true` で隠れた窓の `hidden_windows` も。D-099)、`shot`、`find`、`tree`、`text`、`wait_for` (条件がそろうまで待つ。読むだけ) と、一手ずつの `focus`、`click`、`double_click`、`drag`、`move`、`scroll`、`type`、`key`、`close` (`shot` オブジェクトで操作の後に撮る。`dry_run` で装置に触れない)、操作列の `run` (`actions` の配列)。どれも CLI と同じ経路 (検証、ポリシー、フォアグラウンドガード、緊急停止、実行器) を通り、CLI と同じ JSON を結果の text で返す
- 加えて MCP だけの読み取りの道具 `status` がある (CLI のサブコマンドは無く、上の経路も通らない。引数は空で、撮影の許可も要らず、何も送らない。D-073)。実入力の道具が 10 や 14 で断られたとき、再試行の前に読む。結果は `idle_ms` (セッションの最後の入力からの経過。ai-cu 自身の入力も数える)、`min_idle_ms` (`capture.min_idle_ms`)、`user_active` (`min_idle_ms` が正で `idle_ms` がそれ未満)、`retry_after_ms` (`user_active` のときだけ `min_idle_ms - idle_ms`。人が触り続ければ延びる下限で、それ以外は null)、`cooldown` (人か停止の仕組みが直前の実入力を止め、まだ解けていない。`status` は下ろさない)、`keys_held` (`{keys, buttons}`)、`desktop` (`{state, name}`。`state` は `available` / `screen_saver` / `locked` / `other`)、`stop_files` (目印のファイルの `{path, present}`)、`foreground` (`{process, allowed, is_host}`。許可の無い前面 (起動した側を含む) と、撮影と実入力の総スイッチが両方とも切れた設定ではプロセス名を伏せ、題名は出さない)、`input_refused` (debug ビルド・ホスト不明・DPI の失敗で実入力を断るときの `{code, message}` と、あれば `reason`。DPI の失敗には `reason` が無い)、`unreadable` (読めずに null にした項目の名前。ロック中など)、`indicator` (`{enabled, lead_ms, sound}`。操作の前の枠と予告の間の設定を層で合成した値。`enabled` が true なら、画面を変える道具は枠が出ていないとき `lead_ms` だけ待ってから動く (別の ai-cu の錠の待ちの後は、枠が出ていても `lead_ms` をまるごと待つ。D-113)。D-093。`frame` は枠の今の状態で、枠の装置を持たない経路では null、それ以外は `{state, error, retry_in_ms}`。`state` は `not_started` (このプロセスがまだ枠を出していないだけ。画面を変える道具を呼んでいないか、`enabled` が false)、`running` (枠のスレッドが動き、窓を持っている)、`start_failed` (枠のスレッドを立てられなかったか、ひとりでに終わった)、`rebuild_failed` (モニターの構成の変化の後の作り直しに失敗している)。失敗の 2 つでは枠は出ていないが予告の間は待ち、`error` が理由 (他の状態では null)。`start_failed` は最後の失敗から 60 秒たった後の次の画面を変える操作で立て直し、`retry_in_ms` がそれまでの残り。起動の時間切れの後、止まった枠のスレッドが残っている間は `retry_in_ms` が null で、片付くまで立て直さない (片付かなければ MCP の再接続で戻る。D-103))、読み方の `note`。設定が読めなければ、その誤りで失敗する
- 失敗は `isError: true` の結果で、CLI と同じ `{"ok": false, "error": {"code", "exit_code", "message", "hint"}}` を返す (終了コードの表、`reason`、`detail`、`rule`、`fix` がそのまま使える。`executed`、`partial` などの最上位の項目も同じ)。`hint` の文は、CLI の書き方 (`ai-cu ...`、`--shot`) ではなく道具の名前で書いたものになる (D-056)。ai-cu を起動した側を特定できないときは、実入力の道具を 7 (`reason: host_unknown`) で断る。未知の道具と形の崩れたリクエストだけが JSON-RPC の誤り (-32602)
- 撮った画像は結果に `image` (png、base64) として入る (`path` を書けばファイルにも保存する。`inline: false` なら保存だけ)。Claude Code は画像を会話に出し、元のバイト列を自分のセッションのファイルにも保存する。画像を含む結果は Claude Code の `MAX_MCP_OUTPUT_TOKENS` (既定 25000) にそのまま掛かる。既定で長辺を 1568 に縮める (`max_edge`。客のモデルの長辺の上限と、画像の多い会話の 1 辺 2000 の上限に収める)。さらに `scale` と `window` / `region` で小さくする
- 常駐の緊急停止: クライアントの取り消し (`notifications/cancelled`。Claude Code の Esc) と標準入力の EOF (クライアントの終了) を、実行器が各バッチの前と待ちの途中 (50 ミリ秒ごと) に拾い、押したままのキーとボタンを離して止める。取り消されたリクエストには応答しない。目印のファイル (作ると以後の呼び出しをすべて止める)、利用者のマウス移動、最大実行時間は CLI と同じ。人かそれらの仕組みが実行を止めた直後 (予告の間の入力で止めた 10 の `lead_input` を含む) は、人がマウスとキーボードから離れる (`capture.min_idle_ms`) まで実入力の道具を終了コード 10 で断る
- 操作の前の枠と予告の間 (D-093。下の安全機能): 画面を変える道具 (一手ずつの入力の道具、`focus`、`close`、それらを含む `run`) は、動かす前に画面の縁に青い枠を出し、枠が出ていなければ `lead_ms` (既定 3000) 待ってから動く。別の ai-cu が画面を変える操作の最中なら、先にその終わりまで入力の錠を待ち (最大 30 秒。待ちきれなければ何も送らずに 16)、錠の待ちの後は枠が出ていても `lead_ms` をまるごと待つ (D-113)。常駐の MCP は実行の後 30 秒枠を残し、枠が出ている間は読み取りを含む道具の呼び出し (`guide`、`config`、`dry_run` を除く) の始めと終わりで残す時間を延ばすので、一連の作業の間は枠が出続け、次の呼び出しは待たずに続く (錠を待った呼び出しを除く。AI の一手ずつの呼び出しで毎回待たない。読み取りの道具だけでは新しく出さない。設定で枠を切っていれば延ばさない。D-107)。人か停止の仕組みに止められた後は枠を残さない。読み取りの道具と `dry_run` では出さない
- `ping` は実行中でも読み取りスレッドが即答する。長い呼び出し (`run`) はクライアントの道具の制限時間 (Codex は既定 60 秒) に収まる長さにする。制限時間で切れた呼び出しを、そのまま再発行しない
- ai-cu を起動した側 (エージェント、シェル、端末。親プロセスの連鎖と、その子のコンソール) のウィンドウは、設定にかかわらず入力の届く先、`focus`、`close`、`shot --window`、`find`、`tree`、`text`、`wait_for` の対象にせず、`windows` の一覧からも外す (終了コード 7)。シェル (explorer.exe) は含めない。起動した側を特定できなければ実入力の道具を断る
- 標準出力には MCP のメッセージ以外を書かない。ログ (どの道具が、どの終了コードで、何ミリ秒か、取り消しを受けたか) は標準エラーで、引数と結果と題名は書かない。クライアントが標準エラーを残さないとき (Claude Code の MCP のログには出ない) は、`mcp --log PATH` で同じ行を時刻 (UTC) つきでファイルにも追記する (開けなければログなしで続ける。D-061)
- Claude Code の推奨の権限: 読み取りの道具 (`mcp__ai-cu__guide` など) を allow、実入力の道具を ask。auto モードでは人の門が利用者の設定ファイルだけになる (`mcp.require_approval` に書いた道具を除く) ので、設定ファイルの編集を Claude Code 側で deny にしておく (例: `Edit(~/AppData/Roaming/ai-cu/**)`)。`ai-cu guide --setup` が、この権限の例を設定の場所から組んで出す
- auto モードでは、実入力の道具 (`mcp__ai-cu__click`、`drag`、`run` など) に ask か allow のルールを必ず書く。どちらも無いと、Claude Code の分類器が呼び出しごとに判定し、同じ操作が直前は通ったのに次は拒否される、ということが起きる (ai-cu は呼ばれない)。毎回の確認が要らないなら allow にし、入力の届く先は ai-cu の設定ファイルの `allow.processes` と `allow.regions` で絞る。`require_approval` に書いた道具は allow のルールがあっても毎回確認が出る
- 毎回の承認 (D-078): 設定の `mcp.require_approval` に書いた道具は、`tools/list` で `_meta` に `"anthropic/requiresUserInteraction": true` の印が付く。Claude Code はその道具を呼ぶたびに人の確認を出し、auto や bypassPermissions のモードでも、allow のルールがあっても省かず、「次から聞かない」も出さない (dontAsk のモードでは拒否し、`--permission-prompt-tool` からの allow は拒否に変わる)。印は客への依頼で、ai-cu は `tools/call` で確かめない。Codex など印に対応しない客は無視する。印は一覧を取ったときに決まるので、設定を変えたら (壊れた設定を直したときも) `/mcp` で再接続する。設定が読めない間の `tools/list` は、失敗にせず `guide` を除くすべての道具に印を付けて返す (締める側に倒す)。一覧が利用者の設定で変わるので、`tools/list` の `cacheScope` は `private` (`server/discover` は `public` のまま)

## 操作列 JSON

最上位は `{"version": 1, "actions": [...]}`。各操作は `"type"` で種類を表す。未知のフィールド、未知の種類、未知の版は拒否する (終了コード 3)。

| type | フィールド | 説明 |
|------|------------|------|
| `click` | `x`, `y`, `button` (`left` / `right` / `middle`、既定 `left`) | 移動して押して離す (1 回の `SendInput`)。操作列で次が `type` か `key` なら、その前に 200 ミリ秒待つ (`double_click` も同じ) |
| `double_click` | `x`, `y`, `button` | ダブルクリック |
| `drag` | `from: {x, y}`, `to: {x, y}`, `button`, `steps` (1 から 1000、既定 20), `duration_ms` (既定 300、最大 60000) | 始点で押し、`steps` 回に補間して移動し、終点で離す |
| `move` | `x`, `y` | カーソルの移動 |
| `scroll` | `clicks` (0 以外。正で上または右), `horizontal` (既定 false), `x` と `y` (省略可。組で指定すると先に移動) | 1 クリック = 1 ノッチ (WHEEL_DELTA = 120)。本物のホイールと同じく 1 ノッチずつ、間を 20 ミリ秒空けて送る (100 ノッチで約 2 秒。D-063) |
| `type` | `text`, `interval_ms` (既定 20、最大 1000) | 文字入力。1 文字ずつ送り、文字の間を `interval_ms` だけ空ける。改行は Enter、タブは Tab、その他は `KEYEVENTF_UNICODE` (サロゲートペア対応) |
| `key` | `keys` (例 `ctrl+s`, `alt+tab`, `f5`, `shift+left`) | 修飾キーを押し、キーを押して離し、修飾キーを逆順に離す (1 回の `SendInput`)。その後 50 ミリ秒待つ |
| `wait` | `ms` (最大 600000) | 待つ。待っている間も緊急停止を確かめる |
| `screenshot` | `path`, `region: {x, y, w, h}` または `window: {process, title?}` (どちらも省略で仮想デスクトップ全体。既定の設定では拒否)、`scale` (0 より大きく 1 以下。既定 1)、`max_edge` (1 以上。長辺の上限。縮尺は `scale` と `max_edge` / 長辺 の小さい方。省略で上限なし、MCP は既定 1568)、`settle` (省略可。`{stable_ms?, threshold?, timeout_ms?}`。既定 300 / 0 / 3000、`timeout_ms` は `stable_ms` 以上 600000 以下。`{}` で既定値) | png に保存する。`window` の矩形は撮る直前に `focus` と同じ探し方で解き、見つからない / 最小化 / 画面の外なら終了コード 12。`settle` があれば、撮る前に `wait_stable` と同じ判定で画面が落ち着くのを待ち、最後に撮った画像を保存する (撮り直さない)。`timeout_ms` のうちに落ち着かなくても失敗にせず、そのときの画像を保存する。出力の `screenshots[]` に `settled` (落ち着いたか) と `settle_ms` (待った時間) が入る。一手ずつの `--settle-ms auto` と MCP の `shot.settle_ms: "auto"` は、これを既定値で付ける (D-081) |
| `focus` | `process` (実行ファイル名), `title` (省略可。題名に含む文字列) | 許可されたアプリのウィンドウを前面にする。`SetForegroundWindow` を 1 回呼ぶだけで、入力は送らず、最小化は復元しない。既に前面なら何もしない。人が操作中 (`capture.min_idle_ms`) なら終了コード 10、できなければ 11。前面にしたウィンドウは出力の `focused` に入る (D-050) |
| `close` | `process` (実行ファイル名), `title` (必須。題名に含む文字列) | 許可されたアプリのウィンドウに `WM_CLOSE` を 1 回置く (`PostMessageW`。× ボタンと同じ)。入力は送らず、前面も変えないので、前面のガードは掛けない (端末が前面のままでも閉じられる)。対象がダイアログ (保存の確認など) を開いていて無効なら 7 (`reason: target_disabled`。閉じるボタンも押せない状態)、対象 (または対象に所有されたダイアログ) が前面で人が操作中なら終了コード 10 (自分の入力の直後も同じ。同じ操作列で対象に入力した後なら、前に `wait` を置く)、見つからなければ 12。閉じるか (保存の確認を出すか) はアプリが決める。出力の `closed` に、閉じようとしたウィンドウと `gone` (1 秒のうちに見える窓から消えたか。破棄とは限らない) と `hidden` (窓が破棄されず同じプロセスのまま隠れただけ。トレイに入るアプリで、プロセスは動いている。正確には 1 秒の間、隠れたまま壊れなかった。後始末に 1 秒より長くかけて壊すアプリも true になりうる) が入る。`hidden` が false でもプロセスが終わったとは限らない (exe の入れ替えや再起動の前に確かめるか人間に聞く) (D-062、D-096) |
| `wait_for` | `process` (実行ファイル名), `title` (省略可。題名に含む文字列), `element` (省略可。`{name?, type?}`。`name` は部分一致、`type` は `find` と同じ種類の名前。どちらかが要る), `text` (省略可。要素の文字列に含む文字列。改行は LF にそろえ、大文字小文字を区別), `absent` (既定 false。true なら無くなるまで), `timeout_ms` (既定 10000、最大 600000) | 条件がそろうまで待つ (読むだけで何も送らない)。条件は一番細かい指定で決まる: `text` があれば「絞り込みに一致して最初に文字列を持つ要素の文字列が `text` を含む」、無くて `element` があれば「要素が 1 つ以上ある」、どちらも無ければ「ウィンドウが見えている (最小化は無い扱い)」。`absent` はその否定。最初の判定は待たずに行い、満たされなければ 250 ミリ秒の待ちを挟んで確かめ直す (読み取りの時間は待ちに含めない。その間も緊急停止と最大実行時間を見る)。`timeout_ms` のうちに満たされなければ終了コード 15 (`wait_timeout`)。満たされると出力の `waited` に窓と要素 (`center` はそのまま `click` に渡せる) が入る。`absent` が最初の確認で満たされると `note` も付く (D-070)。パスワードの要素 (UIA の IsPassword) は文字列を読まずに飛ばすので、`text` の条件はパスワードの要素の文字列とは一致しない。`absent` と `text` を組むと、パスワードの要素は最初から無いと数えられ、最初の判定で即時に満たされる (パスワード欄の文字列が消えるのは待てない。D-090)。許可は撮影と同じ (`capture_enabled`、対象のプロセスとクラス)。`timeout_ms` は時間の予算 (`max_duration_ms`) に全額数える (D-068) |
| `wait_stable` | `region: {x, y, w, h}` または `window: {process, title?}` (`screenshot` と同じ形と検証。どちらも省略で全画面、既定の設定では拒否)、`stable_ms` (既定 300、100 以上 60000 以下)、`threshold` (既定 0。0 以上 1 未満)、`timeout_ms` (既定 10000、`stable_ms` 以上 600000 以下) | 画面が落ち着くまで待つ (何も送らない)。UI Automation に要素を出さないアプリや、描き終わるまでの時間が読めない画面に使う。最初の撮影は待たずに行い (`window` が隠れて見えたときは猶予だけ待つ。D-082)、100 ミリ秒の間を空けて撮り直す (撮影の時間は間に含めない。その間も緊急停止と最大実行時間を見る)。各フレームを長辺 256 以下に縮め、RGB のどれかの差が 16 を超える点の割合を直前のフレームと比べ、その差が `threshold` 以下の比較が `stable_ms` 以上続いたら満たす。`window` の矩形は撮るたびに解き、矩形が前と違えば (移動、大きさの変化) 変わったと数える。撮るたびに `screenshot` と同じ判定 (入力デスクトップ、`window` の解決、領域と全画面の規則、deny のウィンドウ) を通し、当たれば止める (`window` の隠れだけは 50 ms ごとに 500 ms まで解き直してから 7。D-082。ロックやセーバーは 14、`window` が見つからない / 最小化は 12、`window` が隠れている、または deny のウィンドウが見えていれば 7 (待つ間に現れても同じ)、許可の外れは 4)。`timeout_ms` のうちに満たされなければ 15 (`unmet: stable`)。満たされると出力の `waited` に最後に撮った矩形の `rect` が入る。`waited[].elapsed_ms` は最初の撮影の前の隠れの猶予 (D-082) を含めず、途中の猶予の分だけ `timeout_ms` を超えうる。`timeout_ms` は時間の予算に全額数える (D-081) |
| `wait_change` | `region` または `window` (`wait_stable` と同じ)、`threshold` (既定 0.001。0 以上 1 未満)、`timeout_ms` (既定 10000、1 以上 600000 以下) | 画面が変わるまで待つ (何も送らない)。待ち始めの最初のフレームを基準にし、以後のフレームと基準との差が `threshold` を超えたら満たす (直前のフレームとは比べないので、少しずつ変わる画面でも積もれば満たす)。撮り方、間隔、判定、止まり方は `wait_stable` と同じ。`timeout_ms` のうちに満たされなければ 15 (`unmet: change`)。満たされると `waited` に `rect` と、そのときの基準との差の `changed_ratio` (小数第 4 位まで) が入る。待ち始めに既に変わっていた画面は基準になるので満たされない。狭い `region` では、キャレットの点滅 1 回でも 0.001 を超えうる (`threshold` を上げるか、広く撮る。D-081) |

キーの名前: `a`-`z`、`0`-`9`、`f1`-`f24`、`enter`、`esc`、`tab`、`space`、`backspace`、`delete`、`insert`、`home`、`end`、`pageup`、`pagedown`、`up`、`down`、`left`、`right`、`pause`、`capslock`、`printscreen`。
修飾キー: `ctrl` (`control`)、`alt`、`shift`、`win` (`super`)。大文字小文字と修飾キーの順序は問わない。

例:

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

`run` の成功時の出力は `{"ok": true, "executed": <実行した操作の数>, "screenshots": [{"action_index", "path", "x", "y", "width", "height", "scale", "window"?, "settled"?, "settle_ms"?}], "focused": [{"action_index", "hwnd", "process", "title", "helper"}], "closed"?: [{"action_index", "hwnd", "process", "title", "gone", "hidden"}], "waited"?: [{"action_index", "elapsed_ms", "checks", "window"?, "element"?, "note"?, "rect"?, "changed_ratio"?}], "elapsed_ms": ...}` (`closed` は `close` があったときだけ出る。`gone` は見える窓から消えたか (破棄とは限らない)、`hidden` は窓が破棄されず隠れただけか (トレイに入るアプリ。プロセスは動いている。false でもプロセスが終わったとは限らない。D-096)。`gone` が false なら保存の確認などが出ていることが多い。`screenshots[].settled` と `settle_ms` は `screenshot` に `settle` があったときだけ出る (落ち着いたかと待った時間。D-081。`settle_ms` は最初の撮影の前の隠れの猶予 (D-082) を含めず、途中の猶予の分だけ `timeout_ms` を超えうる)。`waited` は `wait_for`、`wait_stable`、`wait_change` があったときだけ出る。画面の待ち (`wait_stable`、`wait_change`) の項目には、最後に撮った矩形の `rect` (`x`、`y`、`w`、`h`。次の `shot` や `click` の範囲に使える)、`window` で待ったときの `window`、`wait_change` で満たしたときの基準との差の `changed_ratio` が入り、`element` と `note` は出ない (`elapsed_ms` は最初の撮影の前の隠れの猶予を含めない。D-082)。`wait_for` の項目では `rect` と `changed_ratio` は出ず、`window` (`hwnd`、`process`、`title`) と `element` (`find` と同じ形で `center` つき、`text` は無し。`element.focused` は要素がキーボードのフォーカスを持つか (最上位の `focused` (focus で前面にしたウィンドウ) とは別)。要素の条件なら状態 (`toggle` など) も乗る。D-089) は無ければキーごと省く (`absent` でウィンドウ自身が消えたときは `window` が無く、`element` は「ある」で満たされた要素か文字列の条件のときだけ)。`note` は `absent` が最初の確認で満たされたとき (`timeout_ms` 0 は除く) だけ出る注意で、最初の確認で既に無かったことと次に確かめる道具 (windows / find / text) を書く。UI Automation に要素を出さないアプリでの空振りに気づくためのもの (D-070)。`focused` は `focus` で前面にしたウィンドウ。`helper` は常駐の `ai-cu mcp` が補助プロセスで前面にしたときだけ true (D-057)、`screenshots[].window` は `window` で指定して撮ったウィンドウ)。最上位の `elapsed_ms` は予告の間 (操作の前に枠を出して待つ間。D-093) を含めず、予告の後から数える。画像の点 (px, py) は画面の (x + px / scale, y + py / scale)。
`--dry-run` の出力は `{"ok": ..., "dry_run": true, "issues": [...], "plan": [{"index", "steps": [...]}], "batches": ..., "screen": {"x", "y", "w", "h"} | null}` で、各操作がどの `SendInput` のバッチと待ちに展開されるかを示す。

文字入力の速さについて: アプリ (Windows 11 のメモ帳など) は入力を非同期に処理するので、速く送りすぎると文字を落としたり順序を入れ替えたりする。`type` は既定で 1 文字ずつ 20 ミリ秒の間隔で送る (1 秒あたり約 50 文字)。`interval_ms` を小さくすると速くなるが、受け手によっては文字が欠ける。`ok: true` は、ai-cu がすべての入力を OS に渡せたという意味で、アプリが処理し終えたことまでは保証しない。結果は `screenshot` で確かめる。
入力欄を `click` してすぐ打つと、アプリによっては (gpui など、クリックで移ったフォーカスを次の描画で入力欄に反映するもの) 最初の 1 文字が落ちる。そのため、操作列で `click` / `double_click` の次が `type` か `key` なら、間に 200 ミリ秒待つ (計画に `sleep` として出て、時間の予算に数える。間に `wait` を書けば、その待ちだけにする)。一手ずつの道具を別々に呼ぶときは、呼び出しの間が十分に長いので待たない (D-059)。

## 設定ファイル

TOML。allow と deny を、操作の種類、プロセス、ウィンドウクラス、画面領域、キーの 5 つの次元で宣言する (docs/decisions.md の D-028)。

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

判定の規則:

- 優先順位は、組み込みの拒否 > `deny` > `allow`。組み込みの拒否は `Win` を含む組み合わせ、`Alt+F4`、`Ctrl+Alt+` 系、`Ctrl+Shift+Esc`、`Ctrl+Esc` で、設定では緩められない
- プロセスとクラスは、マウスの操作では座標の下のウィンドウ、キーと文字では前面のウィンドウに対して、送る直前に判定する
- `focus` の対象は、`validate` でプロセス名を、実行時には見つけたウィンドウをフォアグラウンドガードと同じ判定 (プロセス、クラス、管理者権限、ai-cu 自身の端末) に通す。`focus` は入力を送らないが、`general.input_enabled` と `allow.actions` / `deny.actions` の対象
- `close` の対象も同じ判定に通す (D-062)。`allow.actions` を書いた設定では、`close` を並べないと閉じられない。画面領域の規則は `focus` と同じく掛けない (領域はマウスの点と撮影の矩形の規則)。閉じさせたくなければ `deny.actions = ["close"]`
- `wait_for` の対象も、`validate` でプロセス名を、実行時には見つけたウィンドウをプロセスとクラス (と ai-cu を起動した側、自分の端末) で判定する。読むだけなので `general.input_enabled` は要らず、`general.capture_enabled` (撮影と同じ総スイッチ) と `allow.actions` / `deny.actions` の対象。`allow.actions` を書いた設定では `wait_for` を並べないと待てない。拒否されたウィンドウは待たずに即時に 4 (有無を待って探れないように。D-068)
- `wait_stable` と `wait_change` は撮影を繰り返すだけの操作なので、`screenshot` と同じ許可に通す: `general.capture_enabled`、`allow.actions` / `deny.actions` (`allow.actions` を書いた設定では並べないと待てない)、`region` の領域の規則と全画面の禁止、`window` の対象のプロセスとクラス、deny のウィンドウ。判定は撮るたびに行う。`screenshot` の `settle` は撮影の一部なので、`screenshot` の許可だけで使える (D-081)
- `wait` (固定の待ち) は何も動かさないので、総スイッチと `allow.actions` / `deny.actions` の対象外。`deny.actions = ["wait"]` は受け付けるが効かない
- `deny.processes` と `deny.classes` は撮影にも効く: 撮る矩形にそのウィンドウが見えていれば、どの撮り方でも撮らずに 7 で断る (`allow` に無いだけのウィンドウは見ない。D-077)
- `processes` は実行ファイル名で比べ、パスや署名は見ない。`python.exe`、`java.exe`、Electron 系のような汎用の宿主を `allow.processes` に書くと、その名前のどの窓も対象になるので、`classes` で絞る (D-028、D-101)
- `regions` は座標の規則 (マウスの点と撮る矩形) で、要素の読み取り (`find`、`tree`、`text`、`wait_for`) には効かない。許可したプロセスなら、画面外や deny の領域に重なる部分の文字も読める。中身を隠したいアプリは `deny.processes` か `deny.classes` に書く (D-054、D-101)
- 画面領域は、マウスの各点 (ドラッグの補間の点を含む) と撮影の矩形に適用する。撮影の矩形は、deny の領域と少しでも重なれば拒否し、allow の領域があればどれかに丸ごと収まる必要がある
- `type` の改行とタブは、Enter と Tab のキーとしても判定する
- 領域の規則があるときは、`scroll` に `x` と `y` を求める。画面の大きさを読めなかった検証 (出力の `screen` が null) は、領域の規則があると全画面の `screenshot` を通さない (`region` を指定する)
- 待ちの合計に実行の余裕 1000 ミリ秒を足した値が `max_duration_ms` を超える操作列は、検証で拒否する。`max_duration_ms` は 1000 より大きい値にする
- 検証 (`validate`、`--dry-run`、`run` の実行の前) は、画面の大きさ (`GetSystemMetrics` の読み取りだけ) を使って、画面の外の座標を終了コード 3 にする (D-058)。使った画面は出力の `screen` に出る (DPI 認識の宣言に失敗したときなど、読まなかったら null で、範囲は `run` の実行の前に見る)。画面の大きさを使うので、領域の規則の上では、領域に収まる全画面の `screenshot` も判定できる (全画面の許可は次の項で別に要る)
- 撮る対象をウィンドウで指定したとき (`screenshot` の `window`、`shot --window`) は、矩形を撮る直前に解き、そのウィンドウをフォアグラウンドガードと同じ判定 (プロセス、クラス、権限) に通してから、解いた矩形を領域の規則と全画面の禁止に通す (D-053)。検証では対象のプロセスだけを見る。画面の矩形の撮影はそこに見えているものを撮るので、対象の一部が別のウィンドウ (対象に所有されたダイアログを除く) に隠れていれば、その中身が写り込まないよう撮らずに終了コード 7 で止める (矩形の中を 64 ピクセル間隔で間引いて確かめる。それより小さい重なりは見逃しうる)。操作列の撮影 (`screenshot`、`settle`、`wait_stable`、`wait_change`。一手ずつの操作の後の `--shot` と MCP の操作の道具の `shot` オブジェクトも同じ) では、隠れて見えたら撮らずに 50 ms ごとに解き直し、500 ms のうちに前面に描かれれば撮り、その間に窓が消えれば 12 にする (focus の直後の描画の遅れと、閉じかけの窓に次の窓が重なる間。使い切ったら 7 で、message の末尾に待った時間が付く。D-082)。`shot` サブコマンド (`shot --window`) と MCP の `shot` 道具は待たずに 7。`--region` と全画面の撮影には隠れの確認は無いが、deny のウィンドウが見えていれば 7 で断る (下の安全機能の「機密のウィンドウを撮らない」。D-077)
- 要素の読み取り (`find`、`tree`、`text`。UI Automation) は、要素の名前と文字列が画面上の情報なので撮影と同じ許可 (`capture_enabled`) を求め、対象のウィンドウはフォアグラウンドガードと同じ判定 (プロセス、クラス、権限) に通す。開発シェル (`AI_CU_DEV_FORBID_REAL_INPUT`) では最下層が拒否し、フィクスチャ専用の装置はフィクスチャのウィンドウだけに応じる。COM は呼び出しごとに専用の MTA スレッドで初期化する (D-054)。例外は、type / key の前に読むフォーカスの要素の IsPassword (下の `general.password_input`) で、読むのは真偽 1 つだけで画面の情報を返さないので、hwnd の門 (`uia_allowed`) も `capture_enabled` も通さず、開発シェルでは断る門 (`focused_password_allowed`) だけを通して読む (フィクスチャ専用の装置でも hwnd で絞らない。`GetFocusedElement` は hwnd を取らない。D-090)
- `general.password_input` (既定 false) は、キーボードのフォーカスがパスワードの要素 (UIA の IsPassword) にあるときの `type` と `key` を許すか。層の合成は `input_enabled` と同じで、どの層も true のときだけ許す (上の層は倒すだけ)。許していなければ、各操作の最初のキーの束を送る前にフォーカスの要素を読み、パスワード欄なら何も送らずに 4 (`rule` が `general.password_input`) で断る。どの層も許していれば読まない (D-090)
- 全画面の撮影 (`region` を省略した `shot` と `screenshot`、または仮想デスクトップ全体を覆う矩形) は、どの層も `capture.full_screen = true` のときだけ通す (既定は拒否)。`validate` は `region` の無い `screenshot` を、画面の大きさを知らなくても拒否する。複数モニタでは、1 枚のモニタ全体を覆う矩形は全画面とみなさない (仮想デスクトップ全体だけ)。上書き層に `[capture]` の節が無ければ、既定 (拒否側) が入る
- `shot` は直前に、人の最後の入力からの経過 (`GetLastInputInfo`) を見て、`capture.min_idle_ms` (層の中で最も大きい値。既定 3000) 未満なら撮らずに終了コード 10 で断る。ai-cu 自身の `run` の入力も数える。`run` の中の `focus` も同じ判定で、人が操作中なら前面にせずに 10 で止まる (既に前面なら判定しない。D-050)。`close` も、対象 (または対象に所有されたダイアログ) が前面にあるときだけ同じ判定で、人が操作中なら閉じずに 10 で止まる (D-062)。`run` のそれ以外の操作 (`screenshot` を含む)、`windows`、TUI には適用しない (D-049)。`run` は人が対象のウィンドウを前面にした直後に始まるのが普通なので判定しない。その分、人の操作中でも `run` の `screenshot` は撮れる経路が残る (手引きで、10 を `run` で代替しないよう求める)
- 撮影の保存先 (`shot --out`、`screenshot` の `path`、一手ずつの `--shot`、MCP の `path`) は、撮る前に判定して、外れれば撮らずに終了コード 4 で断る (D-075)。設定に関係なく、拡張子は `.png` (大文字小文字は問わない) に限り、`\\?\` と `\\.\` で始まるパスと、ドライブの `:` より後ろの `:` (代替データストリーム。`report.docx:a.png` は拡張子が png に見えて既存のファイルに書き込める) は断り、既にあるファイルが PNG (先頭 8 バイトの署名) でなければ (フォルダを含む) 上書きしない。これら (形と既存ファイル) の断りには、設定では緩められないので path を直すよう求める `hint` が付く。`capture.save_dirs` を書いた層では、保存先がその一覧のどれかのフォルダの下にあるときだけ通す (書いた層のすべてが許す必要があり、空の層は判定に加わらない。どの層も書かなければ制限なし)。保存先は作業ディレクトリに足して `.` と `..` を字面で畳み、部品の単位で大文字小文字を無視して比べる (`C:\Shots` は `C:\shots2\a.png` を許さない)。ドライブ相対の `D:x.png` は絶対にならないので、`save_dirs` のある層では断られる。ジャンクションとシンボリックリンクはたどらない (字面だけで比べるので、許したフォルダの中のリンクを経由すれば外に書ける)。書く直前にも形と既存ファイルを同じハンドルで確かめ直し、そこで断れば終了コード 8 (`画像を保存できない`)。MCP の仮のパス `mcp:inline` は判定しないが、CLI で書くと撮った後に 8 で断られる
- `mcp.require_approval` は、書いた層の和集合 (上の層は道具を足せるが外せない。基本の層が無いときの閉じた層は何も足さない)。名前は MCP の道具の名前 (`guide`、`click`、`run` など 19 個。大文字小文字と前後の空白は無視し、重複は 1 つにする) で、未知の名前と空の名前は設定エラー (終了コード 5)。MCP の客が `tools/list` を取り直したときに効く (上の「MCP サーバーモード」。D-078)
- `[indicator]` (操作の前の枠と予告の間。D-093) は知らせる側を強める向きに合成する: `enabled` はどれかの層が true なら出す (すべての層が false のときだけ出さない)、`lead_ms` は層の最大 (上の層は延ばせるが縮められない)、`sound` はどれかの層が true なら鳴らす。`[indicator]` を書いていない層 (追加の層の `--config` と `AI_CU_CONFIG` を含む) も既定値 (true、3000、false) で合成に加わるので、基本の層だけで弱めても、追加の層を使う実行では既定に戻る (`capture.min_idle_ms` と同じ)。`lead_ms` が 30000 を超えると設定エラー (誤記で毎回長く止まるのを防ぐ)。音と予告の間は枠に従属し、`enabled = false` なら音も待ちも無い。`config show` と MCP の `config` の層ごとの表示、MCP の `status` の `indicator` で値を確かめられる
- 未知のキー、未知の版、解析できない値は、既定値に黙って落とさず設定エラー (終了コード 5) にする

場所と層 (D-029):

- 基本の層 (人間の設定): `%APPDATA%\ai-cu\config.toml` (`SHGetKnownFolderPath` で引く。環境変数の `APPDATA` は読まない)。無ければ「何も許可しない層」になる
- 追加の層: 環境変数 `AI_CU_CONFIG`、`--config=PATH` の順に重ねる。操作は、すべての層が許可したときだけ実行する。指定したファイルが無ければ設定エラー
- TUI で編集するファイルは、`--config`、`AI_CU_CONFIG`、既定の場所の順で最初のもの
- `ai-cu config path` で解決結果を、`ai-cu config show` で各層の中身を確かめられる

TUI (引数なしで起動): 標準入力と標準出力の両方が端末のときだけ起動する。

- 画面は 2 ペイン。左は見出しでまとめた項目の一覧 (変更した項目に ●)、右は選んだ項目の説明、全件の値、効果で、編集も右で行う。上部の帯に有効な状態の要約を出し、実入力が有効で許可リスト (アプリ、クラス、領域) が空なら赤で警告する。幅が 80 桁未満なら 1 ペインになる
- 左: `↑↓` で選び、`Enter` か `→` で開く (総スイッチは切り替え)。右: `↑↓` で選び、`Enter` で編集か追加、`a` で追加、`d` で削除、`←` か `Esc` で戻る
- 追加の方法は項目で変わる: 操作はチェックリスト (`Space` で切り替え)、アプリとクラスは開いているウィンドウの候補から選ぶか手入力 (文字を打つと絞り込む)、領域は画面全体の雛形か手入力、キーは入力欄。入力欄は打つたびに ✓ か ✗ を出す
- どこでも: `s` か `Ctrl+S` で保存、`u` で元に戻す、`?` でヘルプ、`q` で終了 (未保存なら「保存して終了 / 破棄 / 戻る」を選ぶ)。`Ctrl+C` は未保存なら確認に回り、もう一度押すと強制終了
- 確認が出る操作: 総スイッチを有効にする、許可リストを空から使い始める、許可リストを空にする。保存のときは、許可を広げる変更を ⚠ で並べて確認する (グループ「表示」の `[indicator]` で枠をやめる、予告の間を縮める、音をやめる変更も「知らせを弱める」として並ぶ)
- 操作列の実行 (`r`): 作業ディレクトリ直下の `*.json` の一覧から選ぶか、パスを手入力する。`Enter` で事前検査 (保存済みの設定で、画面の大きさつきの検証。装置には触れない) → 確認の画面 (操作数、バッチ数、判定に使う層、停止の手段、注意) → `y` で 5 秒のカウントダウン (その間に対象のウィンドウを前面にして、マウスから手を離す。`Esc` か `q` で取りやめ) → 実行 → 結果 (終了コードの名前、失敗した操作、次の手) を TUI の中に出す。実行か事前検査が設定で断られたときは、`error.fix` (事前検査なら最初の問題の `fix`) を「直し方:」の 1 行で添える。判定は常に保存済みの設定ファイルで行い、未保存の変更があるときは実行しない (先に `s`)。実行中は画面が止まり、キーでは止められない (止めるにはマウスを大きく動かす)。画面の縁の枠は出すが、予告の間はカウントダウンがあるので省く (D-093。別の ai-cu の入力の錠を待った後は省かない。D-113)。ai-cu を動かしている端末のウィンドウには送らない
- 保存はコメントと並びを残す差分編集で、元のファイルは `.bak` に残る。読めない設定を開いたときは既定値で表示し、保存すると置き換える (元は `.broken-<時刻>.bak` に残し、以後の保存で上書きしない)。端末が無いとき (mintty の pty のように本物のコンソールでないときを含む) と、`AI_CU_NO_TUI` か `CI` があるときは、usage を出して終了コード 2 で終わる。mintty (Git Bash) では `winpty ai-cu` か Windows Terminal で起動する。

## 終了コード

| コード | 名前 | 意味 |
|--------|------|------|
| 0 | - | 成功 |
| 1 | `internal` | 内部エラー |
| 2 | `usage` | 使い方の誤り (引数、端末なしでの引数なし起動) |
| 3 | `invalid_script` | 操作列の誤り (JSON、未知のフィールド、形、画面の外の座標) |
| 4 | `policy_denied` | 設定による拒否 (総スイッチ、allow / deny、組み込みの拒否、上限、撮影の保存先の規則、パスワード欄への入力。保存先の形と既存ファイルの規則は設定では緩められないので path を直す) |
| 5 | `config` | 設定ファイルの誤り (構文、未知のキー、未知の版、指定したファイルが無い) |
| 6 | `stopped` | 緊急停止 (目印のファイル、最大実行時間、利用者のマウス操作)。操作の前の予告の間 (D-093) に目印のファイル、マウスで止まったときも 6 で、何も送っていない (`executed` が無い。MCP の取り消しは応答を返さない)。別の ai-cu の入力の錠を待つ間 (D-113) に目印のファイルか MCP の取り消しで止まったときも、錠の待ちの 6 で何も送っていない |
| 7 | `foreground_guard` | フォアグラウンドガード (届く先のウィンドウが不許可、管理者権限、ウィンドウが無い、ai-cu を起動した側のウィンドウ。`focus` と `close` の対象の拒否、`window` で撮る対象が別のウィンドウに隠れているとき、撮る矩形に deny のウィンドウが見えているとき、パスワード欄かを確かめる間に前面のウィンドウが変わったときも同じ)。`reason` が `target_changed` なら、1 つの操作 (`type`) の途中で前面が別のプロセスの窓に変わったので残りを送らずに止めた。それより前の文字は送ってある (`partial` が真。`detail` に変わった後の `window` と最初の `pinned_pid`。D-100) |
| 8 | `os_error` | OS の呼び出しの失敗 (`SendInput`、撮影、保存)。UIA の読み取り (find、text、tree、wait_for の要素) が 30 秒で返らなければ 8。返らない読み取りが 4 本残っている間は読まずにすぐ 8 で、ai-cu を起動し直すまで続きうる (D-104)。読みの途中で UIA が時間切れを返したら (対象のアプリが固まったとき)、残りを読まずに 8 (既定の待ちでは UIA の処理の時間切れ 20 秒の 1 回分。20 秒は Microsoft Learn の既定値からの推定で、測っていない。D-110)。時間切れで読めなかった要素を `skipped` に数えたり、属性を空にして入れたりはしない。入力の錠 (D-113) を作れない・開けない・試せないときも 8 で、何も送っていない (`executed` が無い)。message が権限の違うプロセスの錠か、同じ名前のミューテックスでない物を言うときは、やり直しても直らないので人間に伝える |
| 9 | `forbidden_dev_env` | 開発シェル (`AI_CU_DEV_FORBID_REAL_INPUT`) での実入力と実画面の撮影の拒否 |
| 10 | `user_active` | 人が操作中 (最後の入力から `capture.min_idle_ms` 未満) なので、`shot` を撮らなかった、`focus` で前面にしなかった、または前面にある対象を `close` で閉じなかった。`reason` が `keys_held` なら、キーかマウスのボタンが押されたままなので、操作列の入力を何も送らなかった (押されたままのキーを人に確かめる。D-066)。`reason` が `lead_input` なら、操作の前の予告の間 (画面の縁に枠を出してから動かすまで) に人がマウスかキーボードに触れたので、何も送らずに止めた (`detail` に `idle_ms` と `lead_ms`。人が離れるのを待って 1 回だけやり直す。MCP では `status` の `retry_after_ms`。D-093) |
| 11 | `focus_failed` | `focus` の対象を前面にできなかった (見つからない、最小化、閉じられた、Windows が断った。`error.reason` で分かる)。断られたときは前面のプロセスで分ける: 人が別のアプリを使っている (`refused_other_app`)、前面は ai-cu を起動した側なのに断られた (`refused_host_front`。常駐の `ai-cu mcp` では、補助プロセスを試しても前面にならなかったときなど。試したかは `error.detail` の `helper`)、前面を確かめられない (`refused_unknown`)。入力は送っていない (タスクバーのボタンは点滅しうる) |
| 12 | `window_not_found` | 対象のウィンドウ (`screenshot` の `window`、`shot --window`、`find`、`tree`、`text`、`wait_stable` と `wait_change` の `window`) が見つからない、最小化されている、または画面の外。撮影も読み取りもしていない (操作の後の `--shot` や操作列の `screenshot` なら、どこまで済んだかは最上位の `executed` で分かる)。`close` の対象が見つからない、または `WM_CLOSE` を置けなかったときも 12 で、何も閉じていない |
| 13 | `element_not_found` | `text` の条件で、文字列を持つ要素が無い (パスワードの要素は読まずに飛ばし、飛ばしたときは message の末尾に「(パスワードの要素 N 個は読まない)」が付く。D-090)。何もしていない |
| 14 | `desktop_unavailable` | 画面がロックされているか、スクリーンセーバー、UAC などの別の画面が前面にある (`error.reason` は `screensaver`、`other_desktop`、`locked` (セッションのロック) のどれか)。待っても再試行しても直らないので、人間に解除を頼む。`executed` が無ければ何も送っていない (D-066、D-067) |
| 15 | `wait_timeout` | `wait_for`、`wait_stable`、`wait_change` の条件が時間内に満たされなかった。何も送っていない (操作列なら、それより前の操作は済んでいて `executed` で分かる)。`error.detail` に `timeout_ms`、`checks` (確かめた回数)、`unmet` (`wait_for` は `window` / `element` / `text`、`wait_stable` は `stable`、`wait_change` は `change`)、`window` (最後の判定で見た対象。題名なし。無ければ null)。画面の待ちでは、最後に撮った矩形の `rect` と最後の比較の差の `last_changed_ratio` も入る。同じ条件で繰り返し待たない。`wait_for` なら `windows`、`find`、`text`、`shot` で状態と条件を見直す (D-068)。画面の待ちなら `shot` で今の画面を見て、動き続ける部分 (動画、スピナー、点滅) があれば `region` を狭めるか `threshold` を上げ、変わるのを待っていたなら待つ前に既に変わっていなかったか (待ち始めの画像が基準になる) を確かめる (D-081) |
| 16 | `input_busy` | 別の ai-cu (別の AI か、人が動かしている ai-cu) が画面を変える操作の最中で、30 秒待っても終わらなかった。何も送っていない。`error.detail.waited_ms` に待った長さ。数十秒待って 1 回だけやり直し、続けて断られるなら人間に別の AI の動きを確かめてもらう (D-113) |

4 と、許可の外れで断る 7 (入力の届く先、`focus` と `close` の対象) には、当たった設定のキーの `error.rule` と人間向けの直し方の `error.fix` が付く (上の「使い方」の出力の形。D-079)。撮る矩形に deny のウィンドウが見えている 7 は `rule` だけで、`fix` は付かない。

## 安全機能

- **既定で拒否 (fail-closed)**: 設定が無い、または `input_enabled` / `capture_enabled` が false なら、実入力と撮影を拒否する。`validate` と `--dry-run` は設定が無くても使える
- **設定の allow / deny**: 上の 5 つの次元で宣言する。人間の設定が基本の層で、`--config` や環境変数は deny を足せるが allow を広げられない
- **事前検証 (`validate`)**: 操作列の形、ポリシー、上限、画面の範囲を、装置に触れずに確かめる。`run` も実行の前に同じ検証を通す
- **`--dry-run`**: 入力を送らず (入力の装置を作らず、読むのは画面の大きさだけ)、検証の結果と、各操作がどの入力のバッチに展開されるかを出す
- **フォアグラウンドガード**: `SendInput` の各バッチの直前に、入力が届く先 (マウスは座標の下のウィンドウ、キーは前面のウィンドウ) を取り直してポリシーに通す。管理者権限のウィンドウ (UIPI で入力が届かず、失敗も検出できない) と、権限を確かめられないウィンドウには送らない。1 つの操作 (`type`) のキーの束は、最初の束の前面のプロセスに固定し、途中で別のプロセスの窓が前面になれば残りを送らずに 7 (`target_changed`) で止める。同じプロセスの別の窓 (ダイアログ) への移りは止めない。UWP のアプリどうし (前面のプロセスが共通の `ApplicationFrameHost.exe`) の移りは見分けられないと推定している (確かめていない。D-100)。応答しないアプリの代わりに OS が出すゴーストのウィンドウ (クラス `Ghost`) には、設定にかかわらず送らない (届く先がゴーストなら、マウスもキーも 7。D-119)
- **起動した側の除外**: ai-cu を起動した側 (親プロセスの連鎖: エージェント、シェル、端末、Claude Desktop など。その子のコンソールも) のウィンドウには、設定にかかわらず送らず、`focus` と `close` と `window` 指定の撮影と要素の読み取り (`find`、`tree`、`text`、`wait_for`) の対象にもしない (終了コード 7)。`region` と全画面の撮影には、矩形に入っていれば写る (deny に当たっても数えない。D-077、D-101)。連鎖は起動時に 1 回たどり、PID の再利用を作成時刻の比較で防ぐ。シェル (explorer.exe) の手前で止める (D-055)
- **緊急停止**: 各バッチの前、ドラッグの補間の各ステップ、待機の途中 (50 ミリ秒ごと) で確かめる。停止要因は、目印のファイル (`stop.sentinel_file` と環境変数 `AI_CU_STOP_FILE`)、最大実行時間、利用者のマウス操作 (自分が動かした位置から `mouse_threshold_px` を超えて離れたら停止)。MCP ではクライアントの取り消しと標準入力の EOF も停止要因になる。停止や失敗のときは、押したままのキーとボタンをすべて離す。最大実行時間 (`limits.max_duration_ms`) は操作の前の予告の間を数えず、予告の後から数える (短い上限で予告が上限を食わないように)。予告の間も 50 ミリ秒ごとに目印のファイル、MCP の取り消し、利用者のマウスを見る (D-093)。別の ai-cu の入力の錠の待ちの間も 50 ミリ秒ごとに目印のファイルと MCP の取り消しを見る (マウスは見ない。動かしているのは前の ai-cu なので。D-113)
- **操作の前の枠と予告の間** (D-093): 画面を変える操作 (マウス、キー、文字入力、`focus`、`close`) を含む実行では、検証と操作前の判定 (ロック、押されたままのキー) が通った後、最初の操作の前に、すべてのモニターの縁に青い枠 (約 6 px、内側に向けて薄くなる) と、上の中央に案内の札「ai-cu が操作中 — マウスを動かすと止まる」を出す。枠が出ていなければ `indicator.lead_ms` (既定 3000) 待ってから動く (予告の間。`indicator.sound` なら、待ちに入るときに通知音を 1 回鳴らす)。予告の間に、マウスを動かすと 6、キーかマウスに触れると (始めの 300 ミリ秒は除く) 何も送らずに 10 (`reason: lead_input`) で止まる。予告を省くのは、自分か他の ai-cu の枠が `lead_ms` 以上前から出ているとき (常駐の MCP の連続の呼び出しや、別の AI が操作を終えて残している枠、重い検査の実機テスト (tests/fixture_real.rs) がフィクスチャを開いている間。D-094。`python scripts/verify.py --ignored` が段の始めから終わりまで出す枠はクラス名が別 (`ai_cu_verify_frame`) で、ai-cu の枠に数えないので、それが出ていても予告は省かない。D-111)。他の ai-cu の枠が出たばかりなら、残りだけ待つ。ただし枠を読むのは入力の錠 (下の入力の排他) を取った後で、別の AI が画面を変える操作の最中なら、その実行が終わって錠が解けるのを待ってから枠を読む。錠を待った実行は、枠が出ていても予告を省かず縮めず、`lead_ms` をまるごと待つ (D-113)。複数の AI が同時に動かすと、それぞれが自分の枠を出す (重なって少し濃く見える)。入力が交ざらないようにする排他は、入力の錠で行う (D-113)。実行が終わると、CLI はすぐ、常駐の MCP は最後の呼び出しから 30 秒後に枠を消す (出ている枠は、装置に触れる道具の呼び出しのたびに 30 秒後まで延びる。D-107)。人か停止要因に止められた実行 (6 と、10 の `lead_input`) の後は、すぐ消す (残すと次の実行が予告と人の入力の判定を省いてしまう)。枠を出していない実行 (読み取りの `wait_for`、待ちや撮影だけの `run`) が止められたときも、前の呼び出しが残した自分の枠を消す。常駐の MCP は、実行の後に届いた取り消しでも消す。TUI からの実行は枠を出し、予告の間は省く (カウントダウンがある。別の ai-cu の錠を待った後は省かない。D-113)。撮影だけ、待ちだけ、`find` などの読み取り、`--dry-run`、`validate`、操作前の判定で断る実行、ロック中に始まる close だけの操作列では出さない。枠は撮影から外していて (`WDA_EXCLUDEFROMCAPTURE`)、ai-cu の撮影にも画面録画や画面共有にも写らない。クリックを素通しし、前面を取らず、`windows` の一覧 (隠れた窓の `hidden_windows` を含む。どの ai-cu のプロセスの枠も) と前面のガードと撮影の覆いの判定にも出ない。枠は知らせる手段で、守りの層ではない: 枠を出せなくても (窓を作れない、撮影から外せない) 実行は止めず、予告の間は待ち、標準エラー (MCP では `--log` のファイルにも) に警告を 1 行書く。起動に失敗したら 60 秒たった次の操作で立て直し (時間切れのときは止まった枠のスレッドが片付いてから)、常駐の MCP では `status` の `indicator.frame` で状態と理由を読める (D-103)。主スレッドが固まっても、枠は期限 (予告の間 + 最大実行時間 + 残す時間 + 2 秒。呼び出しで延ばした後は、最後の延長から 30 秒) で自分で消える
- **入力の排他** (D-113): 画面を変える操作 (マウス、キー、文字入力、`focus`、`close`) を含む実行は、検証が通った後、操作前の判定 (ロック、押されたままのキー) と枠・予告の間より前に、セッションごとの名前付きミューテックス `Local\ai-cu-input` (入力の錠) を取り、押したままのキーとボタンを離して枠を解いた後に解く (失敗と停止の後も解く)。別の ai-cu (別の AI か、人が動かしている ai-cu) が画面を変える操作の最中なら、その実行が終わるまで待つので、2 つの AI のクリックや文字が交ざらない。待つ間も 50 ミリ秒ごとに目印のファイルと MCP の取り消しを見て、止まれば何も送らずに 6。30 秒待っても取れなければ、何も送らずに 16 (`input_busy`、`error.detail.waited_ms`)。錠を待った実行は、予告の間を通るなら (枠が有効で、始めのカーソルが読めるとき)、TUI でも縮めず `lead_ms` をまるごと待つ (前の AI を止めた人の入力を見てから動く)。枠を切った設定 (`indicator.enabled = false`) と、ロック中に始まる close だけの操作列は予告の間を通らないので、待った後も人の入力を見ずに送る (人の入力は各バッチの前のマウスの監視 (6) だけが拾う)。撮影だけ、待ちだけ、`find` などの読み取り、`--dry-run`、`validate`、検証で断る実行は錠を取らず、待たない。錠は 1 回の実行の間だけ持ち、MCP の呼び出しをまたいでは持たない (`focus` → `click` → `type` を交ざらせたくなければ 1 つの `run` にまとめる)。前の持ち主が解かずに終わった (強制終了など) 錠は取れる (残ったキーは続く押されたままのキーの確認が拾う)。錠を作れない・開けない・試せない (権限の違う ai-cu が先に作った錠、同じ名前のミューテックスでない物など) ときは、何も送らずに 8。別のセッション (RDP など) の ai-cu とは排他にしない (入力の届く先が違う)。待った時間は `elapsed_ms` と `limits.max_duration_ms` に数えない
- **入力の前の画面とキーの確認**: 操作列に入力、`focus`、撮影 (`wait_stable` と `wait_change` を含む) のどれかがあれば、操作に入る前にセッションのロック (WTS) と入力デスクトップを読み、画面のロック、スクリーンセーバー、UAC などの別の画面なら、何も送らずに終了コード 14 (`desktop_unavailable`) で断る。`shot` も撮る前に同じ確認をする。入力があれば、続けて Shift、Ctrl、Alt、Win とマウスのボタンが押されたままでないかを実行の最初に 1 回だけ見て、押されていれば何も送らずに 10 (`reason: keys_held`) で断る (人の押下が入力に混ざらないように。代わりに離すことはしない)。途中のステップでは、撮影と focus の直前に同じ判定を通し (`wait_stable`、`wait_change`、`screenshot` の `settle` は撮るたびに。待つ間にロックされれば次の撮影で 14)、ロックとセーバーなら 14、判定が読めなければ 8。入力のステップは、入力デスクトップが切り替わった後 (資格情報の画面など) なら 14、ロックの幕の間は前面のガード (7) が断る。撮影を付けない `close`、`wait`、`wait_for` だけの操作列は確認しない (ロック中も閉じられる、待てる。D-066、D-067、D-068)
- **パスワード欄への入力の拒否**: キーボードのフォーカスがパスワードの要素 (UIA の IsPassword。`GetFocusedElement` で読むシステム全体のフォーカス) にあれば、`type` と `key` を既定で何も送らずに終了コード 4 (`error.rule` が `general.password_input`、`error.detail.window` は前面の窓) で断る。どの層も `general.password_input = true` のときだけ許す。順は「フォアグラウンドガード → フォーカスの要素を読む → (通すなら) 緊急停止の確認とガードをもう一度 → 送る」で、読む間に前面のウィンドウが変わったら、読み直さずに何も送らずに 7 で断る (`error.detail.window` は変わった後の窓)。UIA の待ちは 1000 ミリ秒 (`IUIAutomation2` の接続と処理の時間切れ)。`--dry-run` と `validate` では読まない。読み取りの側では、`text` と `wait_for` の文字列の条件がパスワードの要素の文字列を読まずに飛ばし、`find` の要素に `password` が出る。フォーカスの判定が時間切れ (UIA 自身の時間切れ、または 4 秒で返らない) のとき、または返らない判定が 4 本残っているときは、何も送らずに 8 で断る (D-104)。限界: 読めないアプリ (UIA を出さないアプリ、フォーカスの要素が無い、IsPassword が読めない) では通す (fail-open)。判定は各操作の最初のキーの束の前と、Tab か Enter を送った後の次のキーの束の前で行う (`type` の途中の Tab や改行でパスワード欄に移れば、その後は送らない。ただし判定は次の束の直前で、`interval_ms` (既定 20) の間にアプリがフォーカスを移し終えていなければ、移る前の要素を読んで通しうる)。アプリが自分でフォーカスを動かした場合 (Tab や Enter 以外のきっかけ) は、同じ操作の中では見ない。`click` の直後の `type` は、`click` の後の 200 ミリ秒の落ち着き待ちの後のフォーカスで判定するので、フォーカスの反映が遅いアプリでは前の要素を読んで通しうる (D-090)
- **組み込みの拒否**: `Win` を含む組み合わせ、`Alt+F4`、`Ctrl+Alt+` 系、`Ctrl+Shift+Esc`、`Ctrl+Esc` は送らない
- **撮影の制限**: 全画面の撮影は設定で許したときだけ (既定は拒否)。`shot` は人が操作中 (最後の入力から `capture.min_idle_ms` 未満) なら撮らずに終了コード 10 で断る。撮った画像には画面上の個人情報が写りうる
- **機密のウィンドウを撮らない**: どれかの層の `deny.processes` か `deny.classes` に当たるウィンドウが、撮る矩形 (`--region`、全画面、`window`。操作列の `screenshot`、一手ずつの `--shot`、MCP、TUI の実行も) に 1 ピクセルでも見えていれば、撮らずに終了コード 7 で断る (`error.detail` の `window` と `visible`)。`wait_stable`、`wait_change`、`screenshot` の `settle` は撮るたびに判定するので、待つ間に撮る矩形へ deny のウィンドウが現れても、その撮影で 7 で止まる (D-081)。判定は見えている部分で行い、Z 順でそれより前面にある不透明なウィンドウの枠の矩形を引く。リージョンが枠をまるごと覆わないウィンドウ (`SetWindowRgn` で形を変えたもの) は覆いに数えない (D-112)。前面のウィンドウに完全に覆われていれば通る。ai-cu を起動した側と TUI の端末のウィンドウは、deny に当たっても数えない (覆いとしては引く)。deny の部分を塗りつぶして撮ることはしない。撮る矩形は実行時に決まるので、`validate` と `--dry-run` では判定しない。どの層も deny のプロセスとクラスを書いていなければ、ウィンドウの列挙もしない。限界: 判定と撮影の間にウィンドウが動く競合は防がない (数ミリ秒)。`WS_EX_LAYERED` のウィンドウは、`GetLayeredWindowAttributes` で不透明 (色キーが無く、alpha が無いか 255) と読めたときだけ覆いに数えるが、`WS_EX_LAYERED` を持たずに透けるウィンドウ (DirectComposition。Electron の transparent や WinUI) は覆いとして引くので、その後ろが写りうる (`WS_EX_NOREDIRECTIONBITMAP` は不透明なウィンドウ (ブラウザ、エディタ) も持つので見分けに使えない)。形を変えたウィンドウのリージョンの座標が DPI 認識の違うウィンドウでも同じかは確かめておらず、DPI の違うモニターを 2 枚以上つないだ構成では一部だけのリージョンのウィンドウを覆いに数えて後ろが写りうる。リージョンを読めないウィンドウはリージョンが無いとみなす (D-112)。右から左のレイアウトのウィンドウは、リージョンが枠と同じでも覆いに数えない (座標の原点が右上で、測っていない。D-112)。撮影から外れるウィンドウ (`WDA_EXCLUDEFROMCAPTURE`) と、DWM の枠が取れないウィンドウは覆いに数えない。deny のアプリが応答しなくなると OS が元のウィンドウの代わりにゴーストのウィンドウ (クラス `Ghost`、題名は元の題名に「(応答なし)」) を出し、その持ち主は deny に当たらないが、画面に見えているウィンドウ (撮る矩形の外も含む) に応答しない deny のウィンドウ (`IsHungAppWindow`) が 1 つでもあれば、クラス `Ghost` のウィンドウをすべてその deny のウィンドウとして数える (ゴーストと元のウィンドウは結ばないので、ゴーストを動かしても当たる。ゴーストで当たったときは `error.detail.window` はゴースト。ゴーストが元のウィンドウの後ろに立つ並びでは、見えている元の deny のウィンドウで当たる。D-115)。限界: 元のウィンドウが隠れるか cloaked になる版の Windows では、応答しない deny のウィンドウが見えているウィンドウに入らず数えない (この PC の Windows 11 では見えたまま残った)。応答しない deny のウィンドウと deny でないアプリのゴーストが同時にあると、後者でも断る。眠りが明けた直後に応答しない印が先に消えてゴーストが残る隙間は観測されなかったが、20 ms より短い隙間は否定できない (D-115)。Windows 11 の前面のウィンドウの角の丸みの分 (推定で数ピクセル四方) は覆いとみなすので、後ろの deny のウィンドウの角が写りうる (D-077、D-112)
- **保存先の制限**: 撮影の保存先は `.png` に限り、PNG でない既存ファイル (フォルダを含む) は上書きしない。`capture.save_dirs` で保存のフォルダを絞れる。どれも撮る前に終了コード 4 で断り、書く直前にも同じハンドルで確かめ直す (D-075)。限界: `save_dirs` はパスの字面で判定し、許したフォルダの中のジャンクションやシンボリックリンクをたどらない (リンクの先が外でも書ける)
- **設定ファイルの守り**: 設定ファイルが人と AI の境界なので、AI に書き換えさせない。Claude Code では設定ファイルの `Edit` を deny にする (`ai-cu guide --setup` が例を出す)。ただし `Edit` の deny は、シェル (Bash、PowerShell) からの書き換えを止めない。AI にシェルを許すなら、設定ファイルを AI の届かない所に置く (別の利用者の持ち物にする、AI を動かす利用者の書き込みを ACL で外す) か、AI のシェルの書き込みを Claude Code の権限で絞る
- **毎回の承認 (MCP)**: 設定の `mcp.require_approval` に書いた道具は、`tools/list` で `anthropic/requiresUserInteraction` の印が付き、Claude Code はモードや allow のルールにかかわらず呼ぶたびに人の確認を出す (dontAsk のモードでは拒否)。設定が読めない間は `guide` を除くすべての道具に印を付ける。限界: 印は客への依頼で、ai-cu は `tools/call` で人が確かめたかを知れず見ない。印に対応しない客 (Codex など) は無視する。印は道具の名前ごとなので、`click` だけ書いても同じクリックを `run` で呼べば印は付かず、`shot` も一手ずつの道具の撮影や `wait_stable` には及ばない (入力の道具を書くなら `run` も書く)。設定の変更は客が一覧を取り直したとき (`/mcp` の再接続) に効く (D-078、D-101)
- **前面化の制限**: `focus` は、設定で許されたウィンドウに `SetForegroundWindow` を 1 回呼ぶだけ。復元 (`SW_RESTORE`) と前面ロックの回避策 (Alt の空打ち、`AttachThreadInput`、`AllowSetForegroundWindow`、`SystemParametersInfo`) は使わない。人が別のアプリを使っていれば OS が断り、何も送らずに終了コード 11 で止まる (タスクバーのボタンは点滅しうる)。CLI として呼ぶたびに起動されるときは、ai-cu を呼んだ端末が前面なら OS は断らないので、人が操作中 (`capture.min_idle_ms` 未満) なら前面にせずに 10 で止まる (D-050)。常駐の `ai-cu mcp` は、起動の後の人の入力で前面化の権利を失う。そこで MCP の `focus` だけは、自分の呼び出しが断られ (0)、前面が ai-cu を起動した側 (端末など) のときに限って、自分の実行ファイルを隠しのサブコマンドで窓を作らずに (`CREATE_NO_WINDOW`) 起動し、その使い捨ての子に同じ hwnd への `SetForegroundWindow` を 1 回だけ呼ばせる (子は設定もガードも読まず、hwnd が有効なトップレベルのウィンドウであることだけを確かめる。3 秒で終わらなければ kill する)。前面が人の使っている別のアプリなら起動しない (補助も断られるので、前面ロックの抜け道にはならない)。それでも断られたら `reason: refused_host_front` の 11 で、回避策は使わず、人に前面に出してもらう (D-056、D-057)。CLI と TUI の `focus` は補助を使わない
- **閉じる操作の制限**: `close` は、設定で許されたウィンドウに、題名を必須にして `WM_CLOSE` を 1 回置くだけ (`Alt+F4` は組み込みの拒否のまま)。強制終了 (`TerminateProcess`) はしない。前面で人が使っているウィンドウは閉じない (10)。閉じるのはデータを失いうるので、実入力と同じく緊急停止、人が止めた直後の待ち、開発シェルの拒否を通す (D-062)。ロック中で、カーソルが読めない close だけの操作列は、利用者のマウスの監視なしで走る (目印のファイルと最大実行時間は効く。撮影を付けた close はロック中は 14。D-066)
- **実行時ガード (開発用)**: 環境変数 `AI_CU_DEV_FORBID_REAL_INPUT` が (値を問わず) あると、`SendInput` を呼ぶ最下層、撮影の最下層、`focus` と `close` の最下層が拒否する (終了コード 9)。その環境では既定の設定の場所も読まない。このリポジトリの開発シェルでは設定していない (検証済みの release ビルドで手動確認を AI が行う運用。D-051)。debug ビルドの `mcp` は、環境変数に関係なく実入力の道具を 9 で断る (MCP のクライアントからの起動は hook に見えないため。D-055)
- **DPI の確認**: Per-Monitor V2 の DPI 認識を宣言できなければ、座標を扱う `run` と `shot` を拒否する (終了コード 8)
- **後始末**: 停止、失敗、`SendInput` が途中までしか挿入しなかった場合も、押したかもしれないキーとボタンを離す。ボタンを押したまま止まったときは、最後にガードを通った点へ戻してから離す
- **TUI からの実行**: CLI の `run` と同じ経路 (基本の層、検証、フォアグラウンドガード、緊急停止、後始末、開発シェルの拒否、DPI の確認) を通る。判定は保存済みの設定ファイルで行い、編集中の値は使わない。ai-cu 自身の端末のウィンドウが届く先なら、設定にかかわらず送らない (終了コード 7)。その端末のウィンドウを特定できなければ実行しない。実行後は端末に届いた打鍵を読み捨て、結果の画面では Enter と Esc 以外のキー (Ctrl+C を含む) を無視する。端末の無い起動では TUI 自体が起動しないので、AI の経路は広がらない
- **原子的な送信**: 修飾キーを含む組み合わせとクリックは、1 回の `SendInput` で送る

## 名前の由来

`cu` は computer use の略。本当は `cu` だけにしたかったが、Linux に同じ名前のコマンド (`cu`、シリアル回線の接続) があるので、ぶつからないよう `ai-cu` にした。

偶然、「アイ・シー・ユー」(I see you、あなたを見ている) とも読める。ちょっと怖いが、画面を見て (`shot`) 手を出す (`run`) このツールをよく表している。ICU (集中治療室) にも少しかかる。常に見張っていて、いざとなれば止める (緊急停止とガード) という意味で、そう遠くない。

アイコンなどを作るときの参考に。

## 開発者向け

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

## ライセンス

MIT (LICENSE)。不具合や要望は GitHub の Issues へ。守り (allow / deny、ガード、緊急停止など) を越えられる不具合は、公開の Issue ではなく SECURITY.md の非公開の報告へ。
