"""Exercise the demo entry point without starting a container or Studio."""
import contextlib
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples/graph_studio"))
import demo


class ResumeTests(unittest.TestCase):
    def test_delivery_layout_checked_before_turn_and_ui(self):
        v1 = {"selected": {"selected.svg": ""}, "choice": {"response.json": ""},
              "painting": {"painting.ora": ""}}
        for files, missing in [(v1, None), ({"model": {"plate.stl": ""}}, "selected/selected.svg"),
                               ({k: v for k, v in v1.items() if k != "choice"}, "choice/response.json"),
                               ({**v1, "painting": {"other.ora": ""}}, "painting/painting.ora")]:
            with self.subTest(missing=missing):
                outputs = ["image", '{"task":"fresh"}', '{"workspace":"ws","work":{"image":"rev"}}',
                           json.dumps({"files": files}), '{"turn":"turn"}']
                stderr = io.StringIO()
                with patch.object(sys, "argv", ["demo", "bridge", "--resume", "delivery"]), \
                     patch.object(demo.subprocess, "check_output", side_effect=outputs) as command, \
                     patch.object(demo.subprocess, "Popen", side_effect=InterruptedError) as studio, \
                     contextlib.redirect_stderr(stderr):
                    with self.assertRaises(SystemExit if missing else InterruptedError):
                        demo.main()
                    read = command.call_args_list[3]
                    self.assertEqual(read.args[0][1:], ["graph", "--task", "fresh", "open"])
                    self.assertEqual(json.loads(read.kwargs["input"]), {"delivery": "delivery"})
                    if missing:
                        self.assertIn(missing, stderr.getvalue())
                        self.assertEqual(command.call_count, 4)
                        studio.assert_not_called()
                    else:
                        studio.assert_called_once()
