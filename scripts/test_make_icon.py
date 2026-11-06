"""scripts/make_icon.py のテスト (D-127)。Edge も Pillow も使わない (CI で走る)。

使い方: python scripts/test_make_icon.py
追跡している assets/icon/ai-cu.ico の大きさの一覧と中身の形もここで確かめる。
"""

import struct
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # __pycache__ を作らない (scripts/ は build.rs の引き金)

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import make_icon  # noqa: E402

PNG_SIG = b"\x89PNG\r\n\x1a\n"


def fake_png(w: int, h: int, extra: bytes = b"") -> bytes:
    """IHDR まである PNG の頭 (中身は検査しない ICO の組み立て用)。"""
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return PNG_SIG + struct.pack(">I", len(ihdr)) + b"IHDR" + ihdr + b"\0\0\0\0" + extra


def raw_entries(data: bytes) -> list[tuple]:
    """parse_ico を通さずに ICONDIRENTRY を読む。"""
    _, _, count = struct.unpack_from("<HHH", data, 0)
    return [struct.unpack_from("<BBBBHHII", data, 6 + 16 * i) for i in range(count)]


class ImportTest(unittest.TestCase):
    def test_import_does_not_load_pillow(self):
        loaded = [m for m in sys.modules if m == "PIL" or m.startswith("PIL.")]
        self.assertEqual(loaded, [], "make_icon の読み込みで Pillow を import した (CI は Pillow を入れない)")

    def test_sizes_split_small_and_large(self):
        self.assertEqual(make_icon.SMALL_SIZES, (16, 20, 24, 32))
        self.assertEqual(make_icon.LARGE_SIZES, (48, 64, 128, 256))


class SetSvgSizeTest(unittest.TestCase):
    def test_changes_only_root_width_and_height(self):
        svg = (
            '<?xml version="1.0"?>\n<!-- <svg width="1"> -->\n'
            '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">'
            '<rect x="8" width="240" height="240"/><path stroke-width="20"/>'
            '<svg width="10" height="10"/></svg>'
        )
        out = make_icon.set_svg_size(svg, 1024, 512)
        self.assertIn('width="1024" height="512" viewBox="0 0 256 256"', out)
        self.assertIn('<rect x="8" width="240" height="240"/>', out)
        self.assertIn('stroke-width="20"', out)
        self.assertIn('<svg width="10" height="10"/>', out)
        self.assertEqual(out.replace("1024", "256").replace('height="512"', 'height="256"'), svg)

    def test_svg_in_comment_before_root_is_not_the_root(self):
        svg = '<!-- <svg width="1" height="1"> --><svg width="256" height="256"></svg>'
        self.assertEqual(
            make_icon.set_svg_size(svg, 2, 3),
            '<!-- <svg width="1" height="1"> --><svg width="2" height="3"></svg>',
        )

    def test_root_without_size_raises(self):
        with self.assertRaises(ValueError):
            make_icon.set_svg_size('<svg viewBox="0 0 256 256"><rect width="1" height="1"/></svg>', 1, 1)
        with self.assertRaises(ValueError):
            make_icon.set_svg_size('<svg width="256" viewBox="0 0 256 256"></svg>', 1, 1)
        with self.assertRaises(ValueError):
            make_icon.set_svg_size("<html></html>", 1, 1)

    def test_stroke_width_is_not_root_width(self):
        with self.assertRaises(ValueError):
            make_icon.set_svg_size('<svg stroke-width="2" height="256"></svg>', 1, 1)

    def test_tracked_svgs_have_root_size(self):
        for name, size in (("ai-cu", (256, 256)), ("ai-cu-small", (256, 256)), ("social-preview", (1280, 640))):
            text = (ROOT / "assets" / "icon" / f"{name}.svg").read_text(encoding="utf-8")
            out = make_icon.set_svg_size(text, 7, 9)
            self.assertNotEqual(out, text, name)
            self.assertIn(f'width="{size[0]}" height="{size[1]}"', text, name)


class IcoTest(unittest.TestCase):
    def test_round_trip_keeps_sizes_and_bytes(self):
        sizes = list(make_icon.SMALL_SIZES + make_icon.LARGE_SIZES)
        entries = [(s, fake_png(s, s, bytes([s % 251]) * s)) for s in sizes]
        back = make_icon.parse_ico(make_icon.build_ico(entries))
        self.assertEqual([(w, h) for w, h, _ in back], [(s, s) for s in sizes])
        self.assertEqual([d for _, _, d in back], [d for _, d in entries])

    def test_256_is_written_as_zero_in_raw_bytes(self):
        data = make_icon.build_ico([(256, fake_png(256, 256))])
        self.assertEqual(data[6:8], b"\x00\x00")  # ICONDIRENTRY の幅と高さ (1 バイトずつ)

    def test_header_and_entry_fields(self):
        data = make_icon.build_ico([(16, fake_png(16, 16)), (48, fake_png(48, 48))])
        self.assertEqual(struct.unpack_from("<HHH", data, 0), (0, 1, 2))
        for e, size in zip(raw_entries(data), (16, 48)):
            w, h, colors, reserved, planes, bits, _len, _off = e
            self.assertEqual((w, h, colors, reserved, planes, bits), (size, size, 0, 0, 1, 32))

    def test_offsets_do_not_overlap_and_fill_to_end(self):
        entries = [(s, fake_png(s, s, b"x" * (s * 3))) for s in (16, 32, 256)]
        data = make_icon.build_ico(entries)
        pos = 6 + 16 * len(entries)
        for (_, _, _, _, _, _, length, offset), (_, png) in zip(raw_entries(data), entries):
            self.assertEqual(offset, pos)
            self.assertEqual(length, len(png))
            self.assertEqual(data[offset : offset + length], png)
            pos += length
        self.assertEqual(pos, len(data))

    def test_build_rejects_bad_sizes_and_empty(self):
        for bad in (0, 257):
            with self.assertRaises(ValueError):
                make_icon.build_ico([(bad, fake_png(1, 1))])
        with self.assertRaises(ValueError):
            make_icon.build_ico([])

    def test_parse_rejects_wrong_header_and_truncation(self):
        good = make_icon.build_ico([(16, fake_png(16, 16))])
        with self.assertRaises(ValueError):
            make_icon.parse_ico(b"\0\0\2\0\1\0" + good[6:])  # 種類 2 はカーソル
        with self.assertRaises(ValueError):
            make_icon.parse_ico(good[:-1])
        with self.assertRaises(ValueError):
            make_icon.parse_ico(good[:10])


class BboxTest(unittest.TestCase):
    def test_bbox_tolerance(self):
        exp = make_icon.ICON_BBOX
        self.assertTrue(make_icon.bbox_matches((30, 34, 994, 990), exp))
        self.assertFalse(make_icon.bbox_matches((29, 32, 992, 992), exp))
        self.assertFalse(make_icon.bbox_matches((8, 8, 248, 248), exp))  # width/height のまま左上に描いた形
        self.assertFalse(make_icon.bbox_matches(None, exp))


class TrackedIcoTest(unittest.TestCase):
    def test_tracked_ico_has_expected_sizes_and_pngs(self):
        data = (ROOT / "assets" / "icon" / "ai-cu.ico").read_bytes()
        items = make_icon.parse_ico(data)
        expected = list(make_icon.SMALL_SIZES + make_icon.LARGE_SIZES)
        self.assertEqual([w for w, _, _ in items], expected)
        for w, h, png in items:
            self.assertEqual(w, h)
            self.assertEqual(png[:8], PNG_SIG, f"{w} の項目が PNG でない")
            self.assertEqual(png[12:16], b"IHDR")
            pw, ph = struct.unpack(">II", png[16:24])
            self.assertEqual((pw, ph), (w, h), f"{w} の項目の PNG の大きさ")


if __name__ == "__main__":
    unittest.main()
