# ai-cu 手動確認チェックリスト

自動テスト (`cargo test`、フィクスチャ限定の `cargo test -- --ignored`) では確かめられないことを、本物の端末と本物のアプリで確かめる手順。
TUI の手順 (5、13、32) は人が自分の Windows Terminal (PowerShell) で行う。それ以外の CLI の手順は、末尾の節のとおり AI のセッションに任せられる。
各コマンドの後に `$LASTEXITCODE` で終了コードを確かめる。実施したら、末尾の「実施の記録」に日付と結果を足す。

```powershell
$cu  = "<repo>\target\release\ai-cu.exe"
$dir = "<repo>\docs\manual-check"
```

出力の例は 2026-10-08 のもの (利用者名は `<user>` に置き換えてある)。

---

## 0. 設定を絞る (最初に必ず行う)

`allow.processes` を絞らないまま手順 6 以降を行うと、メモ帳へ切り替え損ねたときに、文字と Enter が端末に入る。
端末でコマンドとして実行されうるので、必ず先に絞る。

- [ ] `%APPDATA%\ai-cu\config.toml` を `$dir\example-config.toml` の内容に近づける (TUI でも、エディタでもよい)
  - `input_enabled = true`、`capture_enabled = true`
  - `allow.processes` に `"notepad.exe"` (確認用の `"pw-probe.exe"` や、利用者がふだん使うアプリが入っていてもよい。端末と端末を内蔵するエディタは入れない。手順 2)
  - `deny.processes` に端末 (`WindowsTerminal.exe`、`powershell.exe`、`pwsh.exe`、`cmd.exe`、`OpenConsole.exe`)
  - 端末を内蔵するエディタ (`Zed.exe`、`Code.exe` など) も deny に足す。AI のセッションから走らせると、前面はそのエディタになる

## 1. 設定の場所

```powershell
& $cu config path; $LASTEXITCODE
```
- [ ] 終了コード `0`
- [ ] `"floor":{"kind":"default","path":"C:\\Users\\...\\AppData\\Roaming\\ai-cu\\config.toml"}`
- [ ] `"dev_mode":false` (true なら、環境変数 `AI_CU_DEV_FORBID_REAL_INPUT` が残っている)

出力の例
```
{"ok":true,"paths":{"cli_overlay":null,"dev_mode":false,"edit_target":"C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml","env_overlay":null,"floor":{"kind":"default","path":"C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml"}}}
0
```

## 2. 設定の中身

```powershell
& $cu config show; $LASTEXITCODE
```
- [ ] 終了コード `0`
- [ ] `layers[0].general` が `input_enabled: true`、`capture_enabled: true`
- [ ] `layers[0].allow.processes` に `notepad` がある (`.exe` を外し、小文字にして表示する)。確認用の `pw-probe` (手順 28) や、利用者がふだん使うアプリが入っていてもよい。ただし、端末 (`windowsterminal`、`cmd`、`powershell`、`pwsh`、`openconsole` など) と端末を内蔵するエディタ (`zed`、`code` など) が allow に無い (下の出力の例は 2026-10-08 のもので、allow は notepad だけ)
- [ ] `layers[0].deny.processes` に端末 (`windowsterminal`、`cmd`、`powershell`、`pwsh`、`openconsole`) と、端末を内蔵するエディタ (`zed`、`code` など。使っているもの) が並ぶ

出力の例
```
{"layers":[{"allow":{"actions":[],"classes":[],"keys":[],"processes":["notepad"],"regions":[]},"deny":{"actions":[],"classes":[],"keys":[],"processes":["windowsterminal","cmd","powershell","pwsh","openconsole"],"regions":[]},"general":{"capture_enabled":true,"input_enabled":true},"limits":{"max_actions":1000,"max_duration_ms":600000,"max_text_len":10000},"source":"C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml","stop":{"mouse_threshold_px":50,"sentinel_file":""}}],"ok":true,"paths":{"cli_overlay":null,"dev_mode":false,"edit_target":"C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml","env_overlay":null,"floor":{"kind":"default","path":"C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml"}}}
0
```

## 3. 検証 (装置に触れない)

```powershell
& $cu validate "$dir\sample.json"; $LASTEXITCODE
```
- [ ] 終了コード `0`、出力 `{"issues":[],"ok":true}`

## 4. ドライラン (装置に触れない)

```powershell
& $cu run "$dir\sample.json" --dry-run; $LASTEXITCODE
```
- [ ] 終了コード `0`
- [ ] `"dry_run":true`、`"plan"` に 8 つの操作 (`index` 0 から 7)、`"batches"` が 1 以上
- [ ] 何も入力されない

出力の例 (`plan` は長いので途中を省略。`type` は 1 文字ごとに 1 バッチで、間に 20 ms の `sleep` が入る)
```
{"batches":61,"dry_run":true,"issues":[],"ok":true,"plan":[{"index":0,"steps":[{"ms":5000,"step":"sleep"}]},{"index":1,"steps":[{"events":[{"down":true,"event":"unicode","unit":97},{"down":false,"event":"unicode","unit":97}],"step":"batch","target":{"kind":"keyboard"}},{"ms":20,"step":"sleep"},…]},…,{"index":7,"steps":[…]}]}
0
```

## 5. TUI

Windows Terminal で、引数なしで起動する。画面は 2 ペイン (左: 見出しでまとめた項目の一覧、右: 選んだ項目の説明と編集)。

```powershell
& $cu
```
- [ ] 題名が「ai-cu 設定 — C:\...\config.toml [基本の層: 既定の場所]」。2 行目に要約 (実入力、撮影、アプリ、領域、操作、拒否)。手順 0 の値が見える
- [ ] 左で `↑↓` を 1 回押すと 1 行だけ動く (Release を無視している)。右に選んだ項目の説明と現在の値が出る
- [ ] 「アプリ (プロセス)」で `Enter` → 右が一覧の編集になる。`a` → 開いているウィンドウの候補が出て、文字を打つと絞り込まれる。`Esc` で取りやめ、`←` で左に戻る
- [ ] 「画面撮影」で `Enter` → 値が切り替わり、題名に `[未保存]`、左の項目に `●`。`u` で元に戻る
- [ ] 「実入力 (マウスとキー)」で `Enter` → 無効になる (即時)。もう一度 `Enter` → 有効にする確認が出て、`y` で戻る
- [ ] `?` → ヘルプの枠が崩れずに出る (左右の枠線が全部の行でそろっている)。何かキーで閉じる
- [ ] 変更を残したまま `q` → 「未保存の変更がある」の確認。`Esc` で戻る。`Ctrl+C` も同じ確認に回り、もう一度 `Ctrl+C` で強制終了する
- [ ] 端末の幅を 80 桁未満にする → 1 ペインになり、戻すと 2 ペインに戻る
- [ ] 終了後、端末の表示が元に戻っている (カーソルや入力の表示が崩れていない)。設定ファイルは変わっていない (保存していないので)

端末でない起動は、TUI にせずに usage を出す。

```powershell
"" | & $cu; $LASTEXITCODE
```
- [ ] 終了コード `2`。標準エラーに `Usage:` が出る

出力の例
```
ai-cu: TUI を起動しない: 標準入力と標準出力の両方が端末のときだけ TUI を起動する (stdin=端末でない, stdout=端末)
Usage: ai-cu [OPTIONS] [COMMAND]
詳しくは `ai-cu --help`。
{"error":{"action_index":null,"code":"usage","exit_code":2,"message":"TUI を起動しない: 標準入力と標準出力の両方が端末のときだけ TUI を起動する (stdin=端末でない, stdout=端末)\nUsage: ai-cu [OPTIONS] [COMMAND]\n詳しくは `ai-cu --help`。"},"ok":false}
2
```

## 6. フォアグラウンドガード (端末が前面のまま)

メモ帳には切り替えずに実行する。キーの送り先 (前面) が端末なので、拒否されるはず。

```powershell
& $cu run "$dir\sample.json"; $LASTEXITCODE
```
- [ ] 5 秒の待ちの後、終了コード `7`
- [ ] `"code":"foreground_guard"`。メッセージに、前面のプロセス (`WindowsTerminal.exe` など) と断った理由が出る。判定は、自分の端末 (D-045)、起動した側 (D-055)、設定 (`deny.processes` か `allow.processes`) の順なので、端末が起動した側の連鎖に入っていれば「は ai-cu を起動した側のウィンドウなので送らない」になる見込み (推定。下の「ホストの連鎖の目視」のとおり連鎖に入らない環境では設定の理由になる。実施したら実測で直す)
- [ ] 端末に文字が 1 つも入っていない
- [ ] 最上位に `"executed":1` (先頭の `wait` は済んだ) と `"partial":false` が出る (D-065)
- [ ] `error.detail.window` に前面のウィンドウの `process`、`class`、`pid`、`hwnd` が出て、題名 (`title`) は無い
- [ ] `hint` が `actions[0] は実行済み (executed=1)。操作ごと打ち直さない。` で始まり、`以下は失敗した actions[1] について: ` の後に、上で出た理由に応じた元の 7 の次の手が続く (起動した側なら「何も送っていない。届く先が ai-cu を起動した側…」、設定の理由ならキーのガードの次の手)

出力の例 (2026-10-15 に D-065 の形へ書き換えた。端末が起動した側の連鎖に入っている場合の推定で、`pid`、`hwnd`、`elapsed_ms` の値も推定。実施したら実測で置き換える)
```
ai-cu: [foreground_guard] actions[1]: 前面のウィンドウ (process="WindowsTerminal.exe", class="CASCADIA_HOSTING_WINDOW_CLASS", pid=92104) は ai-cu を起動した側のウィンドウなので送らない
{"elapsed_ms":5012,"error":{"action_index":1,"code":"foreground_guard","detail":{"window":{"class":"CASCADIA_HOSTING_WINDOW_CLASS","hwnd":132456,"pid":92104,"process":"WindowsTerminal.exe"}},"exit_code":7,"hint":"actions[0] は実行済み (executed=1)。操作ごと打ち直さない。残りは、以下の hint に従い、ai-cu windows や ai-cu shot で状態を見直してから送る。以下は失敗した actions[1] について: 何も送っていない。届く先が ai-cu を起動した側 (エージェント、シェル、端末) のウィンドウなので、設定にかかわらず送らない。先に focus で対象のアプリを前面にしてから送る","message":"前面のウィンドウ (process=\"WindowsTerminal.exe\", class=\"CASCADIA_HOSTING_WINDOW_CLASS\", pid=92104) は ai-cu を起動した側のウィンドウなので送らない"},"executed":1,"focused":[],"ok":false,"partial":false,"screenshots":[]}
7
```

## 7. 本番: メモ帳への入力

1. メモ帳を開いて、空のタブを用意する
2. 端末に戻って次を実行し、5 秒以内に **Alt+Tab (キーボードで)** メモ帳へ切り替える。マウスには触れない (触れると手順 8 の緊急停止が働く)
   - 画面の縁に青い枠が出てから約 3 秒 (予告の間。D-093) はキーにも触れない。その間に Alt+Tab を押すと、何も送らずに終了コード 10 (`reason: lead_input`) で止まる。枠が出て 3 秒ほど待ってから、続く 5 秒の待ちの間に Alt+Tab で切り替える (手順 8 も同じ)

```powershell
& $cu run "$dir\sample.json"; $LASTEXITCODE
```
- [ ] 終了コード `0`、`"ok":true`、`"executed":8`
- [ ] メモ帳に、次の内容が入る (1 文字でも欠けたり入れ替わったりしたら、そのまま記録する)

```
ai-cu の動作確認です。
Hello, notepad! 日本語と絵文字 😀 も入ります。
(3 行目) 
おわり
```

- [ ] 終わった後、Ctrl や Shift が押しっぱなしになっていない (普通に文字を打てる)

出力の例 (`elapsed_ms` は 5 秒の待ちに、文字ごとの 20 ms の間隔とキーの後の 50 ms が加わった値。予告の間は含めない。D-093)
```
{"elapsed_ms":6778,"executed":8,"ok":true,"screenshots":[]}
0
```

## 8. 緊急停止 (マウスを動かす)

1. 端末で次を実行し、5 秒以内に Alt+Tab でメモ帳へ切り替える (枠が出てから約 3 秒の予告の間はキーに触れない。手順 7 と同じ)
2. 「緊急停止の確認 1」が入ったら、マウスを大きく動かす

```powershell
& $cu run "$dir\sample-estop.json"; $LASTEXITCODE
```
- [ ] 終了コード `6`、`"code":"stopped"`、メッセージに「利用者がマウスを動かした」
- [ ] 「ここまで来たら停止できていない」が入っていない
- [ ] キーが押しっぱなしになっていない
- [ ] 最上位の `executed` が `action_index` と等しい (下の例では、待ちの途中で止まって 2)。`partial` は、待ちの途中で止まれば false、`type` の途中で止まれば true (D-065)
- [ ] `error.detail` に `cursor` (読んだカーソル)、`from`、`expected` (この操作列はマウスを動かさないので、実行の初めのカーソルの位置で同じ値)、`distance_px` (メッセージの px と同じ値) が出る
- [ ] `hint` が `actions[0]〜actions[1] は実行済み (executed=2)。` で始まり、`以下は失敗した actions[2] について: ` の後に停止の次の手が続く

出力の例 (2026-10-15 に D-065 の形へ書き換えた。座標と `elapsed_ms` の値は推定。実施したら実測で置き換える)
```
ai-cu: [stopped] actions[2]: 利用者がマウスを動かした (予定の位置から 100 px)
{"elapsed_ms":6120,"error":{"action_index":2,"code":"stopped","detail":{"cursor":{"x":1100,"y":800},"distance_px":100,"expected":{"x":1000,"y":800},"from":{"x":1000,"y":800}},"exit_code":6,"hint":"actions[0]〜actions[1] は実行済み (executed=2)。操作ごと打ち直さない。残りは、以下の hint に従い、ai-cu windows や ai-cu shot で状態を見直してから送る。以下は失敗した actions[2] について: 人間か停止の仕組みが実行を止めた。勝手に再実行せず、人間に確かめる","message":"利用者がマウスを動かした (予定の位置から 100 px)"},"executed":2,"focused":[],"ok":false,"partial":false,"screenshots":[]}
6
```

## 9. 緊急停止 (目印のファイル)

1. TUI か設定ファイルで、`stop.sentinel_file` に絶対パスを指定する (例 `C:\Users\<あなた>\ai-cu.STOP`)
2. そのファイルを作る

```powershell
New-Item -ItemType File "$env:USERPROFILE\ai-cu.STOP"
& $cu run "$dir\sample.json"; $LASTEXITCODE
```
- [ ] 終了コード `6`、メッセージに「停止の目印のファイルが現れた」。何も入力されない
- [ ] 枠は一瞬出てすぐ消える。予告の間 (D-093) で止まるので `error.action_index` は null で、`executed` のキーは無い (下の出力の例は D-093 より前のもので `action_index` が 0。枠が 3 秒以上前から出ていて予告を省いたときは、今も 0 になる)
- [ ] 後片付け: そのファイルを消す (消さないと、以後の実行がすべて止まる)

出力の例
```
{"error":{"action_index":0,"code":"stopped","exit_code":6,"message":"停止の目印のファイルが現れた: C:\\Users\\<user>\\ai-cu.STOP"},"ok":false}
6
```

相対パスは拒否されることも確かめる (任意)。

- [ ] `sentinel_file = "stop.txt"` にすると、`& $cu config validate` が終了コード `5` で「絶対パスで書く」と出る

出力の例
```
ai-cu: [config] C:\Users\<user>\AppData\Roaming\ai-cu\config.toml: stop.sentinel_file は絶対パスで書く (環境変数の展開はしない): "stop.txt"
{"error":{"action_index":null,"code":"config","exit_code":5,"message":"C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml: stop.sentinel_file は絶対パスで書く (環境変数の展開はしない): \"stop.txt\""},"ok":false}
5
```

## 10. deny の確認

TUI で `deny.keys` に `enter` を足して保存する。

```powershell
& $cu validate "$dir\sample.json"; $LASTEXITCODE
```
- [ ] 終了コード `4`、`deny.keys` に当たった旨が出る (文字の改行と `key: enter` の両方)
- [ ] 後片付け: `deny.keys` から `enter` を消す。設定のコメントが保存の後も残り、`config.toml.bak` ができている

出力の例
```
ai-cu: [policy_denied] actions[1]: キー enter は deny.keys に当たる: C:\Users\<user>\AppData\Roaming\ai-cu\config.toml
ai-cu: [policy_denied] actions[6]: キー enter は deny.keys に当たる: C:\Users\<user>\AppData\Roaming\ai-cu\config.toml
{"error":{"action_index":1,"code":"policy_denied","exit_code":4,"message":"2 件の問題"},"issues":[{"action_index":1,"code":"policy_denied","message":"キー enter は deny.keys に当たる: C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml"},{"action_index":6,"code":"policy_denied","message":"キー enter は deny.keys に当たる: C:\\Users\\<user>\\AppData\\Roaming\\ai-cu\\config.toml"}],"ok":false}
4
```

## 11. 組み込みの拒否 (設定では緩められない)

```powershell
'{"version":1,"actions":[{"type":"key","keys":"alt+f4"}]}' | & $cu validate -; $LASTEXITCODE
```
- [ ] 終了コード `4`、「Alt+F4 は組み込みで拒否する」

出力の例
```
ai-cu: [policy_denied] actions[0]: alt+f4: Alt+F4 は組み込みで拒否する
{"error":{"action_index":0,"code":"policy_denied","exit_code":4,"message":"1 件の問題"},"issues":[{"action_index":0,"code":"policy_denied","message":"alt+f4: Alt+F4 は組み込みで拒否する"}],"ok":false}
4
```

## 12. 撮影

Enter の打鍵が「最後の入力」になるので、既定の `capture.min_idle_ms` (3000) を超えるよう 4 秒待ってから撮る (手順 14 で、待たないと 10 になることを確かめる)。

```powershell
Start-Sleep -Seconds 4; & $cu shot --out "$env:TEMP\ai-cu-shot.png" --region 0,0,800,600; $LASTEXITCODE
```
- [ ] 終了コード `0`、`"width":800`、`"height":600`、`"scale":1`
- [ ] 画像を自分で開いて、画面の左上 800x600 (物理ピクセル) が写っている
- [ ] 後片付け: 画像には画面の内容が写るので、確かめたら消す

出力の例
```
{"height":600,"ok":true,"path":"C:\\Users\\<USER>~1\\AppData\\Local\\Temp\\ai-cu-shot.png","scale":1,"width":800,"x":0,"y":0}
0
```

## 13. TUI からの実行 (r キー)

事前に手順 0 の設定 (メモ帳を許可し、端末を deny) を保存しておく。未保存の変更があると `r` は断られる。
`sample.json` のある場所で、Windows Terminal から引数なしで起動する。

```powershell
Set-Location $dir
& $cu
```
- [ ] 最下行のキーの案内に `r 実行` が出る。`?` か `F1` のヘルプに「操作列の実行」の行がある
- [ ] `r` → 右ペインが「操作列を実行」になり、`sample.json` と `sample-estop.json` が一覧に出る。文字を打つと絞り込まれる
- [ ] `↓` で `sample.json` を選んで Enter → 「実行の確認」の画面。操作数、判定に使う設定 (config.toml のパス)、停止の手段、最初の操作の注意が出る。枠が崩れていない
- [ ] `Esc` で取りやめ、`r` → 入力欄に前回のパスが入っていて、一覧も全件出る。行 0 で Enter すると同じファイルを再検査する
- [ ] 確認の画面で `y` → 「実行まで 5 秒」のカウントダウン。`x` などのキーでは取り消されず、`Esc` で取りやめられる (メッセージの行に「取りやめた (何も送っていない)」)

端末を前面にしたまま走らせる (自分の端末への送信の拒否)。

- [ ] `r` → `sample.json` → Enter → `y` → 何もしないで 5 秒待つ
- [ ] 「実行に失敗」の画面に `[foreground_guard] (終了コード 7)` と「ai-cu を動かしている端末なので送らない」が出て、末尾に `Enter / Esc 閉じる` が見える
- [ ] 端末に文字が 1 つも入っていない。`q` や `s` を押しても画面は閉じない (Enter か Esc で閉じる)

本番: メモ帳へ入力する。

1. メモ帳を開いて、空のタブを用意する
2. TUI で `r` → `sample.json` → Enter → `y` → カウントダウンの間に **Alt+Tab でメモ帳へ切り替える**。マウスには触れない

- [ ] 「実行した」の画面に `✓ 成功 (終了コード 0): 8 操作` が出る。メモ帳に手順 7 と同じ内容が入る
- [ ] 画面を閉じた後も TUI は開いたまま (設定の編集に戻れる)。`q` で終了し、端末の表示が崩れていない

緊急停止: `sample-estop.json` で同じ流れを行い、「緊急停止の確認 1」が入ったらマウスを大きく動かす。

- [ ] 「実行に失敗」の画面に `[stopped] (終了コード 6)` と「マウスを動かした」が出る

未保存の変更があるとき。

- [ ] 何かの項目を変えて (題名に `[未保存]`)、`r` → メッセージの行に「未保存の変更があるので実行しない」と出て、一覧は開かない。`u` で戻すと `r` が使える

端末でない起動からは到達しない (手順 5 と同じ)。

```powershell
"r`ny`n" | & $cu; $LASTEXITCODE
```
- [ ] 終了コード `2`。TUI も実行も始まらない

## 14. 撮影の制限 (全画面の禁止と、操作中の拒否)

手順 0 の設定 (`[capture]` の節が無い = 既定の `full_screen = false`、`min_idle_ms = 3000`) のまま行う。

```powershell
& $cu shot --out "$env:TEMP\ai-cu-shot.png"; $LASTEXITCODE
```
- [ ] 終了コード `4`、`"code":"policy_denied"`、メッセージに `capture.full_screen = false`。画像はできない

Enter を押した直後 (1 秒以内) に撮る (`;` で続けて実行するので、Enter の打鍵が「最後の入力」になる)。

```powershell
& $cu shot --out "$env:TEMP\ai-cu-shot.png" --region 0,0,800,600; $LASTEXITCODE
```
- [ ] 終了コード `10`、`"code":"user_active"`、メッセージに「最後の入力から N ms」(N は 3000 未満) と `capture.min_idle_ms`。`hint` に「数秒待って」
- [ ] 同じコマンドを、Enter の後に手を離して 4 秒以上待ってから走らせる (`Start-Sleep 4; & $cu shot ...` の形) → 終了コード `0`。後片付けで画像を消す

TUI で確かめる。

- [ ] 左の一覧の末尾に「撮影の制限」の節があり、「全画面の撮影」が無効、「撮影前の無操作 (ms)」が 3000
- [ ] 「全画面の撮影」で `Enter` → 有効にする確認が出る。`y` → 題名に `[未保存]`。`s` → 保存の確認に「⚠ 全画面の撮影を有効にする」が並ぶ。`Esc` で取りやめ、`u` で戻す

## 15. focus 操作 (隠れたメモ帳を前面にする)

手順 0 の設定のまま。`sample-focus.json` (先頭に `focus`、続けて 1 行の入力) を使う。
メモ帳を開いて空のタブを用意し、ブラウザなど別のアプリを最大化してメモ帳を隠す。端末 (Windows Terminal) を前面に戻してから走らせる。マウスには触れない。
`focus` は `shot` と同じ無操作の判定 (既定 3 秒) を通るので、Enter の後は 4 秒待ってから走る形にする。

```powershell
& $cu run (Join-Path $dir sample-focus.json); $LASTEXITCODE
```
- [ ] Enter の直後に走るので、終了コード `10`、`"code":"user_active"`、メッセージに「前面にしない」。メモ帳は前面にならず、何も入らない

```powershell
Start-Sleep -Seconds 4; & $cu run (Join-Path $dir sample-focus.json); $LASTEXITCODE
```
- [ ] 終了コード `0`、`"executed":2`、`"focused"` にメモ帳の `process` と `title`。メモ帳が前面になり、「focus で前面にした」の 1 行が入る (端末が前面のままでも効く)。11 になったらそのまま記録する (Windows Terminal からの起動で前面にできるかは、この手順で初めて測る。D-050 の根拠は Zed の端末で測ったもの)

別のアプリを前面にした状態で走らせる (Enter の直後に Alt+Tab でブラウザへ切り替え、4 秒待つ)。

```powershell
Start-Sleep -Seconds 4; & $cu run (Join-Path $dir sample-focus.json); $LASTEXITCODE
```
- [ ] 終了コード `11`、`"code":"focus_failed"`、メッセージに「前面にできなかった」と `SetForegroundWindow=0`、前面のプロセス名 (ブラウザ)。`hint` に「人間に対象を前面に出してもらう」。メモ帳に何も入らない。ブラウザは前面のまま (タスクバーのメモ帳のボタンが点滅していたら、それも記録する)

メモ帳を最小化して、端末を前面のまま走らせる。

```powershell
Start-Sleep -Seconds 4; & $cu run (Join-Path $dir sample-focus.json); $LASTEXITCODE
```
- [ ] 終了コード `11`、メッセージに「最小化されている」。メモ帳は最小化のまま (復元されない)

事前検査: `--dry-run` で `focus` が手順に出る。

```powershell
& $cu run (Join-Path $dir sample-focus.json) --dry-run; $LASTEXITCODE
```
- [ ] 終了コード `0`、手順に `"step":"focus"`。装置には触れない (メモ帳は動かない)

---

## 16. 一手ずつの操作 (サブコマンドと --shot)

手順 0 の設定のまま。メモ帳を開いて空のタブを用意し、端末を前面に戻しておく。`$r` は `windows` の `rect` を `X,Y,W,H` にしたもの。

```powershell
& $cu windows; $LASTEXITCODE
& $cu focus notepad.exe --shot "$env:TEMP\step.png" --region $r; $LASTEXITCODE
```
- [ ] 終了コード `0`、`"executed":3` (focus、待ち、撮影)、`focused` にメモ帳、`screenshots[0]` に png の情報。メモ帳が前面になり、png にメモ帳が写る (直後でも 10 にならない)

```powershell
& $cu type "一手ずつ" --shot "$env:TEMP\step.png" --region $r; $LASTEXITCODE
& $cu key enter --shot "$env:TEMP\step.png" --region $r --settle-ms 0; $LASTEXITCODE
```
- [ ] どちらも終了コード `0`。`executed` は type が `3` (既定の待ち 300 ms を挟む)、key が `2` (`--settle-ms 0` なので待ちが無い)。`focused` は空。メモ帳に「一手ずつ」と改行が入り、png にそれが写る

```powershell
& $cu click 10 10 --dry-run; $LASTEXITCODE
& $cu key enter --region 0,0,1,1; $LASTEXITCODE
```
- [ ] dry-run は終了コード `0`、`"dry_run":true`、`"plan"` に 1 操作。`--shot` の無い `--region` は使い方の誤り (終了コード `2`)

```powershell
& $cu key enter; $LASTEXITCODE
```
- [ ] 端末が前面のときは終了コード `7` (`foreground_guard`)。端末に改行が入らない (端末が deny に入っている前提)

## 17. 撮影の対象をウィンドウで指定 (--window) と縮尺 (--scale)

手順 0 の設定のまま。メモ帳を開いておく (前面でなくてよい)。

```powershell
& $cu shot --out "$env:TEMP\win.png" --window notepad.exe; $LASTEXITCODE
```
- [ ] 終了コード `0`。`x` `y` `width` `height` が `windows` の `rect` と一致、`scale` が `1.0`、`window` にメモ帳の `process` と `title`。png にメモ帳だけが写る

```powershell
& $cu shot --out "$env:TEMP\win.png" --window notepad.exe --scale 0.5; $LASTEXITCODE
```
- [ ] 終了コード `0`。`width` と `height` が半分 (四捨五入)、`scale` が `0.5`。png が縮んでいて文字は読める

```powershell
& $cu shot --out "$env:TEMP\win.png" --window notepad.exe --window-title zzz; $LASTEXITCODE
& $cu shot --out "$env:TEMP\win.png" --window notepad.exe --region 0,0,1,1; $LASTEXITCODE
```
- [ ] 題名が一致しなければ終了コード `12`、`"code":"window_not_found"`、メッセージに process と title。png は更新されない
- [ ] `--window` と `--region` の併用は使い方の誤り (終了コード `2`)

端末 (Windows Terminal か Zed) をメモ帳の上に重ねて、端末を前面のまま:

```powershell
& $cu shot --out "$env:TEMP\win.png" --window notepad.exe; $LASTEXITCODE
```
- [ ] 終了コード `7`、`"code":"foreground_guard"`、メッセージに「隠れている」と端末のプロセス名。`hint` に「先に focus」。png は更新されない (端末の中身が写り込まない)

メモ帳を最小化してから (終わったら復元する):

```powershell
& $cu shot --out "$env:TEMP\win.png" --window notepad.exe; $LASTEXITCODE
```
- [ ] 終了コード `12`、メッセージに「最小化」。メモ帳は復元されない

端末を前面にして (メモ帳は復元済み):

```powershell
& $cu focus notepad.exe --shot "$env:TEMP\win.png" --window notepad.exe; $LASTEXITCODE
```
- [ ] 終了コード `0`、`"executed":3`、`focused` にメモ帳、`screenshots[0].window` にメモ帳。メモ帳が前面になり、png にメモ帳が写る

## 18. 要素を探す (find) と文字列を読む (text)

手順 0 の設定のまま。メモ帳を開いて何か (例「一手ずつ」と改行) を入れておく。前面でなくてよい。

```powershell
& $cu find --window notepad.exe --type button; $LASTEXITCODE
```
- [ ] 終了コード `0`。`window` にメモ帳、`elements` にボタン (名前は表示言語に依る。例「閉じる」「最大化」「ファイル」)。各要素に `rect` と `center` (物理ピクセル。`windows` の `rect` の中に収まる)、`enabled`、`offscreen`。`truncated` は false

```powershell
& $cu find --window notepad.exe --name "テキスト"; $LASTEXITCODE
& $cu text --window notepad.exe --type document; $LASTEXITCODE
```
- [ ] `find` は終了コード `0` で、本文の要素 (`type` が `document`) が出る
- [ ] `text` は終了コード `0`、`text` にメモ帳の本文 (改行は LF)、`element.type` が `document`

```powershell
& $cu text --window notepad.exe --type button --name "zzz"; $LASTEXITCODE
& $cu find --window notepad.exe --type bogus; $LASTEXITCODE
& $cu find --window chrome.exe; $LASTEXITCODE
```
- [ ] 一致する要素が無ければ終了コード `13`、`"code":"element_not_found"`
- [ ] 未知の種類は使い方の誤り (終了コード `2`、メッセージに使える種類の一覧)
- [ ] 許可されていないプロセスは終了コード `4` (`chrome.exe` が allow に無いとき)

端末をメモ帳の上に重ねたまま:
```powershell
& $cu find --window notepad.exe --type document; $LASTEXITCODE
```
- [ ] 隠れていても終了コード `0` で要素が出る (読むだけなので撮影の重なりの確認は無い)

automation_id、フォーカス、状態 (D-089)。(a) はメモ帳が前面でないまま (端末を重ねたまま) 打つ:
```powershell
& $cu find --window notepad.exe --limit 200; $LASTEXITCODE
& $cu find --window notepad.exe --focused; $LASTEXITCODE
```
- [ ] (a) 1 つ目は終了コード `0`。各要素に `focused` (bool) があり、少なくとも 1 要素に `toggle` / `selected` / `expanded` のどれかが出る (メモ帳のどの要素がどのパターンを持つかは未確認)。どの要素にも出なければ、実装置の状態の経路を確かめられていないので、出た JSON を記録して D-089 に追記し、この項目を未完として残す
- [ ] (a) 2 つ目は終了コード `0` で `elements` が空 (`ok: true`)。空でなければ出た要素を記録し、「ふつう前面の窓だけ」の文面を見直して D-089 に追記する
- [ ] (a) 1 つ目で `automation_id` が空でない要素を 1 つ選び、`& $cu find --window notepad.exe --automation-id <その値>; $LASTEXITCODE` がその要素だけを返す。`--automation-id ""` は終了コード `2`

(b) はメモ帳を前面にしてから (前面化を含むので GO の範囲で行う。11 になったときの扱いは手順 23 のとおり):
```powershell
& $cu focus notepad.exe; $LASTEXITCODE
& $cu windows; $LASTEXITCODE
& $cu find --window notepad.exe --focused; $LASTEXITCODE
```
- [ ] `windows` でメモ帳の `foreground` が true。`find --focused` は終了コード `0` で、本文の要素 (`document`、`focused: true`) を 1 件返すかを見る (フォーカスを持つ要素がどれかは未確認。別の要素が返れば出た JSON を記録して D-089 に追記する。前面の窓で 0 件なら記録し、文面を見直して D-089 に追記する)

## 19. MCP サーバーモード (ai-cu mcp)

手順 0 の設定のまま。メモ帳を開いて何か (例「一手ずつ」と改行) を入れておく。2 つの経路がある。

**A. MCP の客として (Claude Code に登録する。人が行う)**: 別の作業用のプロジェクトで
`claude mcp add ai-cu -- <repo>\target\release\ai-cu.exe mcp --log <repo>\target\manual-check\mcp.log` を実行し (利用者の設定ファイルが変わるので人が行う。登録済みなら `claude mcp remove ai-cu` してから。`--log` は取り消しの受信を見るため。D-061)、
そのプロジェクトで Claude Code を起動して `/mcp` で ai-cu が接続済みと出ることを見る。そのセッションに次を頼む (GO をもらってから):
- [ ] `guide` → 手引きが返る。`windows` → メモ帳が出て、端末 (Claude Code を動かしている WindowsTerminal.exe など) は出ない
- [ ] `focus` (notepad.exe、`shot` に window) → `focused` にメモ帳、画像が会話に出る (端末が前面でも隠れたメモ帳が前面になる = 孫プロセスからの SetForegroundWindow)
- [ ] `type`「MCP から」+ 改行 → `text` でその文字列が読める
- [ ] `run` に `wait` 20000 ミリ秒、`focus` (notepad.exe)、`type` の順で呼び、「Called ai-cu」が出てから 10 秒ほど後に Esc → `target/manual-check/mcp.log` に「tools/call run は取り消された」(時刻が Esc を押した頃)、メモ帳は前面にならず何も入らない。`focus` を待ちより前に置かない (メモ帳が前面になり、Esc が Claude Code に届かない)。早く押しすぎると、送る前の段階 (権限の判定と推定) ごと取りやめになり、ログに 1 行も出ない
- [ ] 端末 (Claude Code の端末) が前面のまま `key` → 7 (`hint` が「起動した側」)
- [ ] Claude Code を終了して `tasklist /FI "IMAGENAME eq ai-cu.exe"` に残っていない (数秒以内)
- [ ] 補助プロセス (D-057): サーバーの起動の後に人が端末で何か打ってから (頼みごとを打てば足りる。サーバーが前面化の権利を失う)、端末が前面のまま `focus` (notepad.exe) → 成功して `focused` にメモ帳、その `helper` が true (補助を通った。直接で足りたなら false)。窓が一瞬出ないこと、`tasklist` に `ai-cu.exe` の補助が残らないことも見る。失敗なら 11 の `error.detail` の `helper`、`helper_result`、`helper_error` を記録する
- [ ] 補助が抜け道にならない: 人が Chrome などを前面にしてから `focus` → 11 (`refused_other_app`、`detail.helper` が false)、前面は Chrome のまま
- [ ] Esc の取り消しは、マウスに触らずキーボードだけで押す (マウスを動かすと先に停止 (6) が効く)
- [ ] close (D-062): 人がメモ帳で Ctrl+Shift+N を押して新しい**ウィンドウ** (タブではない) を開き、何も入れずに置く (この PC の題名は「タイトルなし - メモ帳」。入力すると 1 行目が題名になる。他に同じ題名のウィンドウが無いこと)。端末が前面のまま `close` (notepad.exe、title「タイトルなし」) → `closed[0].gone` が true、そのウィンドウだけが消え、元のメモ帳は残る。前面は端末のまま、文字は何も入らない。`title` を省くと道具の引数の誤りで、何も閉じない
- [ ] close の保存の確認: 同じように新しいウィンドウを開いて「消してよい」と打ち、端末に戻って `close` (title「消してよい」。題名は 1 行目になる) → `gone` が false (保存の確認は Zed などの後ろで出て、前面は変わらない)。確認は人が「キャンセル」で閉じ、そのウィンドウは人が片付ける (AI は確認のボタンを押さない)

**B. プロトコルの確認 (AI のセッションから、release のパスを引数に渡す)**: 読み取りの流れは GO 無しでよい。
hook は python の客の引数にある ai-cu のパスを起動として見るので、`$cu` のような変数ではなく release のパスを文字どおり書く
(変数だと起動先が分からないとして止まる。debug のパスは opt-in しても止まる)。Bash ツールから送るときはパスをスラッシュ区切りで書く
(`\\` を含む文字列は照合に合わず止まる)。
```powershell
python <repo>\docs\manual-check\mcp_client.py <repo>\target\release\ai-cu.exe mcp -- <repo>\docs\manual-check\mcp-requests.jsonl --save-images <repo>\target\manual-check
```
- [ ] 標準エラーの最初の行に `host: [...]` (python.exe、シェル、端末が並ぶ)
- [ ] id 1 (`server/discover`): `supportedVersions` が `["2026-07-28"]`、`resultType` が `complete`、`ttlMs` と `cacheScope` がある
- [ ] id 2 (`tools/list`): 道具が 19 個
- [ ] id 3 (`config`): 基本の層 (`paths.floor.kind`) が `default` で `input_enabled` が true。id 4 (`windows`): メモ帳が出て端末は出ない (`other_windows` に数えられる)
- [ ] id 5、6 (`find`、`text`): 本文の要素と文字列。id 7 (`shot`): `isError` が false で `image` が 1 つ (保存した png にメモ帳だけが写る。隠れていれば 7)
- [ ] id 8 (`click` の dry_run): `dry_run` true、`plan` がある。id 9 (`alt+f4`): `isError` true で `exit_code` 4。id 10 (`ping`): `{}`
- [ ] `server exit=0`

実入力の流れ (GO の後。メモ帳を開き、端末が前面のままでよい):
```powershell
python <repo>\docs\manual-check\mcp_client.py <repo>\target\release\ai-cu.exe mcp -- <repo>\docs\manual-check\mcp-requests-input.jsonl --timeout 30
```
- [ ] id 2 (`focus` + shot): `focused` にメモ帳、`screenshots` に 1 枚。id 4 (`type`): `executed` 3、id 5 (`text`): 本文に「MCP から」
- [ ] id 6 (`run` の 8 秒の `wait`): 待ちの間に人がマウスを大きく動かす → `exit_code` 6 (`stopped`)。直後の id 7 (`key`): 人がまだ動いていれば 10 (cooldown)、3 秒静止していれば 0
- [ ] `server exit=0`。メモ帳の「MCP から」は ctrl+z か手で消す

debug ビルド (`target/debug/ai-cu.exe mcp`) では、実入力の道具が `exit_code` 9 (`debug ビルド`) になり、読み取りと dry_run は通る。

ホストの連鎖の目視 (win32 レビューの中-2): 標準エラーの `host: [...]` に、ai-cu を動かしている端末 (WindowsTerminal.exe、Zed.exe など) が入っているかを見る。
既定の端末が Windows Terminal の環境で cmd.exe をスタートから起動した場合 (WT が COM のハンドオフで起動され、親が svchost になる) と、WSL から起動した場合は、端末が連鎖に入らない。そのときは設定の `deny.processes` の端末が最後の守りになる (`windows` の `foreground.allowed` が false になることで確かめる)。

## 20. ロック中の run と close (D-066、D-067)

前提: `AI_CU_MANUAL_CHECK=1` のセッションで、利用者の GO を受けてから、release の `target/release/ai-cu.exe` で行う
(opt-in の無いセッションでは、ガード hook が止める)。入力デスクトップの判定は実行の最初に掛かるので、操作列の先頭の `wait` では遅らせられない。
ロックの後に起動するため、AI は Bash ツールの `run_in_background` で、**コマンド文字列そのものに** 各起動を直接書く
(ガード hook が各起動を release の起動として照合できるように。中身の見えないスクリプトから起動しない。process は位置引数):

```
sleep 60; target/release/ai-cu.exe run target/manual-check/lock_click.json > target/manual-check/lock_run.json; echo $? > target/manual-check/lock_run.code; target/release/ai-cu.exe close notepad.exe --title タイトルなし > target/manual-check/lock_close.json; echo $? > target/manual-check/lock_close.code
```

準備:
- (a) 手順 0 の設定になっていることを確かめる (allow に notepad があり、端末と端末を内蔵するエディタが allow に無く deny にある。pw-probe や利用者がふだん使うアプリが allow にあってもよい。手順 2)
- (b) 人が Ctrl+Shift+N で空の新しいメモ帳のウィンドウを開く。題名「タイトルなし」のウィンドウが他に無いことを `windows` で確かめる (手順 19 の close と同じ。中身があると保存の確認が出て `gone: false` になる。D-062)
- (c) `target/manual-check/lock_click.json` は、先頭に `focus` (notepad.exe、title「タイトルなし」) を置き、`windows` で取ったそのウィンドウの rect の中の本文の点を 1 回クリックするだけの操作列にする
- (d) ロックが 60 秒に間に合わなかったら、結果を捨てて最初からやり直す (その間も人は手を離す)

順序:
- [ ] (1) AI が上のコマンドを背景で起動し、「60 秒以内にロックし、2 分たつまで解除しない。その間は手を離す」と伝える
- [ ] (2) 人が Win+L でロックする。(3) 2 分たってから人が解除する
- [ ] (4) AI が出力のファイルを読む: `lock_run.code` が 14、`lock_run.json` の `error.code` が `desktop_unavailable`、`error.reason` が `locked`、`executed` が無い (何も送っていない)。`lock_close.code` が 0 で、`closed[0].gone` が true
- [ ] (4') AI が `lock_run.json` の `error.detail.input_desktop` の値を記録する。`Default` なら、ロックの幕 (LockApp.exe) の間は入力デスクトップが Default のままという推定のとおり。結果が出たら docs/design-notes.md の Windows の入力の節の「推定」を実測に直す
- [ ] (5) ロックの無い画面で: (4) の close で閉じたので、新しい空のメモ帳を開き直し、`windows` で lock_click.json の点を取り直してから `run` → 0 で、今までどおりクリックが届く

押されたままのキーの確認 (10 `keys_held`) は、この手順に入れない (人がキーを押したまま実入力を送ると、「手を離す」と食い違う)。
実機テスト `fixture_real_injected_shift_is_seen_by_keys_held_and_run_refuses` (注入した Shift の押下) で代える。

## 21. 条件がそろうまで待つ (wait-for、D-068)

手順 0 の設定のまま。メモ帳を開いて「一手ずつ」を入れておく (人が入力を送らずに済ませるなら、「一手ずつ」を書いたファイルを `notepad <パス>` で開けばよい。題名はファイル名になる)。前面でなくてよい。
allow に別のアプリが余分に入っていても、端末とエディタが deny にあり、条件に使う実行ファイルが notepad.exe だけなら続けてよい (手順 20 と 21 はそれで実施した)。
`wait-for` は読むだけで入力を送らないが、(6) の `close` と同じ実機の一連の手順なので、手順 20 と同じく `AI_CU_MANUAL_CHECK=1` のセッションで
release のパスを文字どおり書き、GO をもらってから行う (この形なら、ガード hook の READ_ONLY に `wait-for` が入っているかに依らず通る)。

```powershell
& $cu wait-for notepad.exe --name "テキスト" --timeout-ms 3000; $LASTEXITCODE
& $cu wait-for notepad.exe --text "一手ずつ" --type document --timeout-ms 3000; $LASTEXITCODE
```
- [ ] (1) 終了コード `0`、`"executed":1`、`waited[0].window` にメモ帳 (`process`、`title`)、`waited[0].element` に本文の要素 (`type` が `document` か `text`) と `center`、`checks` が 1、`elapsed_ms` が小さい (待たずに最初の判定で満たされる)
- [ ] (2) 終了コード `0`、`waited[0].element.type` が `document`。`element` に `text` のキーは無い (本文そのものは結果に入れない)

```powershell
& $cu wait-for notepad.exe --title zzz --timeout-ms 1000; $LASTEXITCODE
& $cu wait-for notepad.exe --name zzz --timeout-ms 1000; $LASTEXITCODE
& $cu wait-for chrome.exe --timeout-ms 1000; $LASTEXITCODE
```
- [ ] (3) 終了コード `15`、`"code":"wait_timeout"`、`elapsed_ms` が 1000 以上、`error.detail.unmet` が `window`、`error.detail.window` が null。`error.detail.checks` は 5 (250 ms ごとに t = 0、250、500、750 と、期限の 1000 の時点で最後に 1 回)。`hint` に「同じ条件で繰り返し待たない」
- [ ] (4) 終了コード `15`、`unmet` が `element`、`error.detail.window` にメモ帳の `process`、`class`、`pid`、`hwnd` (題名は無い)。`checks` は 4 か 5 (判定は「読み取り → 250 ms の待ち」の繰り返しで、読み取りの時間は待ちから引かない。4 回の読み取りの合計が 250 ms を超えると、期限までに 5 回目が入らず 4 になる。この PC のメモ帳では 1 回の読み取りが 89〜176 ms で 4 だった)
  (via: 2026-10-16 に `target/release/ai-cu.exe wait-for notepad.exe --title zzz --timeout-ms 1000` が `checks` 5 で `elapsed_ms` 1004、`--name zzz --timeout-ms 1000` が `checks` 4 で `elapsed_ms` 1093、`--name テキスト --timeout-ms 3000` と `--text 一手ずつ --type document --timeout-ms 3000` が `checks` 1 で `elapsed_ms` 176 と 89 を返した)
- [ ] (5) 終了コード `4` (`policy_denied`) で即時 (`chrome.exe` が allow に無いとき。出力に `issues` があり、`elapsed_ms` と `executed` が無い = validate の段で止まり、待ちの輪に入らない)

`--absent` (ウィンドウが消えるまで待つ)。準備は手順 20 の (b) と同じ文: GO の前に人が Ctrl+Shift+N で空の新しいメモ帳の**ウィンドウ** (タブではない) を開き、
題名「タイトルなし」のウィンドウが他に無いことを `windows` で確かめる。以後は手を離す。`windows` にメモ帳が 1 つも無ければ、AI が `Start-Process notepad` で開いてよい (手順 20 の (b) も同じ。2026-10-15 と 10-16 はそれで実施した)
(メモ帳が開いている状態で `Start-Process notepad` すると、既存のウィンドウにタブが足される / 前回のタブを復元する挙動があり、`close` が「一手ずつ」の入ったウィンドウに当たって保存の確認で `gone: false` になる)。
AI は Bash ツールの `run_in_background` で次を始め、別の呼び出しで `close` を打つ (どちらも入力を送らない。手順 20 と同じく、コマンド文字列そのものに release の起動を直接書く):

```
target/release/ai-cu.exe wait-for notepad.exe --title タイトルなし --absent --timeout-ms 20000 > target/manual-check/wait_absent.json; echo $? > target/manual-check/wait_absent.code
```
```
target/release/ai-cu.exe close notepad.exe --title タイトルなし
```
- [ ] (6) `close` は終了コード `0` で `closed[0].gone` が true。`wait_absent.code` が 0、`wait_absent.json` の `waited[0]` に `window` キーが無い (ウィンドウ自身が消えた)、`elapsed_ms` が `close` までの時間 (20000 未満)

## 22. 隠れた許可済みのアプリ (windows の hidden_allowed、D-074)

手順 0 の設定のまま。役の分け方: AI は読むだけのコマンド (`config show`、`windows`) とメモ帳の起動 (`Start-Process notepad`) だけを行い、入力は送らない。
仮想デスクトップの作成、メモ帳の送り出し、片付け (タスクビュー) と、窓をトレイから出す・隠す操作は人が行う (Win+ は ai-cu の組み込みの拒否で AI は送れない)。
人は端末のあるデスクトップから移らない (移ると AI の合図も返事もできない)。別のデスクトップへ送るのはメモ帳だけにする。
GO は最初に 1 回もらい、人の操作は AI の合図で行い、AI が `windows` を打つ間は人が手を離す。`AI_CU_MANUAL_CHECK=1` のセッションで、release のパスを文字どおり書く。

```powershell
& $cu config show
```
- [ ] (0) 基本の層の `allow.processes` に notepad がある (他の名前があってよい)。無ければ以下を行わずに止まる

```powershell
& $cu windows; $LASTEXITCODE
```
- [ ] (1) メモ帳を開いた状態 (無ければ AI が `Start-Process notepad`) で: 終了コード `0`、`windows` にメモ帳が出て、`hidden_allowed` に `notepad.exe` が無い
- [ ] (2) 人が Win+Tab のタスクビューで「新しいデスクトップ」を作り、メモ帳を右クリック →「移動先」→ 新しいデスクトップへ送り (端末のあるデスクトップは選んだまま)、Esc で閉じた後、同じコマンドで: `windows` にメモ帳が無く、`hidden_allowed` に `notepad.exe` がある。`windows` が空なら `hint` が隠れている側の文 (「hidden_allowed のプロセスは動いているが、ウィンドウが隠れている」)。空でなければ `hint` のキーが無い
- [ ] (3) 人が Win+Tab で新しいデスクトップを × で閉じる (中の窓は残ったデスクトップに戻る)。同じコマンドで (1) と同じ結果に戻る
- [ ] (4) トレイに隠れるアプリ (利用者のアプリなど) が動いていて基本の層の allow にあれば必ず: 人がトレイから主ウィンドウを出した後 (アイコンのメニューやホバーは閉じる) は一覧に出て `hidden_allowed` に無く、窓をトレイに隠した後は同じコマンドで `hidden_allowed` にその実行ファイル名がある (D-074 の推定「トレイのアプリは隠し窓を持つ」をここで確かめる。無ければ D-074 の覆る条件として記録する)
- [ ] (5) 任意: 基本の層の allow に CalculatorApp.exe があれば (足すのも電卓を開くのも人が行う。AI は設定を書き換えない)、電卓が見えている間に `windows` を呼び、`hidden_allowed` に CalculatorApp.exe が出るかを記録する (出れば D-074 の覆る条件。ストアのアプリは見える枠の持ち主が ApplicationFrameHost.exe。win32-reviewer の指摘)
- [ ] どの段でも、`hidden_allowed` に基本の層の allow に無い名前が出ない。読み取りだけで入力は送らない

## 23. deny のウィンドウを撮影から外す (D-077)

手順 0 の設定 (基本の層) のまま、`--config=` の上書き層で文字コード表 (charmap.exe) だけを deny に足す。基本の層の allow に無い文字コード表は `windows` に出ず、撮ってよい範囲はメモ帳の矩形の中に限る (下の「AI のセッションから CLI の手順を確かめるとき」の規則)。全画面は撮らない。
上書き層 `target/manual-check/s23.toml` は次の内容だけにする (allow を広げない。`[capture]` を書かないので、この層の全画面は既定の拒否のまま):

```toml
version = 1
[general]
capture_enabled = true
[deny]
processes = ["charmap.exe"]
```

役の分け方: GO のときに人が文字コード表 (`charmap.exe`) とメモ帳を開き、文字コード表をメモ帳の一部 (例: 右半分) に重ねて置く (Z 順でメモ帳の上)。重ね終えたら端末に GO を打って端末を前面に戻し、人の手はこの 1 回だけで、以後は手を離す。端末の窓はメモ帳と文字コード表の重なりにかけない (かかると端末が覆いとして引かれ、(1) が 7 にならない)。文字コード表は Z 順でメモ帳の上にあるが、前面のウィンドウではない。
AI は `target/release/ai-cu.exe windows` でメモ帳の `rect` を読み、region はメモ帳の `rect` の中で文字コード表が重なっている部分に取る (文字コード表は一覧に出ないので、重なっている位置は人に聞く)。region はメモ帳の角から数十ピクセル離す (前面の窓の角の丸みの分は判定が覆いとみなすため)。
`AI_CU_MANUAL_CHECK=1` のセッションで、release のパスを文字どおり書く。撮った PNG は `Read` で開かず、終了コードと出力の JSON だけを記録する。終了コード 10 (人が操作中) なら 3 秒待って 1 回だけ打ち直す。

```powershell
& $cu shot --config=target/manual-check/s23.toml --out target/manual-check/s23-a.png --region <重なりの矩形>; $LASTEXITCODE
& $cu focus notepad.exe; $LASTEXITCODE
& $cu shot --config=target/manual-check/s23.toml --out target/manual-check/s23-b.png --region <同じ矩形>; $LASTEXITCODE
```
- [ ] (1) 文字コード表がメモ帳の上に重なったまま 1 つ目の `shot`: 終了コード `7`、`error.code` が `foreground_guard`、`error.detail.window.process` が `charmap.exe` (題名のキーは無い)、`error.detail.visible` (`x`、`y`、`w`、`h`) が region の中にある、`hint` に「設定を緩めるよう頼まない」。`s23-a.png` は作られていない
- [ ] (2) `focus notepad.exe` (`--config` なし、基本の層だけ。上書き層は `input_enabled` を持たないため) で終了コード `0`、`focused` にメモ帳。`11` (`focus_failed`) になったらそのまま記録し、人にメモ帳を前面にしてもらってから続ける (手順 15 と同じ扱い)。続く同じ region の `shot` (同じ `--config`): 終了コード `0` (メモ帳に覆われた文字コード表は数えない)
- [ ] 起動した側の端末を数えないこと、`--window` と全画面の経路は、ここでは確かめない (モックのテストに任せる。端末は `windows` に出ず rect を読めず、`--window` は隠れの確認が先に 7 で断るため)。片付け (文字コード表とメモ帳を閉じる) は人が行う

## 24. 毎回の承認 (D-078)

手順 0 の設定 (基本の層) のまま、`--config=` の上書き層で `windows` だけを `mcp.require_approval` に足す。上書き層 `target/manual-check/s24.toml` は次の内容だけにする (`windows` は撮影と同じ許可が要り、上書き層も 1 つの層として判定に加わるので `capture_enabled` を書く。層は締める向きにしか動かないので、許可は広がらない):

```toml
version = 1
[general]
capture_enabled = true
[mcp]
require_approval = ["windows"]
```

手順 19 の A と同じく、別の作業用のプロジェクトで、人が Claude Code に絶対パスで登録し直す (登録済みなら `claude mcp remove ai-cu` してから):
`claude mcp add ai-cu -- <repo>\target\release\ai-cu.exe --config=<repo>\target\manual-check\s24.toml mcp`
(Claude Code はサーバーを別の作業ディレクトリで起動するので、相対の `--config=` は見つからず、設定が読めないとして `guide` 以外のすべての道具に印が付く)。
そのプロジェクトの Claude Code の権限で `mcp__ai-cu__windows` と `mcp__ai-cu__status` を allow にしておき、Claude Code を起動して `/mcp` で接続済みと出ることを見る。そのセッションに次を頼む。確認の画面は人が目で見て操作する (AI は確認の画面を操作しない):

- [ ] `status` → 確認なしで結果が返る。`status` にまで確認が出たら、設定が読めていない (`--config=` のパスを見直して `/mcp` で再接続する)
- [ ] `windows` → allow にしてあっても確認が出て、「次から聞かない」の選択肢が出ない。承認すると一覧が返る
- [ ] もう一度 `windows` → また確認が出る (呼ぶたびに出る)
- [ ] 片付け: 人が `claude mcp remove ai-cu` で登録を外すか、手順 19 の登録に戻す

## 25. 拒否の直し方と導入の文例 (D-079、D-080)

装置に触れない (入力も撮影もしない)。手順 0 の設定 (基本の層) のまま行い、設定ファイルは書かない。

(1) 導入の文例: `target/release/ai-cu.exe guide --setup > target/manual-check/s25-setup.md` を打ち、出力を目で見る:

- [ ] 終了コード 0。`## 1. この実行ファイル` に `target\release\ai-cu.exe` の完全なパスと、「~/.cargo/bin/ai-cu.exe に入れたもの (cargo install か、開発者は python scripts/install.py) を登録する」の注意が出る
- [ ] `## 2. 設定ファイル` の場所が `ai-cu config path` の `floor` (基本の層) のパスと同じで、「ファイルはある」と出る (設定の中身は出ない)
- [ ] `## 3. MCP の登録` の `claude mcp add ai-cu -- "<パス>" mcp` の行が、パスを二重引用符で囲み、そのまま端末に貼れる形 (貼って登録はしない)
- [ ] `## 4.` の CLAUDE.md の文例が CLI 版と MCP 版の 2 つで、CLI 版にも同じパスが入る
- [ ] `## 5.` の権限の JSON で、allow が読み取りの道具 (`mcp__ai-cu__guide` など)、ask が実入力の道具で、基本の層が既定の場所なら deny が `Edit(~/AppData/Roaming/ai-cu/**)`

(2) 拒否の直し方: 許可に無いプロセスへの `focus` を 1 つだけ書いた操作列 `target/manual-check/s25-focus.json` (`{"version": 1, "actions": [{"type": "focus", "process": "calc.exe"}]}`。手順 0 の基本の層の allow に `calc.exe` が無いことを先に `config show` で確かめる) を `target/release/ai-cu.exe validate target/manual-check/s25-focus.json` に通す (装置に触れない):

- [ ] 終了コード 4、`error.rule` と `issues[0].rule` が `allow.processes`
- [ ] `error.fix` が「許可してよければ、端末で ai-cu を引数なしで (--config も AI_CU_CONFIG も付けずに) 起動した TUI の「許可リスト (allow) > アプリ (プロセス)」か、設定ファイル <基本の層のパス> の allow.processes に "calc.exe" を足す」で、基本の層のパスが (1) の場所と同じ
- [ ] 人が TUI (引数なしの起動。端末で人が行う) で、fix の「許可リスト (allow) > アプリ (プロセス)」を TUI の左の一覧で見つけられる (変更は保存しない)

## 26. 画面が落ち着くまで待つ、変わるまで待つ (wait_stable、wait_change、--settle-ms auto、D-081)

手順 0 の設定 (基本の層) のまま。基本の層の `allow.actions` を書いているなら、`wait_stable` と `wait_change` (と `click`、`focus`、`close`、`screenshot`) が並んでいることを `config show` で確かめる (無ければ 4 になる。AI は設定を書き換えず、そのまま記録して止まる)。
準備と GO と背景での起動の書き方は手順 21 と同じ: `AI_CU_MANUAL_CHECK=1` のセッションで、release のパスを文字どおり書き、GO を 1 回もらってから行う。人は GO の後は手を離す。
`wait_stable` と `wait_change` は入力を送らないが、(d) の `click` は入力を送る (メモ帳の本文の中の 1 点だけ)。撮った PNG は `Read` で開かず、終了コードと出力の JSON だけを記録する。終了コード 10 (人が操作中) なら 3 秒待って 1 回だけ打ち直す。

準備:
- 人が Ctrl+Shift+N で空の新しいメモ帳のウィンドウを開く (`windows` にメモ帳が 1 つも無ければ、AI が `Start-Process notepad` で開いてよい)。題名「タイトルなし」のウィンドウが他に無いことを `windows` で確かめる (手順 21 の (6) と同じ。(b) と (c) で閉じるので中身を入れない)
- AI が `windows` でメモ帳の `rect` と `content_point` を読む。(b) の region はメモ帳の `rect` の内側 (角から数十ピクセル離した矩形) に取る
- (b) の region のメモ帳の奥 (メモ帳を閉じた後に見える所) に、ai-cu を起動した側の端末以外の deny の窓 (手順 0 の deny のエディタなど) が無いことを確かめる。deny の窓は `windows` に出ないので、`windows` の `other_windows` の数と人の目で確かめる。あれば、閉じた後の撮影が 7 で止まる (下の (b) の期待)
- 順は (a)、(d)、(b)、(c) にする ((b) と (c) でメモ帳を閉じるため)

(a) キャレットが点滅する中で `wait_stable` が落ち着く: 操作列 `target/manual-check/s26-a.json` は `{"version": 1, "actions": [{"type": "focus", "process": "notepad.exe", "title": "タイトルなし"}, {"type": "wait_stable", "window": {"process": "notepad.exe", "title": "タイトルなし"}}]}`

```
target/release/ai-cu.exe run target/manual-check/s26-a.json
```
- [ ] (a) 終了コード `0`、`executed` が 2、`waited[0]` に `rect` (メモ帳の `rect` と同じ)、`window` (`process` が notepad.exe)、`checks` が 3 以上 (大きな窓では撮影 1 回が約 110 ms で 3 回になる。2026-10-27)、`elapsed_ms` が 300 以上 10000 未満。`elapsed_ms` と `checks` を記録し、release での撮影 1 回の時間 c を、おおよそ「`elapsed_ms` / (`checks` − 1) − 100」で逆算する (比較の間隔は 100 ms の待ちと撮影 1 回の和) (D-081 の自分で決めたことの、キャレットの見立ての覆る条件はこの値で判定する。c が 30 ms を超えるか、15 (`unmet: stable`) で落ち着かなかったら、D-081 の覆る条件として記録する)

(d) 一手ずつの `--settle-ms auto`: X Y はメモ帳の `content_point`

```
target/release/ai-cu.exe click X Y --shot target/manual-check/s26-d.png --window notepad.exe --window-title タイトルなし --settle-ms auto
```
- [ ] (d) 終了コード `0`、`executed` が 2 (固定の `wait` が挟まらない)、`screenshots[0].settled` が true、`settled` が true なら `screenshots[0].settle_ms` が 300 以上 3000 未満。`settled` が false (3000 ms で落ち着かなかった) なら `settle_ms` は 3000 を撮影 1 回分ほど超える (途中で隠れの猶予 (D-082) が効けば最大 500 ms さらに)。それも成功の形なのでそのまま記録し、(a) の c と並べて D-081 の覆る条件として記録する

(b) `region` で `wait_change` を待ち、メモ帳を閉じて満たす: 操作列 `target/manual-check/s26-b.json` は `{"version": 1, "actions": [{"type": "wait_change", "region": {"x": .., "y": .., "w": .., "h": ..}, "timeout_ms": 20000}]}` (region は準備で取った矩形)。
前提: (a) と (d) の後で、メモ帳が前面にあり region を覆っている (前面でなければ、先に `focus notepad.exe --title タイトルなし` を打つ)。
AI は Bash ツールの `run_in_background` で次を始め、`checks` が 2 回以上入るよう 1 秒ほど置いてから、別の呼び出しで `close` を打つ (手順 21 の (6) と同じく、コマンド文字列そのものに release の起動を直接書く):

```
target/release/ai-cu.exe run target/manual-check/s26-b.json > target/manual-check/s26-b.out; echo $? > target/manual-check/s26-b.code
```
```
target/release/ai-cu.exe close notepad.exe --title タイトルなし
```
- [ ] (b) `close` は終了コード `0` で `closed[0].gone` が true。`s26-b.code` が 0、`s26-b.out` の `waited[0]` に `rect` (region と同じ) と `changed_ratio` (0.001 を超える) があり、`window` のキーが無い。`elapsed_ms` が `close` までの時間 (20000 未満)。準備で region の奥に deny の窓があった場合は、代わりに `s26-b.code` が 7 で、`error.detail.window.process` がその deny のアプリ (題名のキーは無い)

(c) `window` で `wait_change` を待つと、閉じた時点で 12: 準備と同じく空のメモ帳を開き直し (`windows` にメモ帳が 1 つも無いときだけ AI が `Start-Process notepad` で開く。メモ帳が開いているときに打つと既存のウィンドウにタブが足されるので (手順 21)、それ以外は人が GO の前に Ctrl+Shift+N で新しいウィンドウを開く)、題名「タイトルなし」が 1 つだけであることを `windows` で確かめる。操作列 `target/manual-check/s26-c.json` は `{"version": 1, "actions": [{"type": "focus", "process": "notepad.exe", "title": "タイトルなし"}, {"type": "wait_change", "window": {"process": "notepad.exe", "title": "タイトルなし"}, "timeout_ms": 20000}]}`。(b) と同じく背景で起動し、1 秒ほど置いてから `close` を打つ:

```
target/release/ai-cu.exe run target/manual-check/s26-c.json > target/manual-check/s26-c.out; echo $? > target/manual-check/s26-c.code
```
```
target/release/ai-cu.exe close notepad.exe --title タイトルなし
```
- [ ] (c) `close` は終了コード `0`。`s26-c.code` が 12 (`window_not_found`。閉じた後の撮影で対象が見つからない)、`s26-c.out` の `executed` が 1 (先頭の `focus` は済み)。15 (`wait_timeout`) ではない (閉じる前にメモ帳の見た目が変わって満たされたなら 0 になる。そのときは `changed_ratio` を記録する)。7 になったら D-082 を見直す

## 27. 要素の木 (tree、D-084)

手順 0 の設定 (基本の層) のまま。撮影の許可 (capture_enabled) が要る。実入力は送らない。
手順 18 と同じく、メモ帳を開いて何か (例「一手ずつ」と改行) を入れておく。前面でなくてよい。

```powershell
& $cu tree --window notepad.exe; $LASTEXITCODE
```
- [ ] (a) 終了コード `0`。`window` にメモ帳、`tree` の 1 行目が根の `window "<題名>" @x,y` (字下げ無し)。メニュー (`menu_bar` や `menu_item`、例「ファイル」) と本文 (`document`) の行が字下げつきで出る。各行の `@x,y` が見た目の位置 (メニューの文字や本文の中) に合う (`windows` の `rect` の中に収まる)。`lines` が行の数と同じ、`truncated` は false、`depth_cut` と `skipped` は 0 (0 でなければ値を記録する)。`folded` を記録する (名前の無い pane や画面外の要素があれば 1 以上)

```powershell
& $cu tree --window notepad.exe --all; $LASTEXITCODE
```
- [ ] (b) 終了コード `0`、`folded` が 0。(a) の `folded` が 1 以上なら、その分 `lines` が増える (名前の無い `pane` などの行が出る。`limit` の 200 行で切れたら `truncated` が true)

```powershell
Measure-Command { & $cu tree --window notepad.exe } | Select-Object TotalMilliseconds
```
- [ ] (c) 時間を記録する (Bash ツールで打つなら `time target/release/ai-cu.exe tree --window notepad.exe`)。数秒を超えて遅ければ、D-084 の覆る条件 (1 要素ずつのプロセス間の呼び出しが遅い) として記録する

## 28. パスワード欄 (D-090、D-092)

確認専用のアプリ pw-probe (`tools/pw-probe`、D-092) で行う。窓の題は `ai-cu pw-probe` で、上から ふつうの欄 (TextBox、automation_id `plain`、初期値 `plain-text`)、パスワード欄 (PasswordBox、`secret`、初期値 `probe-secret` の 12 文字)、長さの表示 (TextBlock、`length`、名前「パスワードの長さ: N」) が並ぶ。Tab の順は ふつうの欄 → パスワード欄で、起動時のフォーカスはふつうの欄。

前提:
- 人が `tools/pw-probe/README.md` の手順で pw-probe をビルド済み (`dotnet build <repo>/tools/pw-probe/pw-probe.csproj -c Release`)
- `pw-probe.exe` は基本の層の `allow.processes` に入れたままでよい (2026-10-28 に利用者が決めた)。入っていなければ人が TUI で足す。AI は実際の設定ファイルを書かない。`general.password_input` は false のまま (書かない)。他は手順 0 のまま (`input_enabled` と `capture_enabled` が true)
- パスワード欄の中身は試し用の固定の文字列 `probe-secret` で、本物のパスワードは入れない。送る文字は無害な 1 文字 (`x`、`a`、`b`、`y`)
- 中身が出ていないかは、出力 (標準出力の JSON) に `probe-secret` が無いことで確かめる。入ったかは長さの表示 (`length` の名前) で確かめる (中身は見ない)

```powershell
$probe = "<repo>\tools\pw-probe\bin\Release\net9.0-windows\pw-probe.exe"
Start-Process $probe; Start-Sleep -Seconds 2
```

(a) 読み取り (実入力は送らない。GO 無しでよい):
```powershell
& $cu find --window pw-probe.exe; $LASTEXITCODE
& $cu text --window pw-probe.exe --name "パスワード欄"; $LASTEXITCODE
& $cu text --window pw-probe.exe; $LASTEXITCODE
& $cu text --window pw-probe.exe --type edit; $LASTEXITCODE
```
- [ ] 1 つ目 (`find`) は終了コード `0`。`automation_id` が `plain` の要素 (種類 `edit`、名前「ふつうの欄」) は `password: false`、`secret` の要素 (種類 `edit`、名前「パスワード欄」) は `password: true`、`length` の要素の名前が「パスワードの長さ: 12」。`plain` と `secret` の `center` を控える ((b) 以降で使う)。`secret` が `password: false` なら、WPF の PasswordBox が IsPassword を出していない (D-092 の覆る条件として記録し、(b) 以降を行わない。読めないアプリでは 4 にならず文字が入る)
- [ ] 2 つ目は終了コード `13` (`"code":"element_not_found"`) で、message の末尾に「(パスワードの要素 1 個は読まない)」。出力のどこにも `probe-secret` が無い
- [ ] 3 つ目 (名前で絞らない) は終了コード `0`。text は木の順で文字列を持つ最初の要素を返すので、見出しの TextBlock「ふつう (plain)」(種類 `text`) が返る見込み (WPF の TextBlock は TextPattern を出す。推定)。`plain-text` (ふつうの欄) が返ってもよい。どちらが返ったかを記録する。出力のどこにも `probe-secret` が無い
- [ ] 4 つ目 (種類 `edit` で絞る) は終了コード `0`、`text` が `plain-text`、`element` の名前が「ふつうの欄」。出力のどこにも `probe-secret` が無い

(b) 入力の拒否 (実入力。GO の範囲で行う)。`<SX> <SY>` は (a) の `secret` の `center`:
```powershell
& $cu focus pw-probe.exe; $LASTEXITCODE
& $cu click <SX> <SY>; $LASTEXITCODE
& $cu type x; $LASTEXITCODE
& $cu find --window pw-probe.exe --automation-id length; $LASTEXITCODE
```
- [ ] `focus` と `click` は終了コード `0` (パスワード欄にフォーカスが移る)
- [ ] `type` は終了コード `4`、`"code":"policy_denied"`、`error.rule` が `general.password_input`、`error.fix` が「を true にする」の形、hint が「何も送っていない。キーと文字の届く先 (フォーカス) がパスワード欄なので断った。パスワードは AI が打たず、人間に打ってもらう」で始まる文 (allow.processes の 4 や、フォアグラウンドガードの 7 とは `rule` と hint で区別する)。`error.detail.window` の `process` が `pw-probe.exe`
- [ ] 最後の `find` は終了コード `0` で、`length` の名前が「パスワードの長さ: 12」のまま (`x` は入っていない)

(c) Tab の後の判定のやり直し (D-090 の追記。実入力)。`<PX> <PY>` は (a) の `plain` の `center`。
Tab を含む文字列は、一手ずつの `type` の引数ではなく、`run` の JSON を標準入力で渡す (PowerShell 5.1 は引数の Tab を引用せずに渡しうるので、ai-cu の側で 2 つの引数に割れうる。推定)。
JSON の `\t` が Tab で、`type` は Tab を Tab キーとして送る。PowerShell が末尾に足す改行は JSON の空白なので、文字としては送られない:
```powershell
& $cu focus pw-probe.exe; $LASTEXITCODE
& $cu click <PX> <PY>; $LASTEXITCODE
& $cu key end; $LASTEXITCODE
'{"version":1,"actions":[{"type":"type","text":"a\tb","interval_ms":200}]}' | & $cu run -; $LASTEXITCODE
& $cu text --window pw-probe.exe --name "ふつうの欄"; $LASTEXITCODE
& $cu find --window pw-probe.exe --automation-id length; $LASTEXITCODE
```
- [ ] `focus`、`click`、`key end` は終了コード `0` (`key end` で、ふつうの欄の文字の末尾に移る)
- [ ] `run` は終了コード `4`、`error.rule` が `general.password_input`、`error.action_index` が `0`。`a` と Tab は送り、Tab でパスワード欄に移った後の `b` は送らない
- [ ] `text` は終了コード `0` で `text` が `plain-texta` (`a` は入った)
- [ ] 最後の `find` で `length` の名前が「パスワードの長さ: 12」のまま (`b` は入っていない)。`interval_ms` を 200 にしているのは、Tab の後にアプリがフォーカスを移し終えてから次の束の前で判定するため (既定の 20 では、移る前のふつうの欄を読んで通しうる)。13 になっていたら、`interval_ms` を 1000 にしてもう 1 回撃つ (その前にパスワード欄の長さを戻すため pw-probe を閉じて起動し直す)。1000 で 12 なら時間の競合として、1000 でも 13 なら Tab の後の判定のやり直しが効いていない (D-090 の追記の失敗) として記録する

(d) 許可を足さずに、ふつうの欄への入力は通る (実入力):
```powershell
& $cu focus pw-probe.exe; $LASTEXITCODE
& $cu click <PX> <PY>; $LASTEXITCODE
& $cu key end; $LASTEXITCODE
& $cu type y; $LASTEXITCODE
& $cu text --window pw-probe.exe --name "ふつうの欄"; $LASTEXITCODE
```
- [ ] `focus`、`click`、`key end`、`type` はどれも終了コード `0` (フォーカスがパスワード欄でなければ、`general.password_input` が false でも断らない)
- [ ] `text` は終了コード `0` で `text` が `plain-textay`

後片付け:
```powershell
Stop-Process -Name pw-probe
```
- [ ] pw-probe が閉じている (`allow.processes` の `pw-probe.exe` は外さない)

AI のセッションの Bash ツールで打つときは、`$cu` の代わりに `target/release/ai-cu.exe` を書き、(c) の `run` は `echo '{"version":1,"actions":[{"type":"type","text":"a\tb","interval_ms":200}]}' | target/release/ai-cu.exe run -` の形にする (単引用符の中の `\t` はそのまま JSON に渡る)。pw-probe の起動と後片付けは `powershell.exe -NoProfile -Command "Start-Process '<exe の絶対パス>'"` などで行ってよい (pw-probe は確認専用のアプリで、起動は実入力ではない)

## 29. 操作の前の枠と予告の間 (D-093)

手順 0 の設定 (基本の層) のまま。基本の層の `allow.actions` を書いているなら、`focus`、`type`、`key`、`wait_for`、`wait_change` が並んでいることを `config show` で確かめる (無ければ 4 になる。AI は設定を書き換えず、そのまま記録して止まる)。基本の層に `[indicator]` を書いていなければ既定 (`enabled = true`、`lead_ms = 3000`、`sound = false`) になる。
準備と GO と背景での起動の書き方は手順 21 と同じ: `AI_CU_MANUAL_CHECK=1` のセッションで、release のパスを文字どおり書き、GO を 1 回もらってから行う。
枠は撮影に写らない作りなので、枠と札が出たか、いつ消えたかは人が目で見て AI に伝える (AI は画像で確かめられない)。人は GO の後、マウスとキーに触れない ((e) でメモ帳を前面にする動きを除く。予告の間に触れて止まることは (b) のとおり実機テストが確かめる)。
送る入力はメモ帳の中だけ (`focus`、`type`「枠」と改行、`key end`)。撮った PNG は `Read` で開かず、終了コードと出力の JSON だけを記録する。

準備:
- 人が空の新しいメモ帳のウィンドウを開く (`windows` にメモ帳が 1 つも無ければ、AI が `Start-Process notepad` で開いてよい)。題名「タイトルなし」のウィンドウが 1 つだけであることを `windows` で確かめる。メモ帳は前回のタブを復元して起動することがあるので、そのときは人がタブの並びの「＋」で空のタブを作って選ぶ (Ctrl+N では新しいタブにならず、復元したタブの題名に `*` が付いたことがある。2026-10-29)
- 以下の `focus` は題名を「メモ帳」で探す (メモ帳のウィンドウが 1 つだけなので、これで同じ窓に当たる)。今のメモ帳は「枠」を入れた後に題名が `*枠 - メモ帳` に変わり、`タイトルなし - メモ帳` に戻ることもあるので、「タイトルなし」で探すと 12 になる
- AI が `target/release/ai-cu.exe config show` を打つ。終了コード `0` で、各層の `indicator` が `{"enabled": true, "lead_ms": 3000, "sound": false}` (基本の層に `[indicator]` を書いてあれば、その値と層の合成 ([設定ファイルの詳細](../reference/config.md) の `[indicator]` の行) を記録し、以下の 3000 をその `lead_ms` に読み替える)
- 操作列 `target/manual-check/s29-a.json` は `{"version": 1, "actions": [{"type": "focus", "process": "notepad.exe", "title": "メモ帳"}, {"type": "type", "text": "枠\n"}]}`

(a) CLI の `run` で、枠と札が出て約 3 秒後に動き、終わるとすぐ消える:

```
s=$(date +%s%N); target/release/ai-cu.exe run target/manual-check/s29-a.json; echo "exit=$? wall_ms=$(( ($(date +%s%N) - s) / 1000000 ))"
```
- [ ] 終了コード `0`、`executed` が 2、`focused` にメモ帳。`wall_ms` から `elapsed_ms` を引いた差が 3000 以上 (予告の間は `elapsed_ms` に含めない)
- [ ] (人の目) 起動の直後に、モニターの縁の四辺に青い枠 (内側に向けて薄くなる) と、上の中央に札「ai-cu が操作中 — マウスを動かすと止まる」が出た。複数のモニターがあれば、すべてのモニターに出た。約 3 秒たってからメモ帳が前面になって「枠」が入り、終わるとすぐ (1 秒もかからずに) 枠と札が消えた
- [ ] (人の目) 枠が出ている間、端末のフォーカスは奪われなかった (メモ帳が前面になるのは `focus` の後)

(b) 予告の間に人が触れると何も送らずに止まり、枠がすぐ消えることは、実機テスト `fixture_real_lead_stops_on_injected_mouse_move` (6) と `fixture_real_lead_stops_on_injected_key_with_lead_input` (10 `lead_input`) が `python scripts/verify.py --ignored` で確かめる (フィクスチャの上に別のスレッドから入力を送り、枠が消えたことも機械で見る。D-095)。手順 29 では行わない (人がマウスやキーに触れる項目は無い)

(c) 常駐の MCP は枠を最後の呼び出しから 30 秒残し、出ている枠は読み取りの呼び出しでも延びる。その間の次の呼び出しは予告なしで続く (D-107): `mcp-requests-indicator.jsonl` は `status`、`focus`、`key end`、読むだけの `wait_for` (無い題名を 20 秒待つ) を 2 本、`key end`、読むだけの `wait_for` (40 秒)、`key end` の順 (id 1〜9)。`--log` の行の「(… ms)」で呼び出しごとの時間 (予告の間を含む) を見る。`mcp_client.py` の `--timeout` は要求ごとの応答の待ち (秒) なので、40 秒の `wait_for` に足りる 60 にする:

```
python <repo>/docs/manual-check/mcp_client.py <repo>/target/release/ai-cu.exe mcp --log <repo>/target/manual-check/mcp-indicator.log -- <repo>/docs/manual-check/mcp-requests-indicator.jsonl --timeout 60
```
- [ ] id 2 (`status`): `indicator` が `{"enabled": true, "lead_ms": 3000, "sound": <基本の層の値>, "frame": {"state": "not_started", "error": null, "retry_in_ms": null}}` (`sound` は基本の層の `[indicator] sound` の値。利用者は意図して true にしている (2026-11-04)。このプロセスはまだ画面を変える道具を呼んでいないので `not_started`。D-103)
- [ ] id 3 (`focus`): `isError` が false。ログの `tools/call focus → ok` の時間が 3000 ms 以上 (予告の間)
- [ ] id 4 (`key`): `isError` が false。ログの時間が 1000 ms 未満 (枠が 3 秒以上前から出ているので予告を省いた)
- [ ] id 5・6 (`wait_for`): どちらも `exit_code` 15 (読むだけなので枠を新しくは出さない)。(人の目) 2 本の待ち (合わせて約 40 秒。id 4 の終わりから 30 秒を超える) の間、枠が出続けた (読み取りの呼び出しの始めと終わりで延びた)
- [ ] id 7 (`key`): `isError` が false。ログの時間が 1000 ms 未満 (延長で枠が出続けたので予告を省いた。延長が無ければ id 4 の終わりから 30 秒で枠が切れ、3000 ms 以上になる)
- [ ] id 8 (`wait_for`): `exit_code` 15。(人の目) 待ちの始めから約 30 秒で枠が消えた (延長の後に切れる)
- [ ] id 9 (`key`): `isError` が false。ログの時間が 3000 ms 以上 (枠が消えた後なので、また予告に入った)。`server exit=0` の後に枠が消えている (プロセスの終わりで消える)

(d) `sound = true` で予告の間に入るときに通知音が鳴る: 上書き層 `target/manual-check/s29-sound.toml` を AI が書く (`version = 1`、`[general]` に `input_enabled = true`、`[indicator]` に `sound = true` だけ。基本の層は変えない):

```
target/release/ai-cu.exe focus notepad.exe --title メモ帳 --config=target/manual-check/s29-sound.toml; echo "exit=$?"
```
- [ ] 終了コード `0`。(人の耳) 枠が出たときに Windows の通知音 (サウンドの設定の Asterisk に当てた音。`MessageBeep(MB_ICONASTERISK)`) が 1 回鳴った (音を消している PC では鳴らない。そのときはそう記録する)
- [ ] 同じコマンドを `--config` なしで打つと、枠は出て音は鳴らない

(e) 枠が出ている間の撮影に枠が写らない: 前提として、人が (d) のメモ帳を最大化して前面に置く (画面の上の端の帯がメモ帳の題名の帯になる。端末や deny の窓が上の端に見えていれば 7 になる)。
AI は、画面の上の端の帯を待ち始めの基準にして変わるのを待つ `run` を背景で始め、1 秒ほど置いてから、別の呼び出しで `key end` を打つ (`key` の予告の間に枠と札が上の端に出る。`key end` は本文の中のキャレットだけを動かし、上の端の帯は変えない)。
2 本目にカーソルを動かす `move` を使わないのは、背景の `run` もマウスを見張っていて、別のプロセスの `move` を利用者のマウスと見て 6 で止まるため。実装のステップ 7 の「`shot` に写らない」を `wait_change` で確かめるのは、撮った PNG を AI が開かずに、写ったかどうかを終了コードと `last_changed_ratio` で読めるようにするため。
操作列 `target/manual-check/s29-e.json` は `{"version": 1, "actions": [{"type": "wait_change", "region": {"x": 0, "y": 0, "w": <主モニターの幅>, "h": 60}, "timeout_ms": 10000}]}`:

人は返事 (GO) を端末に打つので、GO の時点では端末が前面になる。AI は 2 本を 1 つのスクリプト (`target/manual-check/s29-e.sh`) にまとめ、先頭で 12 秒待ってから `windows` で前面を読み、`Notepad.exe` でなければ何も送らずに止まる。人は GO を打った後の 12 秒のうちに、タスクバーか Alt+Tab で最大化したメモ帳を前面にして手を離す (2026-10-29 の実施で、GO を出せないと分かって足した):

```
target/release/ai-cu.exe run target/manual-check/s29-e.json > target/manual-check/s29-e.out; echo $? > target/manual-check/s29-e.code
```
```
target/release/ai-cu.exe key end; echo "exit=$?"
```
- [ ] `key` は終了コード `0`。(人の目) `key` の前の約 3 秒、上の端に枠と札が出ていた
- [ ] `s29-e.code` が 15 (`wait_timeout`)、`s29-e.out` の `error.detail.unmet` が `change`、`last_changed_ratio` が 0.001 以下 (枠と札が撮影に写っていない)。`s29-e.code` が 0 になったら (枠が写って満たした) D-093 の覆る条件として記録し、`changed_ratio` を控える

後片付け: メモ帳の「枠」の行は人が消すか、保存せずに閉じる。`target/manual-check/s29-*` は残してよい (git 管理外)

## 30. 重い検査の枠 (D-111)

`python scripts/verify.py --ignored` が、最初の段 (fmt) の前から最後の段の後まで、画面の縁に別の枠 (`scripts/verify_frame.py` の子プロセス。クラス `ai_cu_verify_frame`) を出すことを人の目で確かめる。
基本の層のまま行い、設定ファイルを作らない。実入力を送るのは ignored の段のテストだけで、送り先は自前のフィクスチャの上 ([開発者向けの詳細](../reference/development.md) の実機のテストの行)。それでも実機に触れるので、GO を 1 回もらってから行い、人は検査の間マウスとキーに触れない (触れると ignored の段のテストが落ちうる。検査は止まらない)。
枠は撮影に写らない作りなので、枠と札が出たか、いつ消えたかは人が目で見て AI に伝える (AI は画像で確かめられない)。cargo を走らせるので、開発セッション (このリポジトリ) の主エージェントが行う。

AI は GO の後、Bash ツールの `run_in_background` で次を打ち、打ったことを人に伝える:

```
python scripts/verify.py --ignored > target/manual-check/s30-verify.log 2>&1; echo "exit=$?"
```
- [ ] `s30-verify.log` の頭 (`== fmt ==` より前) に `枠: 画面の縁に枠を出した。3000 ms 後に始める (D-111)` の行がある (`枠: 出せない (<理由>)。枠なしで続ける` なら、その理由と `target/verify/frame.log` を記録して止まる)
- [ ] (人の目) 打った直後に、モニターの縁の四辺に一様な青い枠と、下の中央 (タスクバーの上) に札「ai-cu の重い検査中 — 実入力のテストの間はマウスとキーに触れない」が出た。複数のモニターがあれば、すべてのモニターに出た
- [ ] (人の目) 最後の段 (ignored) が終わるまで枠と札が出続けた。ignored の段では ai-cu の枠と札 (上の中央) も出るが、下の札と重ならずに両方読めた
- [ ] (人の目) 検査が終わると (`exit=` の行が出た頃に) 枠と札が消えた
- [ ] `exit=0` で、ログの各段の判定が ok (枠が出たまま ignored の段の撮影・入力・予告の間のテストが通る)。落ちた段があれば、その段の `target/verify/<名前>.log` の該当行を記録する

## 31. 入力の排他 (D-113)

release の ai-cu を 2 本 (A と B)、同じメモ帳に対して同時に走らせ、後から打った B が A の終わりまで入力の錠を待ち、文字が交ざらないことと、30 秒を超えて待たされると 16 (`input_busy`) になることを確かめる。
手順 0 の設定 (基本の層。allow に notepad) のまま行い、設定ファイルを作らない。基本の層の `allow.actions` を書いているなら、`focus` と `type` が並んでいることを `config show` で確かめる (無ければ 4 になる。AI は設定を書き換えず、そのまま記録して止まる)。
同じ `target/release/ai-cu.exe config show` で、基本の層と合成の `indicator` の `enabled` と `lead_ms` を記録する。`enabled` が true であることを確かめ (false なら予告の間を通らず (a) の差が出ないので、そのまま記録して止まる)、`lead_ms` が 3000 以外なら、(a) の差の下限 9000 の内の 3000、(人の目) の約 3 秒、(b) の上限の 3000 をその `lead_ms` に読み替える (手順 29 と同じ)。`sound` が true なら、B の予告の間に通知音が 1 回鳴るのが正しい。
メモ帳に実入力を送るので、準備と GO と背景での起動の書き方は手順 21 と同じ: `AI_CU_MANUAL_CHECK=1` のセッションで、release のパスを文字どおり書き、GO を 1 回もらってから行う。人は GO の後、マウスとキーに触れない (A の入力の途中にマウスを動かすと A が 6 で止まる)。
送る入力はメモ帳の中だけ (`focus` と `type`)。撮影はしない。本文は `text` で読む。

準備:
- 人が空の新しいメモ帳のウィンドウを開く (手順 29 の準備と同じ。題名「タイトルなし」のウィンドウが 1 つだけであることを `windows` で確かめる)。`focus` は題名を「メモ帳」で探す
- 操作列 (AI が `target/manual-check/` に書く):
  - `s31-long.json` (A。予告の間 3000 ms の後に約 8 秒打つ): `{"version": 1, "actions": [{"type": "focus", "process": "notepad.exe", "title": "メモ帳"}, {"type": "type", "text": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n", "interval_ms": 200}]}` (A は 40 文字と改行)
  - `s31-short.json` (B): `{"version": 1, "actions": [{"type": "focus", "process": "notepad.exe", "title": "メモ帳"}, {"type": "type", "text": "bbbbb\n"}]}`
  - `s31-long36.json` (A'。(b) 用に約 36 秒打つ): `s31-long.json` の `interval_ms` を 900 にしたもの
- 人は返事 (GO) を端末に打つので、GO の時点では端末が前面になる。A の先頭の `focus` がメモ帳を前面にするので、人が前面を切り替える必要は無い

(a) B は A の終わりまで待ってから動き、文字が交ざらない: AI は 2 本を 1 つのスクリプト `target/manual-check/s31-a.sh` にまとめ、Bash ツールの `run_in_background` で打つ。A を背景で起動し、1 秒置いて (A が錠を取って予告の間に入った後) B を起動する:

```
target/release/ai-cu.exe run target/manual-check/s31-long.json > target/manual-check/s31-a-A.out & pa=$!
sleep 1; s=$(date +%s%N)
target/release/ai-cu.exe run target/manual-check/s31-short.json > target/manual-check/s31-a-B.out; echo "B exit=$? wall_ms=$(( ($(date +%s%N) - s) / 1000000 ))"
wait $pa; echo "A exit=$?"
```
- [ ] A と B の終了コードがどちらも `0`
- [ ] B の `wall_ms` から `s31-a-B.out` の `elapsed_ms` を引いた差が 9000 以上 (A の残りの約 10 秒を錠で待ち、その後に予告の間 3000 ms をまるごと待った。A の枠が出ていても省かない。待った時間は `elapsed_ms` に数えない)
- [ ] 終わった後の `target/release/ai-cu.exe text --window notepad.exe --type document` の本文が、A の 40 文字の行の後に `bbbbb` の行で、`b` が A の行の途中に入っていない
- [ ] (人の目) A が打ち終わって A の枠が消えた後も、メモ帳に何も入らない間 (約 3 秒。B の予告の間) があり、その後に `bbbbb` が入った

(b) 30 秒を超えて待たされると、B は何も送らずに 16 で返る: (a) と同じ形のスクリプト `target/manual-check/s31-b.sh` で、A を `s31-long36.json` に替えて打つ (出力は `s31-b-A.out` と `s31-b-B.out`)。A は予告の間の後に約 36 秒打つので、1 秒後に起動した B は 30 秒の上限に先に当たる
- [ ] B の終了コードが `16`。`s31-b-B.out` の `error.code` が `input_busy`、`error.detail.waited_ms` が 30000 以上、`executed` が無い。`hint` は「何も送っていない」旨と 1 回だけのやり直しを言う。B の `wall_ms` は 30000 以上で、30000 + 3000 (予告の間) より短い (錠を取れなかったので予告に入らない)
- [ ] A の終了コードが `0`。終わった後の `text` の本文に、(a) の 2 行の後に A' の 40 文字の行だけが増え、`bbbbb` の行は増えていない

記録する値: 使った release の版 (`target/release/ai-cu.exe --version`)、(a) と (b) の終了コード、B の `wall_ms` と `elapsed_ms`、(b) の `waited_ms`、`text` の本文。「実施の記録」の節に日付つきで書く。
後片付け: メモ帳の文字は人が消すか、保存せずに閉じる。`target/manual-check/s31-*` は残してよい (git 管理外)

## 32. TUI の配色とキーの案内 (D-129、D-130)

TUI の見た目の確かめで、人が自分の端末で行う。AI は TUI を起動しない (端末が無く、実入力にも関わらない手順だが、見た目は人の目でしか確かめられない)。
本物の設定ファイルは使わない。`--config=` で、手順の間だけ使う設定のコピーを指す (題名に「[追加の層: …]」が付く。配色は TUI が編集しているファイルの `[tui]` から読む)。基本の層は触れない。
`example-config.toml` に `[tui]` は無いので、`theme` は `auto` で始まる。

```powershell
New-Item -ItemType Directory -Force "<repo>\target\manual-check" | Out-Null
$cfg = "<repo>\target\manual-check\s32-config.toml"
Copy-Item "$dir\example-config.toml" $cfg -Force
```

(a) Windows Terminal の暗い背景で、`auto` が `calm` を選び、キーの案内が最下行に常に出る:

```powershell
& $cu --config=$cfg
```
- [ ] 左の一覧の末尾の節「表示」に「テーマ」があり、値が `(auto: calm)`。右に説明が出て、`左の一覧で Enter: 切り替え` の案内がある
- [ ] 全体が落ち着いた青灰色の配色で、原色のべた塗りが無い。補足の灰色の文字、枠、キー名のどれも暗い背景から読める。許可リスト (allow) の値は青系で、緑で塗られていない。拒否リスト (deny) の値は赤系
- [ ] 最下行に `↑↓ 選ぶ  Enter 開く・切替  s 保存  u 元に戻す  r 実行  F1 ヘルプ  Esc 終了` が出る (`s` と `u` は他の札より控えめ)。その上の行は空か、操作の結果のメッセージ
- [ ] 「画面撮影」で `Enter` → 値が切り替わり、題名が `[未保存 1 件] — s で保存` になる。最下行の先頭が `s 保存  u 元に戻す` に替わり、琥珀色で目立つ。メッセージの行に結果が出ても、最下行の案内は消えない
- [ ] `Ctrl+Z` → 元に戻る。題名の `[未保存 …]` と `— s で保存` が消え、`s` と `u` が元の控えめな位置と色に戻る
- [ ] 「アプリ (プロセス)」で `Enter` → 最下行が一覧のキー (`↑↓ 選ぶ  Enter 編集/追加  a 追加  d 削除  u 元に戻す  ← 戻る`) に替わる。`Esc` → 左の一覧に戻るだけで、終了しない。もう一度 `Enter` で右に入り、`a` で候補の一覧を開く → 最下行が候補のキー (`↑↓ 選ぶ  Enter 追加  F5 再読み込み  Esc 取消`) に替わり、`Esc` で取りやめても TUI は終わらない。`←` で左に戻る
- [ ] `F1` → ヘルプの枠が崩れずに出る。「どこでも」の行に `s・Ctrl+S`、`u・Ctrl+Z`、`?・F1`、`q 終了 (主画面では Esc も)` がある。最下行は `何かキー 閉じる` だけ。何かキーで閉じる。`?` でも同じヘルプが出る
- [ ] 変更が無いまま左の一覧で `Esc` → 確認なしで終了し、端末の表示が元に戻る。もう一度起動し、何かの項目を変えてから `Esc` → 「未保存の変更がある」の確認が出る (最下行は `s 保存して終了  y 破棄して終了  Esc 戻る  Ctrl+C 強制終了`)。確認の中の `Esc` は戻るだけ。`q` でも同じ確認が出る。`y` で破棄して終了する
- [ ] 端末の幅を 80 桁未満にして 1 ペインにしても、最下行の案内が崩れず、`F1 ヘルプ` と `Esc 終了` が最後まで残る (幅が足りなければ右の札から省かれる)

(b) 「テーマ」の切り替え: 起動し直して (`& $cu --config=$cfg`)、左の一覧の「テーマ」を選ぶ:
- [ ] `Enter` で `auto → calm → calm-light → ansi → mono → auto` の順に切り替わり (`Space` でも同じ)、確認の画面が出ずに、その場で配色が変わる。値の表示は `calm`、`calm-light`、`ansi`、`mono`、`(auto: calm)`。変更のたびに題名に `[未保存 1 件]` と、項目に `●` が付く (`auto` に戻れば消える)
- [ ] `calm-light`: 明るい背景用なので、暗い背景では文字が読みにくくなる (正しい)。そのまま `Enter` で次に進む
- [ ] `ansi`: 端末の 16 色で、枠と強調がシアン系、注意が黄色、成功が緑、危険が赤。背景は塗られない
- [ ] `mono`: 色が付かない。選んでいる行は反転し、前面のペインの枠が太く、✓ ✗ ⚠ ● ▶ の記号で区別できる
- [ ] `Ctrl+Z` で 1 つ前のテーマに戻る。配色もすぐ戻る
- [ ] `calm-light` にして `s` → 確認が出ずに保存される (テーマは許可を広げる変更ではない)。別の端末で `Get-Content $cfg` に `[tui]` と `theme = "calm-light"` がある。続けて `Enter` を 3 回で `auto` に戻して `s` → `Get-Content $cfg` が `theme = "auto"` になり、設定ファイルのほかの行は、直前の保存 (`calm-light` で保存した直後) のファイルと変わっていない (例の設定からの最初の保存では、足りなかった既定の表 (`password_input`、`[capture]`、`[mcp]`、`[indicator]` など) が増える。それは正しいので、比べるのは最初の保存の後から)。元のファイルは `.bak` に残る
- [ ] `& $cu config show --config=$cfg; $LASTEXITCODE` が終了コード `0` で、出力に `tui` も `theme` も出ない (許可の層だけを出す道具。D-129)
- [ ] `Copy-Item "$dir\example-config.toml" $cfg -Force` で戻して起動し、「画面撮影」だけを変えて `s` → `Get-Content $cfg` に `[tui]` の行が無い (既定の `auto` のときは `[tui]` を書き出さない)。確認が出たら内容を見て保存してよい

(c) 明るい背景: Windows Terminal の別のタブで明るい背景の配色 (設定 > 配色。例 One Half Light) にする。`Copy-Item "$dir\example-config.toml" $cfg -Force` で設定を戻してから、`& $cu --config=$cfg` で起動する:
- [ ] `auto` は背景の明暗を判定せず、`calm` を選ぶ。明るい背景では文字が読みにくい (正しい。覆る条件は D-129)
- [ ] 「テーマ」で `Enter` を 2 回 → `calm-light`。補足の灰色の文字、枠、キー名、allow の青、deny の赤が、明るい背景から読める。未保存のときの琥珀 (`s 保存  u 元に戻す`) も読める

(d) VT を使えないコンソール: レガシコンソールを使う設定の conhost (conhost のタイトルバーのメニュー > プロパティ > オプション >「レガシ コンソールを使う」を有効にして、開き直す) で、`cmd.exe` から起動する。この PC でレガシコンソールにできない、または起動した画面で VT が使えてしまうなら、そう記録して飛ばす (VT が無いときのテーマの選び方は `cargo test` が値で確かめているが、実際のコンソールでの色は人の目でしか確かめられない)。`Copy-Item "$dir\example-config.toml" $cfg -Force` で設定を戻してから:

```
"<repo>\target\release\ai-cu.exe" --config=<repo>\target\manual-check\s32-config.toml
```
- [ ] `theme` が `auto` のとき、色は 16 色 (`ansi`) で、黒地に黒のような読めない画面にならない。「テーマ」の値は `(auto: ansi)`。起動時のメッセージの行に警告は出ない (`auto` が選んだので)
- [ ] 「テーマ」で `Enter` → `calm`: 値が `calm→ansi`、配色は `ansi` のまま。メッセージの行に端末の黄色 (配色が `ansi` のため。琥珀色ではない) で「VT が使えないので ansi の配色で表示している」が出る。`Enter` で `calm-light` に進めても同じ (値は `calm-light→ansi`)。`ansi` に進めると警告が消え、値は `ansi`
- [ ] `calm` にして `s` で保存して終了し、同じ conhost で起動し直す → 起動時のメッセージの行に同じ警告が出て、配色は `ansi`。`Get-Content` で設定の値が `calm` のまま (書き換えられていない)

(e) `NO_COLOR`: 手順 (a) と同じ Windows Terminal で、設定を戻してから起動する:

```powershell
Copy-Item "$dir\example-config.toml" $cfg -Force
$env:NO_COLOR = "1"; & $cu --config=$cfg; Remove-Item Env:NO_COLOR
```
- [ ] `theme` が `auto` のとき、色が付かない (`mono`)。「テーマ」の値は `(auto: mono)`。選んでいる行の反転、枠の太さ、記号で区別できる
- [ ] 「テーマ」で `Enter` → `calm`: `NO_COLOR` があっても、明示した `calm` の色が付く (明示は `NO_COLOR` より優先)。`Enter` を続けて `auto` に戻すと、また色が消える

後片付け: `$env:NO_COLOR` が残っていないこと (`$env:NO_COLOR` が空)。`target/manual-check/s32-*` は残してよい (git 管理外)。実施の記録には、見た端末 (Windows Terminal か conhost、背景の明暗)、各テーマの見え方で読みにくかった箇所、(d) をレガシコンソールで再現できたかを書く

## 33. IME のキー (D-131)

MS-IME が有効なメモ帳で、`key` の IME のキー (`kanji`、`imeon`、`imeoff`、`convert`、`nonconvert`) に MS-IME が反応することと、`type` の文字が未確定に入らないことを確かめる。ai-cu に IME の状態を読む機能は無いので、IME の表示は人が目で見て AI に伝える。
手順 0 の設定 (基本の層。allow に notepad) のまま行い、設定ファイルを作らない。
メモ帳に実入力を送るので、準備と GO の書き方は手順 31 と同じ: `AI_CU_MANUAL_CHECK=1` のセッションで、release のパスを文字どおり書き、GO を 1 回もらってから行う。操作は一手ずつの `focus`、`key`、`type` だけ (`run` は使わない)。送る入力はメモ帳の中だけで、撮影はしない。本文は `text` で読む。

GO の前に `target/release/ai-cu.exe config show` で確かめる (読むだけ):
- `allow.actions` を書いているなら、`focus`、`key`、`type` がある
- `allow.keys` を書いているなら、`kanji`、`imeon`、`imeoff`、`convert`、`nonconvert`、`a`、`i`、`space`、`esc` がある
- 無ければ 4 になるので、AI は設定を書き換えず、そのまま記録して止まる

準備:
- 人が空の新しいメモ帳のウィンドウを開く (手順 31 と同じ。題名「タイトルなし」のウィンドウが 1 つだけであることを `windows` で確かめる)。`focus` は題名を「メモ帳」で探す
- 人がそのメモ帳を前面にして、タスクバーの IME の表示 (「あ」か「A」) を読み、確認の前の状態として AI に伝える (AI は記録する。後片付けで戻す)
- 人は GO と区切りごとの返事のほかは、マウスとキーに触れない (マウスを動かすと 6、予告の間にキーを押すと 10 で止まる)。返事を打つと前面が端末に戻り、最後の入力も更新されるので、各区切りの先頭で `focus` を打つ。`focus` が 10 (`user_active`) なら何も送っていないので、数秒待って打ち直す
- 各コマンドの終了コードは、期待の書いていないものも `0` のはず。違えばそのまま記録して止まる

(a) `kanji` で IME のオンとオフが切り替わる。次の 2 行を打ち、人にタスクバーの IME の表示を聞く:

```
target/release/ai-cu.exe focus notepad.exe --title メモ帳
target/release/ai-cu.exe key kanji
```
- [ ] 表示が押す前と逆になった (「あ」と「A」が入れ替わった)
- [ ] 同じ 2 行をもう一度打つと、表示が元に戻った

(b) `imeoff` と `imeon` で状態を決め打ちできる。次の `<キー>` を `imeoff`、`imeon`、`imeon` の順に替えて 3 回打ち、1 回ごとに人に表示を聞く:

```
target/release/ai-cu.exe focus notepad.exe --title メモ帳
target/release/ai-cu.exe key <キー>
```
- [ ] 1 回目 (`imeoff`) の後は「A」
- [ ] 2 回目 (`imeon`) の後は「あ」
- [ ] 3 回目 (もう一度 `imeon`) の後も「あ」のまま

(c) IME がオンのとき、`key` で未確定の文字と変換に入る ((b) の続きで IME はオン)。未確定の間に端末へ返事を打つと、前面が移って IME が未確定を確定か破棄しうる (推定) ので、AI は次を `target/manual-check/s33-c.sh` にまとめ、Bash ツールで `sh target/manual-check/s33-c.sh; echo "exit=$?"` として 1 回で打つ。人はメモ帳を見たまま、終わるまで返事を打たない:

```
set -e
target/release/ai-cu.exe focus notepad.exe --title メモ帳
target/release/ai-cu.exe key a
target/release/ai-cu.exe key i
target/release/ai-cu.exe key space
target/release/ai-cu.exe key convert
target/release/ai-cu.exe key nonconvert
target/release/ai-cu.exe key esc
target/release/ai-cu.exe key esc
target/release/ai-cu.exe key esc
```
- [ ] `exit=0`
- [ ] `key a` と `key i` で、下線つきの未確定の文字 (「あい」) が出た
- [ ] `key space` で変換に入り、`key convert` と `key nonconvert` のそれぞれで IME が反応した (候補や読みの種類の変化。見えたものをそのまま記録する)
- [ ] `key esc` の後、人が未確定の文字の無いことを確かめた。残っていれば `focus` の後に `key esc` を足し、未確定が消えるまで打つ

(d) 未確定の無い状態で、IME がオンでも `type` の文字は未確定にならない。人に IME の表示が「あ」のままであることを確かめてから打つ:

```
target/release/ai-cu.exe focus notepad.exe --title メモ帳
target/release/ai-cu.exe type aiueo
target/release/ai-cu.exe text --window notepad.exe --type document
```
- [ ] (人の目) `aiueo` が下線なしでそのまま入った (「あいうえお」にも未確定にもならない)
- [ ] `text` の本文が `aiueo`。(c) の未確定が確定して残っていれば、その前に文字が付くので、そのまま記録する

記録する値: 使った release の版 (`target/release/ai-cu.exe --version`)、(a) から (d) の人の目の結果、各コマンドの終了コード、(d) の `text` の本文。「実施の記録」の節のとおり、手元に日付つきで書く。
後片付け: 人が IME を確認の前の状態に戻し、メモ帳を保存せずに閉じる。`target/manual-check/s33-*` は残してよい (git 管理外)

結果が違ったときは、D-131 の覆る条件に戻る: (a) で `kanji` に反応しなければ VK_OEM_AUTO / VK_OEM_ENLW を試す、(b) で `imeon` / `imeoff` に反応しなければ表から外す、どのキーも届かなければスキャンコードの指定を考える、(d) で文字が未確定に入ったら手引き、参照、説明の文を直す。(c) で未確定に入ると確かめられたら、手引きに足す。

## 結果が違ったとき

- 手順 1 で `dev_mode: true`: この端末に `AI_CU_DEV_FORBID_REAL_INPUT` がある。`Remove-Item Env:AI_CU_DEV_FORBID_REAL_INPUT` で外す
- 手順 7 で終了コード `7`: メッセージの `process=` を見る。メモ帳のプロセス名が `notepad.exe` 以外 (Microsoft Store 版など) なら、その名前を `allow.processes` に足す
- 手順 7 で終了コード `6`: 待ちの間にマウスが動いた。Alt+Tab だけで切り替える
- 手順 7 で文字化けや欠け: 出力の JSON と、入らなかった文字を控えて知らせる

## AI のセッションから CLI の手順を確かめるとき

TUI の手順 (5 と 13 の本体、14 の TUI の部分、32) は端末が要るので人が行う。それ以外は、AI のセッションから release の
ai-cu.exe を呼んで確かめられる。開発セッション (このリポジトリ) からは、`/verify` の全手順を通した `target/release/ai-cu.exe` を
パスで呼ぶときだけ hook が通す (D-051)。それも、人が Claude Code を起動したシェルに `AI_CU_MANUAL_CHECK=1` を付けた
セッションに限る (PowerShell なら `$env:AI_CU_MANUAL_CHECK = "1"` の後に `claude`)。実機に触れる一連の手順の前に、人の GO を 1 回もらい、その間は人がマウスとキーボードから
手を離す。別のディレクトリで開いたセッション (hook 無し) に頼む形も使える (2026-10-08 に実施)。そのときの依頼文に書くこと:

- 使うのは release の ai-cu.exe だけ。リポジトリを編集しない、cargo を走らせない
- 実際の設定ファイルは読むだけ。手順 9 と 10 は `--config=` の上書き層 (deny と stop だけを足す。`version = 1` と `[general] input_enabled = true` が要る) で行い、allow を広げる設定は作らない。手順 23 も上書き層で行う (`version = 1`、`[general] capture_enabled = true`、deny だけ)。手順 24 も上書き層で行う (`version = 1`、`[general] capture_enabled = true`、`[mcp] require_approval = ["windows"]` だけ。登録と確認の画面は人が行う)。手順 25 は `guide --setup` と `validate` だけで、設定ファイルを作らない (TUI の左の一覧で fix の場所を探すのは人)。手順 26 は基本の層のまま行い、設定ファイルを作らない (基本の層の `allow.actions` を書いた設定では `wait_stable` / `wait_change` を並べないと 4 になる。そのときは記録して止まる)。手順 27 は基本の層のまま行い、設定ファイルを作らない (撮影の許可が要る。実入力は送らない)。手順 28 は確認専用のアプリ pw-probe (`tools/pw-probe`、D-092) で行う。人がビルドし、基本の層の `allow.processes` に `pw-probe.exe` がある (入れたままでよい。無ければ人が TUI で足す) ことを確かめてから、(a) の読み取りは GO 無しで、(b) から (d) は GO をもらって行い、設定ファイルを作らない (`general.password_input` は書かない)。送る文字は無害な 1 文字 (`x`、`a`、`b`、`y`)。読めないアプリでは 4 にならず文字が入るので、先に (a) の `find` で `secret` の `password` が true になることを確かめてから (b) を行う。pw-probe は AI が起動してよい (起動は実入力ではない)。手順 29 は基本の層のまま行い、(d) だけ上書き層 (`version = 1`、`[general] input_enabled = true`、`[indicator] sound = true` だけ) を `target/manual-check/` に作る。枠は撮影に写らないので、枠と札の見え方と音は人に聞いて記録する。手順 30 は基本の層のまま行い、設定ファイルを作らない。ignored の段がフィクスチャへ実入力を送るので GO の後に行う。枠と札は撮影に写らないので、人に聞いて記録する。cargo を走らせるので、別のディレクトリのセッションへの依頼には入れず、開発セッションの主エージェントが行う (D-111)。手順 33 は基本の層のまま行い、設定ファイルを作らない。一手ずつの `focus`、`key`、`type` は実入力なので GO の後に行う。IME の表示は撮影では確かめず、人に聞いて記録する (D-131)
- `--dry-run` の無い `run` は手順 6、7、8、9、20、26、28 (c)、29、31 だけ (26 の `run` は `focus` と画面の待ちだけで、入力は送らない。28 (c) は pw-probe のふつうの欄に Tab を含む `type` を送る標準入力の `run -`。28 の (b) と (d) は `run` ではなく一手ずつの `focus`、`click`、`key`、`type` で、これも実入力。31 は基本の層のまま (allow に notepad) で、設定ファイルを作らずに 2 本の `run` を同時にメモ帳へ送る実入力なので、GO をもらってから行う。33 は `run` を使わないが、28 の (b) と (d) と同じく一手ずつの `focus`、`key`、`type` が実入力)。7 と 8 は実行の直前にターンを終え、人の GO を待つ (GO の後、人は枠が出てから約 3 秒 (予告の間) 待って Alt+Tab でメモ帳を前面にし、マウスに触れない)。7 は `sample-with-focus.json` (sample.json の先頭に `focus` を足したもの) を使えば、端末が前面のままで済み、Alt+Tab の合図が要らない (2026-10-09 に実施)
- 手順 2 の前提 (allow に notepad があり、端末と端末を内蔵するエディタが allow に無く deny にある、input_enabled が true、dev_mode が false) が崩れていたら、run 系を走らせずに止まる。allow に `pw-probe` や利用者がふだん使うアプリがあるのは崩れではない
- 画像は全画面を撮らない。手順 12 の画像は寸法だけ調べて消す。メモ帳の内容は `windows` で矩形を取り、その範囲だけ撮って読んでよい
- コマンド、終了コード、出力をそのまま記録し、期待と違っても直さず報告する
- コマンドは Bash ツールで走らせる (PowerShell 5.1 は標準エラーの扱いで終了コードの表示が壊れる)

## 実施の記録

実施の記録 (日時、版、結果、写した値) は、実施した環境が分かるので公開しない。実施した人の手元 (git で管理しない場所) に残す。
