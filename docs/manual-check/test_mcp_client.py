"""docs/manual-check/mcp_client.py のテスト (D-055)。ai-cu も子プロセスも起動しない (CI で走る)。

使い方: python docs/manual-check/test_mcp_client.py
"""

import copy
import json
import os
import sys
import tempfile
import threading
import time
import unittest

sys.dont_write_bytecode = True  # __pycache__ を作らない (docs/ は build.rs の引き金)

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import mcp_client  # noqa: E402


def tools_reply(reply_id=2, image_data="A" * 400):
    return {
        "jsonrpc": "2.0",
        "id": reply_id,
        "result": {
            "tools": [
                {"name": "find", "annotations": {"readOnlyHint": True}},
                {"name": "click", "annotations": {"readOnlyHint": False}},
                {"name": "plain"},
                {"name": "pixel", "annotations": {"readOnlyHint": True}},
            ],
            "content": [{"type": "image", "data": image_data}],
        },
    }


class ClipTest(unittest.TestCase):
    def test_boundaries(self):
        for limit in (300, 4000):
            exact = "x" * limit
            self.assertEqual(mcp_client.clip(exact, limit), exact)
            over = "x" * (limit + 1)
            self.assertEqual(
                mcp_client.clip(over, limit),
                "x" * limit + f" …(全 {limit + 1} 字のうち {limit} 字)",
            )

    def test_short(self):
        self.assertEqual(mcp_client.clip("abc", 200), "abc")


class ToolsSummaryTest(unittest.TestCase):
    def test_read_only_names_in_order(self):
        self.assertEqual(
            mcp_client.tools_summary(tools_reply()),
            "tools: 4 個 / readOnlyHint: find, pixel",
        )

    def test_no_read_only(self):
        reply = {"id": 2, "result": {"tools": [{"name": "click"}, {"name": "a", "annotations": {}}]}}
        self.assertEqual(mcp_client.tools_summary(reply), "tools: 2 個 / readOnlyHint: (なし)")

    def test_not_tools_list(self):
        self.assertIsNone(mcp_client.tools_summary({"id": 1, "result": {"content": []}}))
        self.assertIsNone(mcp_client.tools_summary({"id": 1, "result": "x"}))
        self.assertIsNone(mcp_client.tools_summary({"id": 1, "result": {"tools": "x"}}))
        self.assertIsNone(
            mcp_client.tools_summary({"id": 1, "error": {"code": -32601, "message": "no"}})
        )


class NamesTest(unittest.TestCase):
    def test_reply_path_number(self):
        self.assertEqual(mcp_client.reply_path("d", 2), os.path.join("d", "reply-2.json"))

    def test_reply_path_string(self):
        self.assertEqual(
            mcp_client.reply_path("d", 'a/b\\c:d*e?f"g<h>i|j k\tl'),
            os.path.join("d", "reply-a_b_c_d_e_f_g_h_i_j_k_l.json"),
        )

    def test_run_dir_name(self):
        now = time.struct_time((2026, 11, 18, 9, 5, 7, 2, 322, 0))
        self.assertEqual(mcp_client.run_dir_name(now), "20261118-090507")

    def test_save_path(self):
        self.assertIsNone(mcp_client.save_path(None, {"id": 2}))
        self.assertIsNone(mcp_client.save_path("d", {"result": {}}))
        self.assertIsNone(mcp_client.save_path("d", {"id": None, "error": {}}))
        self.assertEqual(mcp_client.save_path("d", {"id": 3}), os.path.join("d", "reply-3.json"))


class MakeRunDirTest(unittest.TestCase):
    def test_second_gets_suffix(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = os.path.join(tmp, "replies")
            first = mcp_client.make_run_dir(base, "20261118-090507")
            second = mcp_client.make_run_dir(base, "20261118-090507")
            self.assertEqual(first, os.path.join(base, "20261118-090507"))
            self.assertEqual(second, os.path.join(base, "20261118-090507-2"))
            self.assertTrue(os.path.isdir(first))
            self.assertTrue(os.path.isdir(second))


class ParseLineTest(unittest.TestCase):
    def test_object(self):
        self.assertEqual(mcp_client.parse_line('{"id": 1}'), {"id": 1})

    def test_not_object(self):
        self.assertIsNone(mcp_client.parse_line("not json"))
        self.assertIsNone(mcp_client.parse_line("[1, 2]"))
        self.assertIsNone(mcp_client.parse_line("3"))


class ParseArgsTest(unittest.TestCase):
    def test_all_options(self):
        cmd, opts = mcp_client.parse_args(
            ["ai-cu.exe", "mcp", "--", "r.jsonl", "--timeout", "5", "--save-images", "img",
             "--save-replies", "rep"]
        )
        self.assertEqual(cmd, ["ai-cu.exe", "mcp"])
        self.assertEqual(opts["requests"], "r.jsonl")
        self.assertEqual(opts["timeout"], 5.0)
        self.assertEqual(opts["save_dir"], "img")
        self.assertEqual(opts["replies_dir"], "rep")

    def test_default_no_replies(self):
        _, opts = mcp_client.parse_args(["ai-cu.exe", "--", "r.jsonl"])
        self.assertIsNone(opts["replies_dir"])

    def test_no_separator(self):
        self.assertIsNone(mcp_client.parse_args(["ai-cu.exe", "r.jsonl"]))


class QueueTest(unittest.TestCase):
    def setUp(self):
        self.others = []
        self.bads = []

    def test_drain_until_none(self):
        responses = ['{"id": 7}', "garbage", '{"id": 8}', None, '{"id": 9}']
        mcp_client.drain(responses, threading.Condition(), self.others.append, self.bads.append)
        self.assertEqual(self.others, [{"id": 7}, {"id": 8}])
        self.assertEqual(self.bads, ["garbage"])

    def test_drain_empty_returns(self):
        start = time.monotonic()
        mcp_client.drain([], threading.Condition(), self.others.append, self.bads.append)
        self.assertLess(time.monotonic() - start, 0.5)
        self.assertEqual(self.others, [])

    def test_wait_reply_other_id(self):
        responses = ['{"id": 1, "result": {}}', '{"id": 2, "result": {"ok": true}}']
        reply, closed, timed_out = mcp_client.wait_reply(
            responses, threading.Condition(), 2, 1.0, self.others.append, self.bads.append
        )
        self.assertEqual(reply, {"id": 2, "result": {"ok": True}})
        self.assertFalse(closed)
        self.assertFalse(timed_out)
        self.assertEqual(self.others, [{"id": 1, "result": {}}])

    def test_wait_reply_skips_bad_line(self):
        responses = ["not json", '{"id": 2}']
        reply, _, _ = mcp_client.wait_reply(
            responses, threading.Condition(), 2, 1.0, self.others.append, self.bads.append
        )
        self.assertEqual(reply, {"id": 2})
        self.assertEqual(self.bads, ["not json"])


class ShowTest(unittest.TestCase):
    def test_order_and_full_save(self):
        reply = tools_reply()
        original_len = len(json.dumps(reply, ensure_ascii=False))
        lines = []
        with tempfile.TemporaryDirectory() as tmp:
            mcp_client.show(reply, "", 50, tmp, None, [0], out=lines.append)
            self.assertEqual(len(lines), 3)
            self.assertTrue(lines[0].startswith("--- tools: 4 個"))
            self.assertTrue(lines[1].startswith("<<< "))
            self.assertIn("…(全 ", lines[1])
            self.assertTrue(lines[2].startswith("(saved "))
            path = os.path.join(tmp, "reply-2.json")
            self.assertIn(path, lines[2])
            with open(path, encoding="utf-8") as f:
                saved = json.load(f)
        self.assertEqual(saved["result"]["content"][0]["data"], "<400 chars>")
        self.assertEqual(saved, reply)
        self.assertGreater(len(json.dumps(saved, ensure_ascii=False)), 50)
        self.assertGreater(original_len, len(json.dumps(saved, ensure_ascii=False)))

    def test_no_run_dir(self):
        # 書かないことは save_path(None, ...) が None を返すこと (NamesTest) に任せ、
        # ここでは表示が要約と本文の 2 行だけで (saved の行が無いことを確かめる。
        lines = []
        mcp_client.show(tools_reply(), "", 50, None, None, [0], out=lines.append)
        self.assertEqual(len(lines), 2)
        self.assertTrue(lines[0].startswith("--- tools: "))
        self.assertTrue(lines[1].startswith("<<< "))
        self.assertFalse(any(l.startswith("(saved") for l in lines))

    def test_null_id_shown_not_saved(self):
        lines = []
        reply = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "parse"}}
        with tempfile.TemporaryDirectory() as tmp:
            mcp_client.show(copy.deepcopy(reply), "(別の id の応答) ", 300, tmp, None, [0],
                            out=lines.append)
            self.assertEqual(os.listdir(tmp), [])
        self.assertEqual(len(lines), 1)
        self.assertTrue(lines[0].startswith("<<< (別の id の応答) "))


if __name__ == "__main__":
    unittest.main()
