"""assets/icon/ の SVG から、exe に埋め込む ICO とソーシャルプレビューの PNG を作る (D-127)。

開発者が手で打つ道具で、CI でもビルドでも走らせない。生成物の assets/icon/ai-cu.ico は git で追跡する。
要るもの: Microsoft Edge (ヘッドレスで SVG を PNG に描く) と Pillow (縮める)。

手順:
  1. 各 SVG の根の width/height を描く大きさ (アイコンは 1024、ソーシャルプレビューは 1280x640) に
     書き換えた写しを target/icon/ に作る (Chromium は width/height を持つ SVG をその大きさで左上に描くため。
     docs/design-notes.md)
  2. 写しを Edge のヘッドレスで同じ大きさの窓に描き、target/icon/ に PNG を作る (透明の地。プロファイルは
     target/icon/edge-profile に閉じる)
  3. 描けた PNG の不透明な画素の外接矩形が地の範囲 (アイコンは 32..992、前後 2px) と合うことを確かめる
  4. Pillow で縮め、小さい版 (字だけ) から 16/20/24/32、大きい版から 48/64/128/256 の PNG を作り、
     PNG を詰めた ICO (Vista 以降が読む形) を assets/icon/ai-cu.ico に書く
  5. ソーシャルプレビューの PNG は target/icon/social-preview.png に置くだけ (GitHub の設定から利用者が上げる)
どこかで失敗したら (Edge が非 0 で終わる、時間切れ、PNG が無い、外接矩形が合わない) 終了コード 1 で終わり、
ICO は書かない。

使い方: python scripts/make_icon.py [--edge PATH]
Edge は既定の場所 (Program Files (x86) と Program Files の Microsoft/Edge/Application/msedge.exe) を探す。

モジュールの読み込みでは標準ライブラリだけを使う (Pillow は生成の経路の関数の中で import する。
CI は Pillow を入れずに scripts/test_make_icon.py を走らせる)。
"""

import argparse
import io
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
ICON_DIR = ROOT / "assets" / "icon"
OUT_DIR = ROOT / "target" / "icon"
ICO_PATH = ICON_DIR / "ai-cu.ico"

# 小さい版 (カーソルが判らなくなる大きさ。字だけ) と大きい版 (L1) の大きさ (D-127)
SMALL_SIZES = (16, 20, 24, 32)
LARGE_SIZES = (48, 64, 128, 256)

RENDER = 1024  # アイコンを Edge で描く大きさ
ICON_BBOX = (32, 32, 992, 992)  # 1024 で描いた地 (256 の座標の 8..248) の外接矩形 (右と下は含まない)
SOCIAL_SIZE = (1280, 640)
BBOX_TOLERANCE = 2
EDGE_TIMEOUT_S = 60

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

_ROOT_TAG = re.compile(r"<svg(?=[\s>/])[^>]*>")
_COMMENT = re.compile(r"<!--.*?-->", re.S)


def _replace_attr(tag: str, name: str, value: str) -> str:
    # 前に空白を求めて stroke-width などの別の属性に当てない
    pat = re.compile(r'(\s)' + name + r'\s*=\s*("[^"]*"|\'[^\']*\')')
    if not pat.search(tag):
        raise ValueError(f"SVG の根に {name} の属性が無い")
    return pat.sub(lambda m: f'{m.group(1)}{name}="{value}"', tag, count=1)


def set_svg_size(svg_text: str, w: int, h: int) -> str:
    """SVG の根の width/height だけを書き換えた文字列を返す。根に属性が無ければ ValueError。"""
    # コメントの中の <svg を根と取り違えないよう、コメントを同じ長さの空白に伏せて探す
    masked = _COMMENT.sub(lambda c: " " * len(c.group(0)), svg_text)
    m = _ROOT_TAG.search(masked)
    if m is None:
        raise ValueError("SVG の根 (<svg ...>) が無い")
    tag = _replace_attr(svg_text[m.start() : m.end()], "width", str(w))
    tag = _replace_attr(tag, "height", str(h))
    return svg_text[: m.start()] + tag + svg_text[m.end():]


def build_ico(entries: list[tuple[int, bytes]]) -> bytes:
    """大きさと PNG のバイト列の組から、PNG を詰めた ICO を組む。

    ICONDIR (予約 0、種類 1、数 n)、各 ICONDIRENTRY (幅と高さは 256 を 0 で書く、色数 0、予約 0、
    planes 1、bitcount 32、大きさ、オフセット)、その後に PNG を順に置く。
    """
    if not entries:
        raise ValueError("ICO に入れる項目が無い")
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dir_bytes = b""
    for size, data in entries:
        if not 1 <= size <= 256:
            raise ValueError(f"ICO の大きさは 1..256 (受けたのは {size})")
        side = 0 if size == 256 else size  # ICONDIRENTRY の 1 バイトでは 256 を 0 で表す
        dir_bytes += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return header + dir_bytes + b"".join(data for _, data in entries)


def parse_ico(data: bytes) -> list[tuple[int, int, bytes]]:
    """ICO を読み戻し、(幅, 高さ, 中身) の並びを返す。幅と高さの 0 は 256 として返す。"""
    if len(data) < 6:
        raise ValueError("ICO の頭が短い")
    reserved, kind, count = struct.unpack_from("<HHH", data, 0)
    if reserved != 0 or kind != 1:
        raise ValueError(f"ICO の頭が違う (予約 {reserved}、種類 {kind})")
    if len(data) < 6 + 16 * count:
        raise ValueError("ICO の項目の表が短い")
    out = []
    for i in range(count):
        w, h, _colors, _res, _planes, _bits, size, offset = struct.unpack_from(
            "<BBBBHHII", data, 6 + 16 * i
        )
        if offset + size > len(data):
            raise ValueError(f"ICO の項目 {i} が末尾を越える")
        out.append((w or 256, h or 256, data[offset : offset + size]))
    return out


def bbox_matches(bbox, expected, tol: int = BBOX_TOLERANCE) -> bool:
    """外接矩形 (左, 上, 右, 下) が期待と前後 tol 以内で合うか。bbox が None (透明だけ) なら偽。"""
    if bbox is None:
        return False
    return all(abs(a - b) <= tol for a, b in zip(bbox, expected))


def find_edge(explicit: str | None) -> Path | None:
    if explicit:
        p = Path(explicit)
        return p if p.is_file() else None
    for env, default in (
        ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        ("ProgramFiles", r"C:\Program Files"),
    ):
        base = Path(os.environ.get(env) or default)
        p = base / "Microsoft" / "Edge" / "Application" / "msedge.exe"
        if p.is_file():
            return p
    return None


class MakeIconError(Exception):
    pass


def render(edge: Path, svg: Path, png: Path, w: int, h: int, profile: Path) -> None:
    """SVG (根の width/height が w,h の写し) を Edge のヘッドレスで w,h の窓に描き、png に書く。"""
    if png.exists():
        png.unlink()  # 古い PNG で先に進まない
    cmd = [
        str(edge),
        f"--user-data-dir={profile}",
        "--no-first-run",
        "--headless=new",
        "--disable-gpu",
        "--hide-scrollbars",
        "--default-background-color=00000000",
        f"--window-size={w},{h}",
        f"--screenshot={png}",
        svg.resolve().as_uri(),
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, timeout=EDGE_TIMEOUT_S)
    except subprocess.TimeoutExpired as e:
        raise MakeIconError(f"Edge が {EDGE_TIMEOUT_S} 秒で終わらなかった ({svg.name})") from e
    if proc.returncode != 0:
        tail = proc.stderr.decode("utf-8", "replace").strip().splitlines()[-3:]
        raise MakeIconError(f"Edge が終了コード {proc.returncode} で終わった ({svg.name}): {tail}")
    if not png.is_file():
        raise MakeIconError(f"Edge が PNG を書かなかった ({png})")


def render_checked(edge: Path, name: str, w: int, h: int, expected_bbox, profile: Path):
    """assets/icon/<name>.svg を w,h に書き換えて描き、外接矩形を確かめた Pillow の画像を返す。"""
    from PIL import Image

    src = ICON_DIR / f"{name}.svg"
    sized = OUT_DIR / f"{name}.render.svg"
    sized.write_text(set_svg_size(src.read_text(encoding="utf-8"), w, h), encoding="utf-8")
    png = OUT_DIR / f"{name}-render.png"
    render(edge, sized, png, w, h, profile)
    with Image.open(png) as im:
        img = im.convert("RGBA")
    if img.size != (w, h):
        raise MakeIconError(f"{png.name} の大きさが {img.size} (期待は {(w, h)})")
    bbox = img.getchannel("A").getbbox()
    if not bbox_matches(bbox, expected_bbox):
        raise MakeIconError(
            f"{png.name} の不透明な画素の外接矩形が {bbox} (期待は {expected_bbox} の前後 {BBOX_TOLERANCE}px)"
        )
    print(f"描いた: {png.relative_to(ROOT)} 外接矩形 {bbox}")
    return img, png


def png_bytes(img, size: int) -> bytes:
    from PIL import Image

    small = img.resize((size, size), Image.Resampling.LANCZOS)
    buf = io.BytesIO()
    small.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def make(edge: Path) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    profile = OUT_DIR / "edge-profile"
    small_img, _ = render_checked(edge, "ai-cu-small", RENDER, RENDER, ICON_BBOX, profile)
    large_img, _ = render_checked(edge, "ai-cu", RENDER, RENDER, ICON_BBOX, profile)
    sw, sh = SOCIAL_SIZE
    _, social_png = render_checked(edge, "social-preview", sw, sh, (0, 0, sw, sh), profile)
    social_out = OUT_DIR / "social-preview.png"
    social_out.write_bytes(social_png.read_bytes())

    entries = []
    for sizes, img in ((SMALL_SIZES, small_img), (LARGE_SIZES, large_img)):
        for size in sizes:
            data = png_bytes(img, size)
            (OUT_DIR / f"ai-cu-{size}.png").write_bytes(data)
            entries.append((size, data))
    ico = build_ico(entries)
    ICO_PATH.write_bytes(ico)
    print(f"書いた: {ICO_PATH.relative_to(ROOT)} ({len(ico)} バイト、大きさ {[s for s, _ in entries]})")
    print(f"書いた: {social_out.relative_to(ROOT)} (GitHub の Settings の Social preview から上げる)")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--edge", help="msedge.exe のパス (既定の場所に無いとき)")
    args = ap.parse_args()
    edge = find_edge(args.edge)
    if edge is None:
        where = f"--edge の {args.edge}" if args.edge else "既定の場所 (Program Files (x86) と Program Files)"
        print(f"Edge (msedge.exe) が {where} に無い。--edge PATH で msedge.exe を指す", file=sys.stderr)
        return 1
    try:
        make(edge)
    except MakeIconError as e:
        print(f"失敗: {e}。ICO は書いていない", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
