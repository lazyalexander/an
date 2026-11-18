import io
import sys
import tarfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "headless"))
import admission


def layout(skill=b"Run main", code=b"x", link=None):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, content in [
            ("spool", None),
            ("spool/code", None),
            ("spool/SKILL.md", skill),
            ("spool/code/main", code),
        ]:
            if content is False:
                continue
            item = tarfile.TarInfo(name)
            if link == name:
                item.type = tarfile.SYMTYPE
                item.linkname = "/etc"
            elif content is None:
                item.type = tarfile.DIRTYPE
            else:
                item.size = len(content)
            tar.addfile(item, io.BytesIO(content) if item.isfile() else None)
    buf.seek(0)
    return admission.inspect_layout(buf)


class LayoutTest(unittest.TestCase):
    def test_minimum(self):
        self.assertEqual(layout(), "Run main")

    def test_layout_budget(self):
        from unittest.mock import patch

        for limit in ("MAX_SPOOL_ENTRIES", "MAX_SPOOL_PATH_BYTES"):
            with patch.object(admission, limit, 3), self.assertRaisesRegex(ValueError, "budget"):
                layout()

    def test_export_members_are_released(self):
        from unittest.mock import patch

        original = tarfile.TarFile.next

        def check_cache(archive):
            if not archive.firstmember:
                self.assertFalse(archive.members)
            return original(archive)

        with patch.object(tarfile.TarFile, "next", check_cache):
            self.assertEqual(layout(), "Run main")

    def test_runtime_platform(self):
        platform = {"os": "linux", "architecture": "arm64"}
        for expected in ("linux/arm64", "linux/arm64/v8"):
            args = admission.runtime_args("image", "name", "label", Path("/tmp"), True, platform)
            self.assertEqual(args[args.index("--platform") + 1], expected)
            platform["variant"] = "v8"

    def test_invalid_members(self):
        for kwargs in [
            dict(skill=False),
            dict(skill=b" \n"),
            dict(skill=b"\xff"),
            dict(skill=b"x" * 262145),
            dict(code=False),
            dict(code=b""),
        ] + [
            dict(link=p)
            for p in ("spool", "spool/code", "spool/code/main", "spool/SKILL.md")
        ]:
            with self.subTest(kwargs=list(kwargs)), self.assertRaises(ValueError):
                layout(**kwargs)

    def test_digest_cache_and_pending_cleanup(self):
        import tempfile, json
        from unittest.mock import patch

        k = admission.k
        image = "image@sha256:" + "1" * 64
        platform = {
            "variant": "2" * 64,
            "platform": {"os": "linux", "architecture": "arm64"},
        }
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(k, "ROOT", Path(tmp)),
            patch.object(k, "DB", Path(tmp) / "db"),
            k.connect() as db,
            patch.object(admission, "variant", return_value=platform),
        ):
            k.put(
                db,
                "admission",
                image.split("@")[-1],
                dict(spec=admission.SPEC, **platform),
            )
            db.commit()
            with patch.object(admission.subprocess, "check_output") as cli:
                admission.check(db, image)
                cli.assert_not_called()
            key = image.split(":")[-1]
            k.put(db, "probe", key, {"name": "test", "stage": "cleanup"})
            db.commit()
            with patch.object(
                admission.subprocess,
                "check_output",
                return_value=json.dumps(
                    [{"id": "test", "configuration": {"labels": {}}}]
                ),
            ):
                with self.assertRaisesRegex(ValueError, "identity mismatch"):
                    admission.check(db, image)
            with patch.object(admission.subprocess, "check_output", return_value=b"[]"):
                admission.check(db, image)
            with self.assertRaises(ValueError):
                k.get(db, "probe", key)
            with (
                patch.object(admission, "SPEC", "spool/next"),
                self.assertRaisesRegex(ValueError, "needs_admission"),
            ):
                admission.require(db, image)

    def test_legacy_instance_and_foreign_skill_are_refused(self):
        import tempfile
        from unittest.mock import patch
        import instances

        k = admission.k
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(k, "ROOT", Path(tmp)),
            patch.object(k, "DB", Path(tmp) / "db"),
            k.connect() as db,
        ):
            image = "legacy"
            k.put(db, "image", image, image)
            task = k.task_new(db, [image], [], {})
            instance = dict(
                id=k.uid(),
                name="legacy-instance",
                spool=image,
                intent="load",
                status="running",
                active=None,
                task=task,
            )
            k.put(db, "instance", instance["id"], instance)
            db.commit()
            with (
                patch.object(instances, "observe", return_value="running"),
                patch.object(instances, "submit") as submit,
            ):
                with self.assertRaisesRegex(ValueError, "needs_admission"):
                    instances.call(task, "run", dict(instance=instance["id"]))
                with self.assertRaisesRegex(ValueError, "needs_admission"):
                    instances.call(task, "load", dict(spool=image))
                submit.assert_not_called()
            with self.assertRaisesRegex(ValueError, "outside task"):
                instances.call(task, "skill", dict(spool="foreign-image"))
