"""Composition benchmark regressions, with no model or container dependency."""

from contextlib import closing
import copy
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "examples/composition"))
from unittest.mock import Mock, patch
from relay import reference, verify_cleanup
from catalog import SPOOLS, manifest
from evaluate import CASES
from live import expected_outputs

sys.path.insert(0, str(ROOT / "studio/tests"))
from fake_agent import Mcp
from scoring import assess_safely

IMAGES = {name: f"fixture-{n}@sha256:{n:064x}" for n, name in enumerate(SPOOLS)}
BINARY = ROOT / "target/debug/an"


class CompositionTests(unittest.TestCase):
    def test_cleanup_requires_positive_container_evidence(self):
        runtime = Mock()
        runtime.close.return_value = True
        report = {"instances": ["owned"], "instance_names": {"an-owned": "owned"}}
        with patch("relay.subprocess.check_output", side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                verify_cleanup(runtime, report)
        self.assertTrue(report["host_cleanup"])
        self.assertFalse(report["cleanup"])
        with patch("relay.subprocess.check_output", return_value=b"[]"):
            verify_cleanup(runtime, report)
        self.assertTrue(report["cleanup"])

    def test_legacy_report_observes_names_before_close(self):
        runtime = Mock(task="task")
        runtime.cli.return_value = {"instances": [{"name": "an-owned", "id": "owned"}]}
        report = {"instances": ["owned"]}
        with patch("relay.subprocess.check_output", return_value=b"[]"):
            verify_cleanup(runtime, report)
        self.assertTrue(report["cleanup"])
        self.assertEqual(report["instance_names"], {"an-owned": "owned"})
        runtime.cli.assert_called_once_with("state", "--task", "task")
        runtime.close.assert_called_once()

        for observed in (OSError("inventory unavailable"), {"instances": []}):
            runtime = Mock(task="task")
            if isinstance(observed, OSError):
                runtime.cli.side_effect = observed
            else:
                runtime.cli.return_value = observed
            report = {"instances": ["owned"]}
            with self.subTest(observed=observed), patch("relay.subprocess.check_output", return_value=b"[]"):
                with self.assertRaises((ValueError, OSError)):
                    verify_cleanup(runtime, report)
            self.assertFalse(report["cleanup"])
            runtime.close.assert_called_once()

    def test_cleanup_detects_names_and_labels_independently(self):
        runtime = Mock()
        runtime.close.return_value = True
        report = {"instances": ["owned"], "instance_names": {"an-owned": "owned"}}
        for name, labels in (("an-owned", {}), ("other", {"an.instance": "owned"})):
            inventory = [{"id": name, "configuration": {"labels": labels}}]
            with self.subTest(name=name), patch("relay.subprocess.check_output", return_value=json.dumps(inventory)):
                verify_cleanup(runtime, report)
                self.assertFalse(report["cleanup"])
                self.assertEqual(report["remaining_instances"], ["owned"])

    def test_cases_and_oracles(self):
        for case in CASES:
            with self.subTest(case=case["name"]):
                self.assertTrue(
                    all(assess_safely(reference(case, IMAGES), case, IMAGES).values())
                )
                self.assertEqual(
                    set(expected_outputs(case)),
                    {SPOOLS[n]["output"] for n in case["deliver"]},
                )
        parallel = expected_outputs(next(c for c in CASES if c["name"] == "parallel"))
        self.assertIn(b"Line 3", parallel["review.md"])
        self.assertIn(b"80,20", parallel["lines.svg"])
        self.assertIn(b'stroke="black"', parallel["lines.svg"])

    def test_right_names_can_still_be_wired_wrong(self):
        case = next(c for c in CASES if c["name"] == "flat-colour")
        plan = reference(case, IMAGES)
        bad = copy.deepcopy(plan)
        paint = next(n for n in bad["nodes"] if n["id"] == "n-painting")
        paint["inputs"]["lines"] = {"node": "n-sketch", "port": "sketch.svg"}
        result = assess_safely(bad, case, IMAGES)
        self.assertTrue(result["selection"])
        self.assertFalse(result["wiring"])
        delivery = next(n for n in plan["nodes"] if n["kind"] == "delivery")
        delivery["inputs"].pop("0")
        self.assertFalse(assess_safely(plan, case, IMAGES)["delivery"])

    @unittest.skipUnless(BINARY.exists(), "cargo build first")
    def test_catalog_and_paused_submission_via_real_mcp(self):
        case = next(c for c in CASES if c["name"] == "parallel")
        with tempfile.TemporaryDirectory() as tmp:
            env = {**os.environ, "AN_HOME": tmp, "AN_TURN": "", "AN_AGENT_TOKEN": ""}

            def cli(*args, body=None):
                return json.loads(
                    subprocess.check_output(
                        [BINARY, *args],
                        env=env,
                        text=True,
                        input=json.dumps(body or {}),
                    )
                )

            # Test-only admission snapshots avoid image builds. This test never resumes the graph.
            with closing(sqlite3.connect(Path(tmp) / "registry.sqlite3")) as db, db:
                db.execute("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
                for name, image in IMAGES.items():
                    db.execute(
                        "INSERT INTO objects VALUES('admission',?,?)",
                        (
                            image.split("@")[1],
                            json.dumps(
                                {
                                    "spec": "spool/v2",
                                    "manifest_digest": "fixture",
                                    "manifest": manifest(name, SPOOLS[name]),
                                }
                            ),
                        ),
                    )
                db.execute(
                    "INSERT INTO objects VALUES('task','case',?)",
                    (
                        json.dumps(
                            {
                                "work": dict.fromkeys(IMAGES.values(), "seed"),
                                "inputs": dict.fromkeys(case["inputs"], "fixture"),
                            }
                        ),
                    ),
                )
            env["AN_TURN"] = cli("permission", "begin", "--task", "case")["turn"]
            cli("graph", "--task", "case", "control", body={"action": "pause"})
            token = cli("permission", "delegate", "--task", "case")["agent_token"]
            mcp = Mcp(
                {
                    "command": str(BINARY),
                    "args": ["mcp", "--task", "case"],
                    "env": {"AN_HOME": tmp, "AN_AGENT_TOKEN": token, "AN_TURN": ""},
                }
            )
            try:
                catalog = mcp.tool("an_catalog", {})
                self.assertEqual(set(catalog["spools"]), set(IMAGES.values()))
                self.assertEqual(set(catalog["inputs"]), set(case["inputs"]))
                self.assertEqual(catalog["delivery_destination"], "case")
                graph = mcp.tool("an_graph_state", {})
                args = {
                    "base_revision": graph["revision"],
                    "nodes": reference(case, IMAGES)["nodes"],
                    "idempotency_key": "once",
                }
                receipt = mcp.tool("an_graph_submit", args)
                self.assertEqual(receipt["status"], "accepted")
                self.assertEqual(receipt, mcp.tool("an_graph_submit", args))
                self.assertTrue(
                    all(
                        s["status"] == "queued"
                        for s in mcp.tool("an_graph_state", {})["activity"].values()
                    )
                )
                self.assertEqual(cli("state", "--task", "case")["instances"], [])
                env["AN_TURN"] = cli("permission", "begin", "--task", "case")["turn"]
                with self.assertRaisesRegex(RuntimeError, "Active host turn required"):
                    mcp.tool("an_catalog", {})
            finally:
                mcp.close()
                mcp.proc.stdout.close()
            cli("graph", "--task", "case", "control", body={"action": "stop"})
            finish = cli("graph", "--task", "case", "finish")
            self.assertEqual(finish, {"closed": True, "cleanup_pending": []})
            self.assertEqual(cli("graph", "--task", "case", "finish"), finish)


if __name__ == "__main__":
    unittest.main()
