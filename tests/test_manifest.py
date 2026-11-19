import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "headless"))
import manifest


def interface():
    return dict(schema="an.spool/v1", name="processor", summary="Process an input",
                inputs=[dict(name="source", description="Source data",
                             media_types=["application/ld+json"], required=True)],
                outputs=[])


class ManifestTest(unittest.TestCase):
    def test_nonempty_input(self):
        value = interface()
        self.assertEqual(manifest.check(json.dumps(value).encode()), value)

    def test_port_errors(self):
        for key, value in [
            ("required", "true"), ("name", "Upper"), ("description", " "),
            ("media_types", []), ("media_types", ["text/*"]),
            ("media_types", ["text/plain; charset=utf-8"]),
            ("media_types", ["text/plain", "TEXT/plain"]),
        ]:
            with self.subTest(key=key,value=value):
                data = interface()
                data["inputs"][0][key] = value
                with self.assertRaisesRegex(ValueError, key):
                    manifest.check(json.dumps(data).encode())

    def test_repeated_port(self):
        data = interface()
        data["inputs"] *= 2
        with self.assertRaisesRegex(ValueError, "duplicate"):
            manifest.check(json.dumps(data).encode())

    def test_yaml_limits(self):
        for raw in (b"a: &x []\nb: *x", b"!!map {}", b"<<: {}", b"a: []\na: []",
                    b"---\na\n---\nb", b"["*17+b"]"*17, b"- x\n"*4100,
                    b"\xff", b"a"*65537):
            with self.subTest(prefix=raw[:16]), self.assertRaises(ValueError):
                manifest.check(raw)
