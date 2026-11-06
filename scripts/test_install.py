"""scripts/install.py の配置の手順のテスト。一時ディレクトリだけを使い、~/.cargo/bin には触れない。

使い方: python scripts/test_install.py
"""

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import install  # noqa: E402

COMMIT12 = "0123456789ab"


class InstallTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.dest_dir = self.dir / "bin"
        self.src = self.dir / "ai-cu.exe"

    def tearDown(self):
        self.tmp.cleanup()

    def write_src(self, data: bytes):
        self.src.write_bytes(data)

    def test_first_install_copies_to_dest(self):
        self.write_src(b"v1")
        dest, placed = install.install(self.src, self.dest_dir)
        self.assertTrue(placed)
        self.assertEqual(dest, self.dest_dir / "ai-cu.exe")
        self.assertEqual(dest.read_bytes(), b"v1")
        self.assertEqual(sorted(p.name for p in self.dest_dir.iterdir()), ["ai-cu.exe"])

    def test_new_version_renames_old_instead_of_overwriting(self):
        self.write_src(b"v1")
        install.install(self.src, self.dest_dir)
        self.write_src(b"v2")
        dest, placed = install.install(self.src, self.dest_dir)
        self.assertTrue(placed)
        self.assertEqual(dest.read_bytes(), b"v2")
        olds = list(self.dest_dir.glob("ai-cu.old-*.exe"))
        self.assertEqual(len(olds), 1)
        self.assertEqual(olds[0].read_bytes(), b"v1")
        self.assertFalse((self.dest_dir / "ai-cu.exe.new").exists())

    def test_same_version_keeps_file_and_leaves_no_old(self):
        self.write_src(b"v1")
        install.install(self.src, self.dest_dir)
        dest, placed = install.install(self.src, self.dest_dir)
        self.assertFalse(placed)
        self.assertEqual(dest, self.dest_dir / "ai-cu.exe")
        self.assertEqual(list(self.dest_dir.glob("ai-cu.old-*.exe")), [])
        self.assertFalse((self.dest_dir / "ai-cu.exe.new").exists())

    def test_remove_old_deletes_renamed_versions_only(self):
        self.dest_dir.mkdir()
        (self.dest_dir / "ai-cu.old-1.exe").write_bytes(b"x")
        (self.dest_dir / "ai-cu.exe").write_bytes(b"cur")
        (self.dest_dir / "other.exe").write_bytes(b"o")
        self.assertEqual(install.remove_old(self.dest_dir), [])
        self.assertEqual(
            sorted(p.name for p in self.dest_dir.iterdir()), ["ai-cu.exe", "other.exe"]
        )

    def test_missing_source_is_an_error(self):
        with self.assertRaises(SystemExit):
            install.install(self.src, self.dest_dir)


class StampTest(unittest.TestCase):
    """版の印の文法 (src/build_info.rs の文法を実行時に読むのは install.py だけ。D-069)"""

    def test_parse_version_reads_each_form(self):
        self.assertEqual(
            install.parse_version(f"ai-cu 0.1.0+g{COMMIT12}\n"),
            install.Stamp(f"0.1.0+g{COMMIT12}", COMMIT12, False),
        )
        self.assertEqual(
            install.parse_version(f"ai-cu 0.1.0+g{COMMIT12}.dirty"),
            install.Stamp(f"0.1.0+g{COMMIT12}.dirty", COMMIT12, True),
        )
        self.assertEqual(
            install.parse_version("ai-cu 0.1.0+unknown"),
            install.Stamp("0.1.0+unknown", None, False),
        )
        # pre-release は通す
        rc = install.parse_version(f"ai-cu 0.2.0-rc1+g{COMMIT12}")
        self.assertIsNotNone(rc)
        self.assertEqual(rc.commit12, COMMIT12)
        self.assertEqual(rc.version, f"0.2.0-rc1+g{COMMIT12}")
        # 形が違うものは None
        for bad in (
            "ai-cu 0.1.0",  # 印が無い (この変更より前の release)
            "ai-cu 0.1.0+G0123456789AB",  # 大文字
            "ai-cu 0.1.0+g0123456789a",  # 11 桁
            "ai-cu 0.1.0+g0123456789abc",  # 13 桁
            "",
        ):
            with self.subTest(text=bad):
                self.assertIsNone(install.parse_version(bad))

    def test_release_problem_refuses_unknown_dirty_and_mismatch(self):
        head = COMMIT12 + "0" * 28
        text, nxt = install.release_problem(None, head)
        self.assertIn("印が無い", text)
        self.assertEqual(nxt, install.NEXT_REBUILD)

        text, nxt = install.release_problem(install.Stamp("0.1.0+unknown", None, False), head)
        self.assertIn("unknown", text)
        self.assertEqual(nxt, install.NEXT_REBUILD_IN_REPO)

        text, nxt = install.release_problem(
            install.Stamp(f"0.1.0+g{COMMIT12}.dirty", COMMIT12, True), head
        )
        self.assertIn("dirty", text)
        self.assertEqual(nxt, install.NEXT_REBUILD)

        text, nxt = install.release_problem(
            install.Stamp(f"0.1.0+g{COMMIT12}", COMMIT12, False), "f" * 40
        )
        self.assertIn("HEAD", text)
        self.assertEqual(nxt, install.NEXT_REBUILD)

        self.assertIsNone(
            install.release_problem(install.Stamp(f"0.1.0+g{COMMIT12}", COMMIT12, False), head)
        )

    def test_read_stamp_separates_not_running_from_no_stamp(self):
        """動かない (起動できない) は RuntimeError、動くが印が無いのは None (断る文を分けるため)"""
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            with self.assertRaises(RuntimeError):
                install.read_stamp(d / "no-such.exe")
            not_exe = d / "ai-cu.exe"
            not_exe.write_bytes(b"not a program")
            with self.assertRaises(RuntimeError):
                install.read_stamp(not_exe)
        # python --version は終了コード 0 で "Python 3.x.y" を出す: 動くが印の形ではない
        self.assertIsNone(install.read_stamp(Path(sys.executable)))


class GitTest(unittest.TestCase):
    """git の読み取り。一時ディレクトリにリポジトリを作る (このリポジトリの木は読まない)"""

    def setUp(self):
        if shutil.which("git") is None:
            raise unittest.SkipTest("git が PATH に無い")
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_tree_changes_sees_untracked_and_modified_files(self):
        d = self.dir
        install.git(d, "init", "-q")
        (d / "a.txt").write_text("a", encoding="utf-8")
        install.git(d, "add", "a.txt")
        install.git(
            d,
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "init",
        )
        self.assertEqual(install.tree_changes(d), [])
        head = install.head_commit(d)
        self.assertEqual(len(head), 40, head)
        self.assertTrue(all(c in "0123456789abcdef" for c in head), head)

        (d / "b.txt").write_text("b", encoding="utf-8")
        self.assertEqual(install.tree_changes(d), ["?? b.txt"])

        (d / "a.txt").write_text("aa", encoding="utf-8")
        self.assertIn(" M a.txt", install.tree_changes(d))

    def test_git_failure_raises(self):
        with self.assertRaises(RuntimeError):
            install.head_commit(self.dir)


class RecordTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_record_appends_json_lines(self):
        log = self.dir / "no" / "such" / "install-log.jsonl"
        install.record(log, {"version": "0.1.0+g" + COMMIT12, "dest": "C:\\配置先\\ai-cu.exe"})
        install.record(log, {"version": "0.1.0+g" + COMMIT12, "dest": "D:\\二つ目\\ai-cu.exe"})
        text = log.read_text(encoding="utf-8")
        lines = text.splitlines()
        self.assertEqual(len(lines), 2)
        entries = [json.loads(line) for line in lines]
        self.assertEqual(entries[0]["dest"], "C:\\配置先\\ai-cu.exe")
        self.assertEqual(entries[1]["dest"], "D:\\二つ目\\ai-cu.exe")
        # ensure_ascii=False なので日本語がそのまま書かれる
        self.assertIn("配置先", text)
        self.assertNotIn("\\u914d", text)


if __name__ == "__main__":
    unittest.main()
