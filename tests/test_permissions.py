import os
import sys
import sqlite3
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / "reference"))
import permissions as p


class PermissionTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.execute("CREATE TABLE objects(kind,id,body, PRIMARY KEY(kind,id))")
        p.k.put(self.db, "task", "t", {})
        self.db.commit()
        self.turn = self.control("begin")["turn"]
        self.enterContext(patch.dict(os.environ, AN_TURN=self.turn))

    def control(self, action, req=None, decision=None):
        with self.db:
            return p.control(self.db, "t", action, req, decision)

    def gate(self, req=None, detail="file"):
        with self.db:
            return p.gate(self.db, "t", "publish", ["spool"], detail, req)

    def test_once_bound_to_details(self):
        req = self.gate()["request_id"]
        self.control("decide", req, "once")
        with self.assertRaises(ValueError):
            self.gate(req, "changed")
        self.assertIsNone(self.gate(req))
        with self.assertRaises(ValueError):
            self.gate(req)

    def test_rotation_and_denial(self):
        req = self.gate()["request_id"]
        self.control("decide", req, "deny")
        with self.assertRaises(ValueError):
            self.gate(req)
        req = self.gate()["request_id"]
        self.control("decide", req, "turn")
        self.assertIsNone(self.gate(detail="new version"))
        self.control("begin")
        with self.assertRaises(ValueError):
            self.gate()

    def test_host_decisions_and_workspace_ceiling(self):
        p.k.put(self.db, "workspace", "w", {"images": ["a","b"]})
        p.k.put(self.db, "task", "t", {"workspace":"w"}, True)
        flow = ["handoff","a","b"]
        p.policy(self.db, "w", [flow])
        self.db.commit()
        def check():
            with self.db:
                return p.gate(self.db,"t","handoff",flow,{},flow=flow)
        self.assertEqual(check()["status"], "permission_required")
        for decision in ("deny","unavailable","allow"):
            with self.db:
                p.set_host(self.db,"t",{"default":"ask","effects":[dict(operation="handoff",scope=flow,details={},decision=decision)]})
            if decision == "allow":
                self.assertIsNone(check())
            else:
                with self.assertRaisesRegex(ValueError, decision):
                    check()
        with self.db:
            p.policy(self.db,"w",[])
        with self.assertRaisesRegex(ValueError, "workspace"):
            check()

    def test_policy_rotation_invalidates_decisions_and_turn_grants(self):
        first = self.gate()["request_id"]
        self.control("decide", first, "turn")
        self.assertIsNone(self.gate())
        with self.db:
            p.set_host(self.db, "t", {"default":"ask","effects":[]})
        second = self.gate()["request_id"]
        with self.assertRaisesRegex(ValueError, "changed"):
            self.gate(first)
        with self.db:
            p.set_host(self.db, "t", {"default":"ask","effects":[]})
        self.assertEqual(self.control("pending"), [])
        with self.assertRaisesRegex(ValueError, "changed"):
            self.control("decide", second, "once")

    def test_generic_host_permission_does_not_approve_transfer(self):
        with self.db:
            p.set_host(self.db, "t", {"default":"ask","effects":[
                dict(operation="mcp",scope=["an_run"],details={},decision="allow")
            ]})
        self.assertEqual(self.gate()["status"], "permission_required")

    def test_effect_preserves_policy_and_rejects_stale_requests(self):
        denied = dict(operation="log", scope=["private"], details={}, decision="deny")
        ask = dict(operation="publish", scope=["spool"], details="file", decision="ask")
        with self.db:
            p.set_host(self.db, "t", {"default":"unavailable", "effects":[denied, ask]})
        first = self.gate()["request_id"]
        stale = self.gate()["request_id"]
        self.control("effect", first, "allow")
        self.assertIsNone(self.gate())
        with self.assertRaisesRegex(ValueError, "changed"):
            self.control("effect", stale, "allow")
        with self.db, self.assertRaisesRegex(ValueError, "deny"):
            p.gate(self.db, "t", "log", ["private"], {})
        with self.assertRaisesRegex(ValueError, "unavailable"):
            self.gate(detail="other")

    def test_null_policy_rejected_and_empty_array_closes_flows(self):
        p.k.put(self.db, "workspace", "w", {"images":["a","b"]})
        saved = p.policy(self.db, "w", [["handoff","a","b"]])
        with self.assertRaises(ValueError):
            p.policy(self.db, "w", None)
        self.assertEqual(p.policy(self.db, "w"), saved)
        self.assertEqual(p.policy(self.db, "w", [])["flows"], [])
