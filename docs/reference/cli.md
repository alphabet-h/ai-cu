# 使い方 (詳細)

README の「使い方」の詳細。[README に戻る](../../README.md#使い方)

文中の `D-xxx` は、[判断の記録](../decisions.md) の番号。

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
  click と double-click は [--modifiers ctrl,shift,alt] で修飾キーを押したままクリックする (カンマ区切り、繰り返しも可。前面の窓も許可された窓であること。D-137)
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
- 標準出力には、結果を 1 つの JSON で出す。成功は `{"ok": true, ...}`、失敗は `{"ok": false, "error": {"code", "exit_code", "message", "hint", "action_index"}}`。`hint` は終了コードごとの次の手。同じ終了コードの中で理由を分けるときは、機械が読む `error.reason` (`focus` の 11 の `not_found` / `minimized` / `closed` / `refused_host_front` / `refused_other_app` / `refused_unknown`、MCP の 7 の `host_unknown`、`close` の 7 の `target_disabled`、10 の `keys_held` (キーかマウスのボタンが押されたまま)、10 の `lead_input` (予告の間に人の入力があった。D-093)、7 の `target_changed` (文字を打つ途中で前面が別のプロセスに変わった。途中まで送ってある。D-100)、14 の `screensaver` / `other_desktop` / `locked`) を足す。`error.detail` は理由の補足と、ガード・停止・無操作の状況 (D-065): 11 の断られた場合は `set_foreground` (0 か 1)、`foreground_process`、`foreground_is_host`、MCP の補助プロセスを使ったかの `helper` (true / false) とその結果の `helper_result` (`accepted` / `refused` / `timeout` / `failed`、使わなければ null。`failed` のときは理由が `helper_error` に入る。D-057)。7 (フォアグラウンドガード) は対象のウィンドウの `window` (`process`、`class`、`pid`、`hwnd`。入力のガードでは届く先、`focus` では前面にする対象 (前面化の後に対象が開いたダイアログが前面になって断ったときは、そのダイアログ)、`close` では閉じる対象。届く先が無ければ null) と、入力のガードのマウスでは点の `point` (`x`、`y`。修飾キーつきのクリックを前面のガードで断ったときは、前面の窓の `window` で `point` は無い。D-137)。撮る矩形に deny のウィンドウが見えていて断ったときは、そのウィンドウの `window` と、見えている部分の外接矩形の `visible` (`x`、`y`、`w`、`h`)。応答しない deny のウィンドウの代わりに OS が出したゴーストで断ったときは、`window` はゴーストのウィンドウ (`class` が `Ghost`) で、`message` の末尾にその旨が付く (D-115)。`window` で撮る対象が別のウィンドウに隠れていて断ったとき (撮影の隠れ) は、覆っている窓の `window`、隠れを見つけた点の `point` (`x`、`y`)、その窓の見えている部分の外接矩形の `visible` (deny の窓と同じ定義。枠と撮る矩形の重なりから、手前の覆いの窓を引く)、`see_through` (その窓が deny の窓の判定 (D-077) で覆いに数えない窓か。隠れの判定はこの値を見ないので、真でも 7)。`visible` は、窓の一覧が読めない、覆っている窓が一覧に無い、枠と撮る矩形が交わらない、見えている部分が残らないときに省き、`see_through` は前の 2 つのときに省く (どれでも 7 のまま。D-135)。ai-cu を起動した側の窓を撮ろうとして断った 7 は、その窓の `window` (D-135)。パスワード欄かを確かめる間に前面のウィンドウが変わった 7 は、変わった後の前面の窓の `window`。4 (パスワード欄への入力の拒否) は、最初のガードが読んだ前面の窓の `window` (7 と同じ形。D-090)。6 の利用者のマウスでの停止は `cursor` (読んだカーソル)、`from` と `expected` (直前の移動の線分)、`distance_px` (その線分からの距離)。10 (`focus` と `close`) は `idle_ms` と `min_idle_ms`、10 の `keys_held` は押されたままのキーの `keys` (`shift`、`ctrl`、`alt`、`win`) とボタンの `buttons` (`left`、`right`、`middle`、`x1`、`x2`)、10 の `lead_input` は予告の終わりに読んだ無操作の長さの `idle_ms` と、設定の予告の間の `lead_ms`。14 は入力デスクトップの名前の `input_desktop` (読めなければ null)。15 (`wait_for`、`wait_stable`、`wait_change` の時間切れ) は `timeout_ms`、`checks` (確かめた回数。画面の待ちでは撮った回数)、`unmet` (最後の判定で満たされなかった段。`wait_for` は `window` / `element` / `text`、`wait_stable` は `stable`、`wait_change` は `change`)、`window` (最後の判定で見た対象のウィンドウ。7 と同じ `process`、`class`、`pid`、`hwnd`。その判定で無ければ null。画面の待ちでは `window` で待ったときだけ値が入る)。画面の待ちの 15 には、最後に撮った矩形の `rect` (`x`、`y`、`w`、`h`) と、最後の比較の差の `last_changed_ratio` (`stable` は直前のフレームとの差、`change` は待ち始めの基準との差。比べる前なら null) も入る (D-081)。16 (別の ai-cu の操作の終わりを待ちきれなかった) は待った長さの `waited_ms` (D-113)。12 (`window_not_found`) は、撮影の対象 (`screenshot`、`shot --window`、`wait_stable` と `wait_change` の `window`) のときだけ、最小化か画面の外の窓の `window` (7 と同じ形)。最小化は許可された窓のときだけで、見つからないときは無い (D-135)。題名は入れない。設定による拒否 (4 と、許可の外れで断る 7) には、当たった設定のキーの `error.rule` と、人間向けの直し方の `error.fix` を足す (D-079)。`rule` の値は設定のキーそのもの (`general.input_enabled`、`general.capture_enabled`、`general.password_input`、`allow.actions`、`deny.actions`、`allow.keys`、`deny.keys`、`allow.regions`、`deny.regions`、`allow.processes`、`deny.processes`、`allow.classes`、`deny.classes`、`limits.max_actions`、`limits.max_duration_ms`、`limits.max_text_len`、`capture.full_screen`、`capture.save_dirs`。TUI で項目を選んだとき、右の枠の先頭に灰色で出るキーと同じ) で、組み込みのキーの拒否は `builtin`、保存先の形と既存ファイルの規則は `save_path`。値は変えない (名前を変えるときは別名を足す)。`fix` は「許可してよければ、<どこで> の <キー> <変え方>」の形の 1 文で、<変え方> はキーで変わる (allow の一覧なら「に <値> を足す」(修飾キーつきのクリックを `allow.keys` で断ったときは「を空にする」。D-137)、deny の一覧なら「から <値> を外す」、上限 (`limits.*`) なら「を <n> 以上にする」、スイッチ (`general.input_enabled`、`general.capture_enabled`、`general.password_input`、`capture.full_screen`) なら「を true にする」)。<どこで> は断った層で変わる: 基本の層 (人間の設定ファイルの層。コードと過去の判断ログでは floor / 床と呼ぶ) なら TUI (引数なしの起動。左の一覧での場所「<節> > <項目>」を添える) か基本の層の設定ファイルのパス、追加の層ならその層の設定ファイル (`--config` か `AI_CU_CONFIG`)。基本の層の設定ファイルがまだ無いときは、どの層で断っても「設定ファイルがまだ無いので何も許可されていない。端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動し、TUI で設定を作る (手順は ai-cu guide --setup)」の別の文になり、設定の場所を引けないときは `fix` を付けない。`builtin`、`save_path`、撮る矩形に deny のウィンドウが見えている 7 (D-077) にも `fix` は付けない (設定では直せないか、緩めるよう頼ませない)。AI は `fix` をそのまま人に伝え、設定を書き換えない。`validate` と `--dry-run` の `issues[]` の各項目にも同じ `rule` と `fix` が付き、最上位の `error` の 2 つは最初の問題 (`action_index` と同じ) のもの。`reason`、`detail`、`rule`、`fix` は値があるときだけ出し、既存の項目は変えない (D-056、D-079)。
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

使う側 (MCP の登録や PATH からの呼び出し) には、`~/.cargo/bin/ai-cu.exe` に入れたもの ([README の「導入」](../../README.md#導入) の `cargo install`。開発者は下の `python scripts/install.py`) を使う。`scripts/install.py` は、作業中の release を配置する開発者向けの道具で、動いている MCP サーバーがあっても配置でき (古い版は `ai-cu.old-<時刻>.exe` に改名して残り、次の配置で消える)、サーバーは再接続で新しい版になる。配置は、作業ツリーが clean で、release の印が HEAD と一致するときだけ通る (未コミットの変更、git が読めない、`--version` に印が無い (この変更より前の release)、`.dirty`、`+unknown`、HEAD と違うコミットは断り、条件ごとの次の手を出す)。順は /verify → コミット → `cargo build --release` の打ち直し → `python scripts/install.py` (D-064、D-069)。配置は「.dev/install-log.jsonl」に 1 行ずつ記録される
