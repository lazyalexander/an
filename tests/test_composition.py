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
from relay import reference
from catalog import SPOOLS, manifest
from evaluate import CASES
from live import expected_outputs

sys.path.insert(0, str(ROOT / "studio/tests"))
from fake_agent import Mcp
from scoring import assess_safely

IMAGES = {name: f"fixture-{n}@sha256:{n:064x}" for n, name in enumerate(SPOOLS)}
BINARY = ROOT / "target/debug/an"


class CompositionTests(unittest.TestCase):
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
