"""Regression checks for the two-instruction fixture's acceptance criteria."""

import base64
import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "examples/composition"))
from catalog import SPOOLS
from live import expected_outputs
from relay import reference
from studio_evidence import held_publication, verify_delivery, verify_plan, verify_wakes
from studio_turns import CASES, allowed_flows, approve


class StudioAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.runtime = SimpleNamespace(
            workspace="workspace", images={name: name for name in SPOOLS}
        )
        first = reference(CASES["ink"], self.runtime.images)["nodes"]
        painting = next(
            n
            for n in reference(CASES["flat-colour"], self.runtime.images)["nodes"]
            if n["id"] == "n-painting"
        )
        delivery = {
            "id": "colour",
            "kind": "delivery",
            "outputs": [],
            "inputs": {"colour": {"node": painting["id"], "port": "painting.svg"}},
        }
        self.activity = {}
        for node in first + [painting, delivery]:
            active = {
                "id": "attempt-" + node["id"],
                "attempt": 1,
                "status": "done",
                "bindings": {},
                "outputs": {},
            }
            for name, ref in node["inputs"].items():
                if "node" in ref:
                    active["bindings"][name] = {"revision": ref["port"]}
            for port in node["outputs"]:
                active["outputs"][port] = {"revision": port}
            if node["kind"] == "delivery":
                active["delivery"] = node["id"]
            self.activity[node["id"]] = active
        added = [painting["id"], delivery["id"]]
        first_events = [
            {
                "kind": "submitted",
                "actor": "agent",
                "instruction_cursor": 1,
                "nodes": [n["id"] for n in first],
            }
        ]
        self.first = {
            "revision": 1,
            "nodes": {n["id"]: n for n in first},
            "activity": {n["id"]: copy.deepcopy(self.activity[n["id"]]) for n in first},
            "events": first_events,
        }
        self.second = {
            "revision": 2,
            "nodes": {n["id"]: n for n in first + [painting, delivery]},
            "activity": self.activity,
            "events": first_events
            + [
                {
                    "kind": "submitted",
                    "actor": "agent",
                    "instruction_cursor": 8,
                    "nodes": added,
                }
            ],
        }

    def open(self, action, delivery):
        self.assertEqual(action, "open")
        outputs = expected_outputs(CASES["flat-colour"])
        name = "lines.svg" if delivery == "results" else "painting.svg"
        return {"files": {"artifact": {name: base64.b64encode(outputs[name]).decode()}}}

    def test_additive_reuse_check_rejects_reexecution_and_wrong_revision(self):
        verify_plan(self.runtime, CASES["ink"], self.first, {}, 1)
        slot = next(iter(self.first["nodes"]["results"]["inputs"]))
        before = self.first["activity"]["results"]
        before.pop("delivery")
        before.update(status="permission_required", requests={slot: "hold"})
        request = {
            "id": "hold",
            "operation": "publish",
            "scope": ["line-art", slot, "workspace"],
            "details": [before["id"], "lines.svg"],
            "revision": "lines.svg",
        }
        self.assertEqual(
            held_publication(self.runtime, self.first, [request], [])["request"], "hold"
        )
        with self.assertRaisesRegex(ValueError, "before any delivery"):
            held_publication(self.runtime, self.first, [request], [{"id": "early"}])
        with self.assertRaisesRegex(ValueError, "pending decision"):
            held_publication(
                self.runtime, self.first, [{**request, "revision": "unrelated"}], []
            )

        def verify():
            added = verify_plan(
                self.runtime, CASES["flat-colour"], self.second, self.first, 8
            )
            verify_delivery(self.open, CASES["flat-colour"], self.second)
            return added

        self.assertEqual(verify(), ["colour", "n-painting"])
        self.activity["n-line-art"]["attempt"] = 2
        with self.assertRaisesRegex(ValueError, "executed again"):
            verify()
        self.activity["n-line-art"]["attempt"] = 1
        self.activity["n-painting"]["bindings"]["lines"]["revision"] = "unrelated"
        with self.assertRaisesRegex(ValueError, "upstream revision"):
            verify()

    def test_extra_scope_is_never_approved(self):
        expected = allowed_flows(self.runtime, "flat-colour")
        rows = [
            {
                "operation": op,
                "origin": origin,
                "source": {"producer_port": port},
                "destination": dest,
                "producer_argv": list(argv),
            }
            for op, origin, port, dest, argv in expected
        ]
        request = Mock()
        card = {"id": "new", "turn_scope": True, "routes": rows}
        approve(request, card, expected)
        request.assert_called_once_with(
            "intent-decide", id="new", decisions=["allow", "allow"]
        )
        request.reset_mock()
        rows.append({**rows[0], "destination": "elsewhere"})
        with self.assertRaisesRegex(ValueError, "scope"):
            approve(request, card, expected)
        request.assert_not_called()

    def test_decision_wake_and_submission_must_share_the_original_thread(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            state = directory / "registry/agents/a"
            state.mkdir(parents=True)
            (state / "thread").write_text("first")

            def trace(cursor, thread, tools):
                events = [{"type": "thread.started", "thread_id": thread}]
                events += [
                    {
                        "type": "item.completed",
                        "item": {
                            "type": "mcp_tool_call",
                            "server": "an",
                            "status": "completed",
                            "tool": tool,
                        },
                    }
                    for tool in tools
                ]
                events.append({"type": "turn.completed"})
                (state / f"wake-{cursor}.jsonl").write_text(
                    "\n".join(json.dumps(e) for e in events)
                )

            records = [{"cursor": 1}, {"cursor": 9}]
            trace(1, "first", ["an_graph_state"])
            trace(4, "first", ["an_graph_candidate"])
            trace(9, "first", ["an_graph_state", "an_graph_candidate"])
            self.assertEqual(verify_wakes(directory, "a", records), "first")
            trace(9, "another", ["an_graph_state", "an_graph_candidate"])
            with self.assertRaisesRegex(ValueError, "original thread"):
                verify_wakes(directory, "a", records)


if __name__ == "__main__":
    unittest.main()
