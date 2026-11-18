import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "headless"))
import kernel as k
import workspaces as w


class MappingTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.enterContext(patch.object(k, "ROOT", Path(tmp.name)))
        override = patch.object(k, "DB", Path(tmp.name) / "db")
        override.start()
        self.addCleanup(override.stop)
        self.a = SimpleNamespace(
            harness="test", session="same", key=None, workspace="a"
        )
        self.b = SimpleNamespace(
            harness="test", session=None, key="same", workspace="a"
        )
        self.c = SimpleNamespace(
            harness="other", session="same", key=None, workspace="b"
        )
        with k.connect() as db:
            for wid in ("a", "b"):
                k.put(db, "workspace", wid, dict(images=[], editable=[], tools={}))
            for a in (self.a, self.b, self.c):
                w.bind(db, a)
            w.bind(db, self.a)
        self.tasks = [w.resolve(a) for a in (self.a, self.b, self.c)]

    def test_mapping(self):
        self.assertEqual(len(set(self.tasks)), 3)
        self.assertEqual(self.tasks[0], w.resolve(self.a))
        with k.connect() as db:
            self.a.workspace = "b"
            with self.assertRaises(ValueError):
                w.bind(db, self.a)
            k.put(db, "instance", "active", dict(task=self.tasks[0], status="running"))
        with self.assertRaisesRegex(ValueError, "active"):
            w.resolve(self.a, True)
        with k.connect() as db:
            k.put(
                db,
                "instance",
                "active",
                dict(task=self.tasks[0], status="unloaded"),
                True,
            )
        self.assertNotEqual(self.tasks[0], w.resolve(self.a, True))

    def test_visibility(self):
        a, b, c = self.tasks
        with k.connect() as db:
            for n in range(53):
                k.put(
                    db,
                    "run",
                    str(n),
                    dict(
                        task=a,
                        spool="S",
                        status="failed" if n == 52 else "delivered",
                        outputs=[] if n == 51 else ["x"],
                        output="revision",
                    ),
                )
            self.assertEqual(w.upstream(db, b, "0"), "revision")
            for owner, source in ((c, "0"), (b, "51"), (b, "52")):
                with self.assertRaises(ValueError):
                    w.upstream(db, owner, source)
            self.assertFalse(w.deliveries(db, c, 0)["items"])
            first = w.deliveries(db, b, 0)
            self.assertEqual(len(first["items"]), 50)
            self.assertEqual(
                [x["id"] for x in w.deliveries(db, b, first["next"])["items"]], ["50"]
            )

    def test_live_binding_cannot_dispatch_after_rotation(self):
        key = w.identity(self.a)
        with w.binding_guard(key, task=self.tasks[0]):
            with self.assertRaisesRegex(ValueError, "busy"):
                w.resolve(self.a, True)
        current = w.resolve(self.a, True)
        with (
            self.assertRaisesRegex(ValueError, "reconnect"),
            w.binding_guard(key, task=self.tasks[0]),
        ):
            self.fail("stale dispatch")
        with w.binding_guard(key, task=current):
            pass
