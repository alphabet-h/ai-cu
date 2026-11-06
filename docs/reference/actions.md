# 操作列 JSON (詳細)

README の「操作列 JSON」の詳細。[README に戻る](../../README.md#操作列-json)

文中の `D-xxx` は、[判断の記録](../decisions.md) の番号。

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
