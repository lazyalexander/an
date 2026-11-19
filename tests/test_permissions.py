import os
import sys
import sqlite3
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "headless"))
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
