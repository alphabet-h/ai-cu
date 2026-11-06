# pw-probe

手動確認の手順 28 (パスワード欄の守り、docs/decisions.md の D-090) のための、確認専用の WPF のアプリ。
ふつうの欄とパスワード欄と、パスワードの長さの表示だけを持つ。置いた判断は D-092。

WPF の PasswordBox は、フィクスチャの Win32 の `ES_PASSWORD` の EDIT と別の UIA の提供側なので、
ai-cu がパスワードの要素 (UIA の IsPassword) を読まずに飛ばすこと、パスワード欄へのキーの送信を既定で断ることを、別の実物で確かめられる。

## ビルド

.NET 9 の SDK (WPF を含む Windows 用) で作る。NuGet の依存は無い。cargo と `scripts/verify.py` の対象には入れていない。

```
dotnet build tools/pw-probe/pw-probe.csproj -c Release
```

exe は `tools/pw-probe/bin/Release/net9.0-windows/pw-probe.exe`。`bin/` と `obj/` は .gitignore で無視している。

## 窓と要素

窓の題は `ai-cu pw-probe`。小さな窓 (420x220。WPF の論理単位で、表示倍率 125% では物理でおよそ 525x275) を画面の中央に出す。起動時のフォーカスはふつうの欄で、Tab の順は ふつうの欄 → パスワード欄。

| 要素 | 種類 | AutomationId | 名前 | 初期値 |
|------|------|--------------|------|--------|
| ふつうの欄 | TextBox | `plain` | ふつうの欄 | `plain-text` |
| パスワード欄 | PasswordBox | `secret` | パスワード欄 | `probe-secret` (12 文字) |
| 長さの表示 | TextBlock | `length` | パスワードの長さ: N | パスワードの長さ: 12 |

- 長さの表示は、パスワード欄の中身が変わるたび (PasswordChanged) に「パスワードの長さ: N」に更新する。UIA の名前も同じ文字列にするので、パスワード欄に文字が入ったかを、中身を見ずに `find` の名前で確かめられる
- 各欄の上の見出し (「ふつう (plain)」「パスワード (secret)」) は、欄の名前を含まない文字列にしてある。`find` の名前は部分一致なので、含むと見出しも当たる
- ふつうの欄から `type` で Tab を含む文字列を送ると、途中でパスワード欄に移る。D-090 の追記の「Tab の後の束で判定をやり直す」を実物で撃てる

## 使い方

docs/manual-check/CHECKLIST.md の手順 28 から使う。実入力を伴うので、手順のとおり利用者の GO の後に、`/verify` を通した release の `target/release/ai-cu.exe` で行う。

## 注意

- **本物のパスワードを入れない。** 初期値は試し用の固定の文字列で、確認の途中でも本物の資格情報を打たない
- 許可に足すときはプロセス名 `pw-probe.exe` (または `pw-probe`) で足す。固有の名前なので、他のアプリには届かない
