import base64
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from an_paint.protocol import ProtocolError, decode_layer_png, layers_to_replace, render_body


class RenderBodyTest(unittest.TestCase):
    def test_carries_revision_backend_sentence_png_and_selection(self):
        png = b"\x89PNG\r\n\x1a\n"
        body = render_body("untitled", 3, "code", "红色轮子，黄铜握把", png, None)
        self.assertEqual(body["document"], "untitled")
        self.assertEqual(body["revision"], 3)
        self.assertEqual(body["backend"], "code")
        self.assertEqual(body["description"], "红色轮子，黄铜握把")
        self.assertEqual(base64.b64decode(body["png_base64"]), png)
        self.assertIsNone(body["selection"])

        selected = render_body("doc", 1, "dsh", "蓝色轮子", png, {"x": 2, "y": 4, "w": 8, "h": 8})
        self.assertEqual(selected["selection"], {"x": 2, "y": 4, "w": 8, "h": 8})


class ReplacePlanTest(unittest.TestCase):
    def test_deletes_only_missing_an_layers(self):
        delete, replace = layers_to_replace(["an/轮子", "an/握把", "ink"], ["an/轮子"])
        self.assertEqual(delete, ["an/握把"])
        self.assertEqual(replace, ["an/轮子"])
        self.assertNotIn("ink", delete + replace)

    def test_a_new_an_layer_is_replaced_and_not_deleted(self):
        delete, replace = layers_to_replace(["ink"], ["an/轮子"])
        self.assertEqual(delete, [])
        self.assertEqual(replace, ["an/轮子"])

    def test_rejects_a_name_outside_an(self):
        with self.assertRaises(ProtocolError):
            layers_to_replace(["ink"], ["轮子"])


class DecodePngTest(unittest.TestCase):
    def test_rejects_bad_base64_and_non_png(self):
        with self.assertRaises(ProtocolError):
            decode_layer_png("!!!!")
        with self.assertRaises(ProtocolError):
            decode_layer_png(base64.b64encode(b"hello").decode("ascii"))

    def test_returns_png_bytes(self):
        raw = b"\x89PNG\r\n\x1a\nrest"
        self.assertEqual(decode_layer_png(base64.b64encode(raw).decode("ascii")), raw)


if __name__ == "__main__":
    unittest.main()
