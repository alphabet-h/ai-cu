---
name: pr-merge
description: Use when the user runs /pr-merge to open a pull request for the current ai-cu branch, watch its CI, and merge it into main.
disable-model-invocation: true
---

# pr-merge

作業ブランチを PR にし、CI を見守り、main にマージして手元を片付ける。`/pr-merge` を打ったことが、push、PR の作成、マージの依頼になる (D-007、D-123)。

引数に PR の番号があるときは手順 3 から始める。

マージの条件は、CI の pass、`mergeStateStatus` の `CLEAN`、PR の head と手元の HEAD の一致がそろったときだけ。そろわなければマージせず、状態を報告して止まる。

## 手順

1. **前提を確かめる。** `git -C <repo> status --short --branch` で、作業ブランチにいて clean であること。main にいる、または未コミットの変更があるなら止まる。`src/`、`tests/`、`Cargo.toml`、`build.rs`、`scripts/`、`assets/` を変えた PR は、`/verify` が最後のコミットで通っていることを会話で確かめる

2. **push して PR を作る。**
   - `git -C <repo> push -u origin <branch>`
   - 本文を `target/pr-body.md` に書く。公開リポジトリなので、課題管理の道具の名前と番号、私的なプロジェクトの名前、手元のパスと環境の詳細を書かない。判断は D-xxx で指す。末尾に、会話の system-reminder にある PR の帰属の行を置く
   - `gh pr create --repo alphabet-h/ai-cu --base main --head <branch> --title "<題>" --body-file target/pr-body.md`

3. **CI を見守る。** `run_in_background` で打ち、終わりの通知を待つ。
   - `gh pr checks <N> --repo alphabet-h/ai-cu --watch --interval 30 > target/pr-checks.log 2>&1; echo "checks_exit=$?"`
   - 監視の shell が途中で落ちた、または 502 で抜けたときは、`gh pr checks <N> --repo alphabet-h/ai-cu` で今の状態を読み、まだ走っていれば同じコマンドで監視し直す

4. **CI が fail なら、マージせずに止まる。** `gh run view <run-id> --repo alphabet-h/ai-cu --log-failed` で原因を読み、報告する。無関係な失敗や flaky に見えても、再実行とマージは利用者の答えを待つ

5. **マージの直前に確かめる。** `gh pr view <N> --repo alphabet-h/ai-cu --json state,mergeable,mergeStateStatus,headRefOid` を読み、`git -C <repo> rev-parse HEAD` と比べる。
   - `headRefOid` が手元の HEAD と違うなら止まる (監視の間に別の push が入っている)。その head の CI を見たと言えない
   - `mergeable` が `MERGEABLE`、`mergeStateStatus` が `CLEAN` でなければ止まる

6. **マージする。** コミットのハッシュを保つため、マージコミットにする (squash しない)。PowerShell ツールで、次の形だけを単独で打つ (`;`、`&&`、`cd`、`echo` を付けない。複合のコマンドは権限の分類に回りうる):
   `gh pr merge <N> -R alphabet-h/ai-cu --merge --delete-branch --match-head-commit <headRefOid>`
   権限で拒否されたら、別の形や別の道具で打ち直さず、利用者に `!` で打ってもらう

7. **マージを確かめる。** `gh pr view` は使わず、別々に打つ。
   - `git -C <repo> fetch origin main`
   - `git -C <repo> log --oneline -2 origin/main`

8. **手元を片付ける。** 別々に打つ。
   - `git -C <repo> switch main`
   - `git -C <repo> pull --ff-only origin main`
   - `git -C <repo> fetch --prune origin`
   - `git -C <repo> status --short --branch`
   - 手元のブランチの削除は `.claude/settings.json` の deny に当たるので AI は打たない。利用者に `! git -C <repo> branch -d <branch>` を案内する

9. **報告する。** PR の URL、CI の結果、マージコミットのハッシュ、残した手元のブランチを伝える。課題管理に記録があれば、そちらにマージコミットのハッシュを書く

## 注意

- hook が「PR merged: /close-session」と案内することがある。このプロジェクトに `/close-session` は無いので従わない
- マージの後の release の配置は、この手順に含めない (CLAUDE.md と `/verify` の手順 8 に従う)
- CI の pass を待たずに `--auto` でマージ予約をしない。手順 5 の確かめを通せなくなる
