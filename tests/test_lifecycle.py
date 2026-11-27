"""Failure paths that must not require damaging the host's Apple service."""

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / "reference"))
sandbox = tempfile.TemporaryDirectory()
os.environ["AN_HOME"] = sandbox.name
import kernel
import instances


class LifecycleTest(unittest.TestCase):
    def setUp(self):
        root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.enterContext(patch.object(kernel, "ROOT", root))
        self.enterContext(patch.object(kernel, "DB", root / "registry.sqlite3"))
        with kernel.connect() as db:
            kernel.put(db, "image", "test", "test", True)
            self.task = kernel.task_new(db, ["test"], [], {})
            self.i = dict(
                task=self.task,
                spool="test",
                platform={"os": "linux", "architecture": "arm64"},
                id=kernel.uid(),
                name="test",
                intent="load",
                status="loading",
                active=None,
            )
            instances.save(db, "instance", self.i)

    def state(self):
        return instances.call(self.task, "state", {})["instances"][0]

    def test_interrupted_load_missing_or_adopted(self):
        with (
            patch.object(instances, "observe", return_value="unknown"),
            patch.object(instances, "apple") as cli,
        ):
            self.assertEqual(self.state()["status"], "unknown")
            cli.assert_not_called()
        with (
            patch.object(instances.admission, "require", return_value={"platform": self.i["platform"]}),
            patch.object(instances, "observe", return_value="running"),
            patch.object(instances, "apple") as cli,
        ):
            self.assertEqual(
                instances.call(self.task, "load", {"spool": "test"})["id"], self.i["id"]
            )
            cli.assert_not_called()

    def test_worker_launch_failure_releases_slot(self):
        self.i["status"] = "running"
        with (
            kernel.connect() as db,
            patch("subprocess.Popen", side_effect=OSError("spawn failed")),
        ):
            run = instances.submit(
                db, self.i, {"argv": ["true"], "input": None, "outputs": []}
            )
            self.assertEqual(kernel.get(db, "run", run)["status"], "failed")
            self.assertIsNone(kernel.get(db, "instance", self.i["id"])["active"])

        permissions = instances.permissions
        with kernel.connect() as db:
            turn = permissions.control(db, self.task, "begin", None, None)["turn"]
        with patch.dict(os.environ, AN_TURN=turn):
            args = {"run": run}
            pending = instances.call(self.task, "log", args)
            self.assertEqual(pending["status"], "permission_required")
            with kernel.connect() as db:
                permissions.control(db, self.task, "decide", pending["request_id"], "once")
            args["request_id"] = pending["request_id"]
            self.assertIn("spawn failed", instances.call(self.task, "log", args))

    def test_handoff_retry_after_worker_launch_failure(self):
        self.i["status"] = "running"
        with kernel.connect() as db:
            turn = instances.permissions.control(db, self.task, "begin", None, None)["turn"]
        with patch.dict(os.environ, AN_TURN=turn), kernel.connect() as db:
            key = "handoff-test"
            ticket = dict(task=self.task, recipient="test", revision=kernel.revision(db, {}),
                          stamp=instances.permissions.stamp(db, self.task))
            kernel.put(db, "handoff", key, ticket)
            db.commit()
            args = {"argv":["true"], "input":key, "outputs":[]}
            with patch("subprocess.Popen", side_effect=OSError("spawn failed")):
                first = instances.submit(db, self.i, args)
            self.assertEqual(kernel.get(db, "run", first)["status"], "failed")
            self.assertEqual(kernel.get(db, "handoff", key), ticket)
            with patch("subprocess.Popen"):
                second = instances.submit(db, self.i, args)
            self.assertEqual(kernel.get(db, "handoff", key)["used"], second)

    def test_legacy_writer_blocks_new_reservation(self):
        self.i["status"] = "running"
        with kernel.connect() as db:
            task = kernel.get(db, "task", self.task)
            task["editable"] = ["test"]
            kernel.put(db, "task", self.task, task, True)
            kernel.put(
                db,
                "run",
                kernel.uid(),
                dict(task=self.task, spool="test", status="unknown"),
            )
            db.commit()
            with self.assertRaisesRegex(ValueError, "legacy"):
                instances.submit(
                    db, self.i, {"argv": ["true"], "input": None, "outputs": []}
                )

    def test_image_without_keepalive_is_not_registered(self):
        import subprocess

        image = "minimal@sha256:" + "2" * 64
        config = {"name": image, "descriptor": {"digest": image.split("@")[1]}}
        with (
            kernel.connect() as db,
            patch.object(kernel, "inspect_image", return_value=config),
            patch.object(
                instances.admission,
                "variant",
                return_value={"platform": {}, "variant": "test"},
            ),
            patch.object(
                subprocess,
                "check_output",
                side_effect=[subprocess.CalledProcessError(1, "run"), b"[]"],
            ),
        ):
            with self.assertRaisesRegex(ValueError, "/bin/sleep infinity"):
                kernel.register(db, image)
            with self.assertRaises(ValueError):
                kernel.get(db, "image", image)
