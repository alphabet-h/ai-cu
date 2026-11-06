"""/verify を通った release (target/release/ai-cu.exe) を ~/.cargo/bin に配置する (D-064、D-069)。

配置するのは、clean な木で HEAD と同じコミットから作った release だけ (D-069)。次のどれかなら配置せず、
終了コード 1 で断る。断る文には条件ごとの次の手を添える。--allow-dirty のような抜け道は無い。
  (1) 作業ツリーに未コミットの変更がある (git status --porcelain --untracked-files=normal が空でない)
      → コミットして作業ツリーを clean にしてから、cargo build --release を打ち直し、もう一度配置する
  (2) git が読めない (無い、リポジトリでない、時間切れ)
      → git が読める場所 (このリポジトリ) で cargo build --release を打ち直してから配置する
  (3) release の --version が動かない (起動できない、時間切れ、終了コードが 0 でない) か、
      版の印が無い (この変更より前の release など) か、印が +unknown か .dirty
      → +unknown は (2) と同じ。動かない、印なし、.dirty は、cargo build --release を打ち直してから配置する
  (4) 印のコミットが git rev-parse HEAD と前方一致しない
      → cargo build --release を打ち直してから配置する
順は「/verify → コミット → clean な木で cargo build --release を打ち直し → この script」。
版の印の文法は src/build_info.rs (ai-cu <semver>+g<コミットの先頭 12 桁>[.dirty] か +unknown)。

動いている実行ファイル (常駐の MCP サーバー) は上書きも削除もできないが、名前は変えられる。
そこで既にある ai-cu.exe を ai-cu.old-<時刻>.exe に改名してから新しいものを置き、消せる古いものは消す。
動いているサーバーは、再接続するまで古い版のまま動く。

置いたときは .dev/install-log.jsonl (git 管理外) に 1 行の JSON (at、version、commit、sha256、src、dest)
を追記する。「同じ版が既にある」ときは記録しない。

使い方: python scripts/install.py [--src PATH] [--dest-dir DIR] [--log PATH]
既定は、このリポジトリの target/release/ai-cu.exe、~/.cargo/bin、.dev/install-log.jsonl。
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

NAME = "ai-cu.exe"
OLD_PREFIX = "ai-cu.old-"

# `ai-cu --version` の出力。版の本体は semver (pre-release の `-rc1` なども通す)、build metadata の `+` は 1 つ
VERSION_RE = re.compile(
    r"^ai-cu (?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"
    r"\+(?:g(?P<commit>[0-9a-f]{12})(?P<dirty>\.dirty)?|unknown))\s*$"
)

# 断ったときの次の手 (D-069)
NEXT_COMMIT = (
    "コミットして作業ツリーを clean にしてから、cargo build --release を打ち直し、もう一度配置する (D-069)"
)
NEXT_REBUILD = "cargo build --release を打ち直してから (build.rs が印を埋め直す)、もう一度配置する (D-069)"
NEXT_REBUILD_IN_REPO = (
    "git が読める場所 (このリポジトリ) で cargo build --release を打ち直してから配置する (D-069)"
)


@dataclass(frozen=True)
class Stamp:
    """`--version` から読んだ版の印。`unknown` は `commit12` が None"""

    version: str
    commit12: str | None
    dirty: bool


def parse_version(text: str) -> Stamp | None:
    """`ai-cu --version` の出力を読む。形が違えば None"""
    m = VERSION_RE.match(text)
    if m is None:
        return None
    return Stamp(m["version"], m["commit"], m["dirty"] is not None)


def release_problem(stamp: Stamp | None, head: str) -> tuple[str, str] | None:
    """release の印が配置できない理由と次の手。配置できるなら None。
    木が clean なことは呼び出し側が先に見ているので、ここでは「コミットして」を言わない"""
    if stamp is None:
        return (
            "release の --version に版の印が無い (この変更より前の release か、形が違う)",
            NEXT_REBUILD,
        )
    if stamp.commit12 is None:
        return ("release の版の印が +unknown (git を読めない環境で作られた)", NEXT_REBUILD_IN_REPO)
    if stamp.dirty:
        return ("release は未コミットの変更がある木で作られた (.dirty)", NEXT_REBUILD)
    if not head.startswith(stamp.commit12):
        return (f"release のコミット {stamp.commit12} が HEAD {head[:12]} と違う", NEXT_REBUILD)
    return None


def git(root: Path, *args: str) -> str:
    """`git -C <root> <args>` の標準出力 (末尾の改行だけ落とす。行頭の空白は status の X 列なので保つ)。
    終了コードが 0 でない、git が無い、時間切れ、のどれも RuntimeError。
    `GIT_OPTIONAL_LOCKS=0` で、status が index を書き直さないようにする (build.rs と同じ)"""
    try:
        out = subprocess.run(
            ["git", "-C", str(root), *args],
            capture_output=True,
            text=True,
            timeout=30,
            env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"},
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        raise RuntimeError(str(e)) from e
    if out.returncode != 0:
        lines = (out.stderr or "").strip().splitlines()
        reason = lines[0] if lines else f"git {' '.join(args)} が終了コード {out.returncode}"
        raise RuntimeError(reason)
    return out.stdout.rstrip("\r\n")


def tree_changes(root: Path) -> list[str]:
    """作業ツリーの未コミットの変更 (`git status --porcelain` の各行)。clean なら空。
    scripts/verify.py も使う (D-083)"""
    lines = git(root, "status", "--porcelain", "--untracked-files=normal").splitlines()
    return [line for line in lines if line]


def head_commit(root: Path) -> str:
    """`git rev-parse HEAD` (40 桁)。
    scripts/verify.py も使う (D-083)"""
    return git(root, "rev-parse", "HEAD").strip()


def read_stamp(exe: Path) -> Stamp | None:
    """実行ファイルの `--version` から版の印を読む。動くが形が違う (印が無い) なら None。
    動かない (起動できない、時間切れ、終了コードが 0 でない) なら RuntimeError (理由つき)。
    「印が無い」と「動かない」を分けるのは、断る文に本当の原因を出すため (次の手はどちらも打ち直し)"""
    try:
        out = subprocess.run([str(exe), "--version"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise RuntimeError(str(e)) from e
    if out.returncode != 0:
        raise RuntimeError(f"終了コード {out.returncode}")
    return parse_version(out.stdout)


def record(log: Path, entry: dict) -> None:
    """配置の記録を 1 行の JSON で追記する (親のディレクトリが無ければ作る)"""
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def missing_source(src: Path) -> str:
    return f"配置する実行ファイルが無い: {src} (先に cargo build --release)"


def remove_old(dest_dir: Path) -> list[str]:
    """改名して残した古い版のうち、消せるもの (もう動いていないもの) を消す。消せなかった名前を返す。"""
    kept = []
    for p in sorted(dest_dir.glob(OLD_PREFIX + "*.exe")):
        try:
            p.unlink()
        except OSError:
            kept.append(p.name)
    return kept


def install(src: Path, dest_dir: Path) -> tuple[Path, bool]:
    """src を dest_dir/ai-cu.exe に置く。戻りは (配置先, 置いたか)。「同じ版が既にある」なら置かない"""
    if not src.is_file():
        raise SystemExit(missing_source(src))
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / NAME
    tmp = dest_dir / (NAME + ".new")
    shutil.copy2(src, tmp)
    if dest.exists():
        if sha256(dest) == sha256(tmp):
            tmp.unlink()
            print(f"同じ版が既にある: {dest}")
            return dest, False
        # 動いていても改名はできる。時刻を付けて、動いている古い版どうしがぶつからないようにする
        dest.rename(dest_dir / f"{OLD_PREFIX}{time.time_ns()}.exe")
    tmp.rename(dest)
    return dest, True


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--src", type=Path, default=root / "target" / "release" / NAME)
    ap.add_argument("--dest-dir", type=Path, default=Path.home() / ".cargo" / "bin")
    ap.add_argument("--log", type=Path, default=root / ".dev" / "install-log.jsonl")
    a = ap.parse_args()

    # (1) 木と HEAD。git が読めなければ「コミットして」は案内しない
    try:
        changes = tree_changes(root)
        head = head_commit(root)
    except RuntimeError as e:
        print(f"git で作業ツリーを確かめられない: {e}。{NEXT_REBUILD_IN_REPO}", file=sys.stderr)
        return 1
    # (2) 未コミットの変更
    if changes:
        shown = ", ".join(changes[:5]) + (" ..." if len(changes) > 5 else "")
        print(
            f"作業ツリーに未コミットの変更がある ({len(changes)} 件) ので配置しない: {shown}。{NEXT_COMMIT}",
            file=sys.stderr,
        )
        return 1
    # (3) release の有無
    if not a.src.is_file():
        print(missing_source(a.src), file=sys.stderr)
        return 1
    # (4) release の印と HEAD の照合。動かないものは、印が無いのとは別の文で断る (次の手は同じ)
    try:
        stamp = read_stamp(a.src)
    except RuntimeError as e:
        print(f"release の --version が動かない ({e}) ので配置しない。{NEXT_REBUILD}", file=sys.stderr)
        return 1
    problem = release_problem(stamp, head)
    if problem is not None:
        print(f"{problem[0]} ので配置しない。{problem[1]}", file=sys.stderr)
        return 1
    assert stamp is not None

    # (5) 配置
    kept = remove_old(a.dest_dir) if a.dest_dir.is_dir() else []
    dest, placed = install(a.src, a.dest_dir)
    out = subprocess.run([str(dest), "--version"], capture_output=True, text=True, timeout=10)
    if out.returncode != 0:
        print(f"配置したものが動かない: {dest} (終了コード {out.returncode})", file=sys.stderr)
        return 1
    # (6) 印字と (7) 記録 (置いたときだけ)
    digest = sha256(dest)
    print(f"配置した: {dest} ({out.stdout.strip()}, sha256 {digest[:12]})")
    if placed:
        record(
            a.log,
            {
                "at": datetime.now().astimezone().isoformat(timespec="seconds"),
                "version": stamp.version,
                "commit": head,
                "sha256": digest,
                "src": str(a.src),
                "dest": str(dest),
            },
        )
    if kept:
        print(f"動いている古い版は残した (MCP を再接続すると次の配置で消える): {', '.join(kept)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
