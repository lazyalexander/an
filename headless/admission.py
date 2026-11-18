"""Structural admission for native Apple Silicon spool images."""

import fcntl
import hashlib
import json
import shutil
import subprocess
import tarfile
from pathlib import PurePosixPath
import kernel as k

SPEC = "spool/v1"
MAX_SPOOL_ENTRIES = 16384
MAX_PATH_BYTES = 4096
MAX_SPOOL_PATH_BYTES = 4 * 1024 * 1024


def inspect_layout(stream):
    dirs = set()
    members = {}
    skill = None
    spool_path_bytes = 0
    with tarfile.open(fileobj=stream, mode="r|*") as archive:
        while (entry := archive.next()) is not None:
            # Streaming mode still caches TarInfo objects on older Python versions.
            archive.members.clear()
            path_bytes = len(entry.name.encode("utf-8", "surrogateescape"))
            if path_bytes > MAX_PATH_BYTES:
                raise ValueError("Export path exceeds length limit")
            path = PurePosixPath(entry.name)
            if path.parts[:1] != ("spool",):
                continue
            spool_path_bytes += path_bytes
            if len(members) >= MAX_SPOOL_ENTRIES or spool_path_bytes > MAX_SPOOL_PATH_BYTES:
                raise ValueError("Spool layout exceeds admission budget")
            if ".." in path.parts or path in members:
                raise ValueError("Ambiguous /spool archive path")
            members[path] = entry.isfile() and entry.size > 0
            if entry.isdir():
                dirs.add(path)
            if path == PurePosixPath("spool/SKILL.md"):
                if not entry.isfile() or not 0 < entry.size <= 262144:
                    raise ValueError("Expected regular SKILL.md of at most 256 KiB")
                skill = archive.extractfile(entry).read().decode("utf-8")
    if PurePosixPath("spool") not in dirs or not skill or not skill.strip():
        raise ValueError("Missing real /spool directory or nonempty UTF-8 SKILL.md")
    if not any(
        ok
        and path.parts[:2] == ("spool", "code")
        and len(path.parts) > 2
        and all(p in dirs for p in path.parents if p.parts)
        for path, ok in members.items()
    ):
        raise ValueError(
            "Expected real /spool/code directory with a nonempty regular file"
        )
    return skill


def runtime_args(image, name, labels, path, editable, platform):
    if "," in str(path):
        raise ValueError("Unsupported comma in mount path")
    argv = [
        "run",
        "--detach",
        "--platform",
        "/".join(platform[key] for key in ("os", "architecture", "variant") if platform.get(key)),
        "--name",
        name,
        "--label",
        labels,
        "--network",
        "none",
        "--read-only",
        "-w",
        "/work",
        "--entrypoint",
        "/bin/sleep",
    ]
    for d in ("input", "output", "work"):
        readonly = d == "input" or (d == "work" and not editable)
        argv += [
            "--mount",
            f"type=bind,source={path / d},target=/{d}"
            + (",readonly" if readonly else ""),
        ]
    return [*argv, image, "infinity"]


def variant(image):
    item = json.loads(
        subprocess.check_output([k.CONTAINER, "image", "inspect", image])
    )[0]
    if item["configuration"]["descriptor"]["digest"] != image.split("@")[-1]:
        raise ValueError("Local image digest mismatch")
    matches = [
        v
        for v in item["variants"]
        if v["platform"]["os"] == "linux" and v["platform"]["architecture"] == "arm64"
    ]
    if len(matches) != 1:
        raise ValueError("Expected one native linux/arm64 platform image")
    return {"platform": matches[0]["platform"], "variant": matches[0]["digest"]}


def record(db, image):
    row = db.execute(
        "SELECT body FROM objects WHERE kind='admission' AND id=?",
        (image.split("@")[-1],),
    ).fetchone()
    data = json.loads(row[0]) if row else {}
    return {**data, "image": image} if data.get("spec") == SPEC else {}


def summary(db, image):
    data = record(db, image)
    data.pop("skill", None)
    return {**data, "status": "admitted" if data else "needs_admission"}


def require(db, image):
    data = record(db, image)
    if not data or any(data.get(k) != v for k, v in variant(image).items()):
        raise ValueError(f"needs_admission: run an register {image}")
    return data


def check(db, image):
    info = variant(image)
    directory = k.ROOT / "probes"
    directory.mkdir(exist_ok=True)
    key = image.split("@")[-1].split(":")[1]
    with (directory / (key + ".lock")).open("ab") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)

        def cli(*args):
            return subprocess.check_output(
                [k.CONTAINER, *args], pass_fds=(lock.fileno(),)
            )

        def cleanup(probe):
            items = json.loads(cli("list", "--all", "--format", "json"))
            hit = next((x for x in items if x["id"] == probe), None)
            if hit:
                if hit["configuration"]["labels"].get("an.probe") != probe:
                    raise ValueError("Probe identity mismatch")
                cli("delete", "--force", probe)
                if any(
                    x["id"] == probe
                    for x in json.loads(cli("list", "--all", "--format", "json"))
                ):
                    raise RuntimeError("Probe still present")
            if (directory / probe).exists():
                shutil.rmtree(directory / probe)
            db.execute("DELETE FROM objects WHERE kind='probe' AND id=?", (key,))
            db.commit()

        old = db.execute(
            "SELECT body FROM objects WHERE kind='probe' AND id=?", (key,)
        ).fetchone()
        if old:
            cleanup(json.loads(old[0])["name"])
        data = record(db, image)
        if data and all(data.get(k) == v for k, v in info.items()):
            return
        probe = "an-probe-" + k.uid()
        path = directory / probe

        def phase(stage):
            k.put(db, "probe", key, dict(name=probe, stage=stage), True)
            db.commit()

        phase("startup")
        try:
            for d in ("input", "output", "work"):
                (path / d).mkdir(parents=True)
            try:
                cli(*runtime_args(image, probe, "an.probe=" + probe, path, True, info["platform"]))
            except subprocess.CalledProcessError as exc:
                raise ValueError(
                    "Probe startup failed: requires /bin/sleep infinity and available Apple container service"
                ) from exc
            item = json.loads(cli("inspect", probe))[0]
            cfg = item["configuration"]
            mounts = {m["destination"]: m for m in cfg["mounts"]}
            if (
                item["status"]["state"] != "running"
                or cfg["platform"] != info["platform"]
                or cfg["image"]["descriptor"]["digest"] != image.split("@")[1]
                or cfg["networks"]
                or not cfg["readOnly"]
                or set(mounts) != {"/input", "/output", "/work"}
                or any(
                    m["source"] != str(path / d[1:])
                    or ("ro" in m["options"]) != (d == "/input")
                    for d, m in mounts.items()
                )
            ):
                raise ValueError("Probe platform, readiness or mount mismatch")
            phase("layout")
            cli("stop", probe)
            with subprocess.Popen(
                [k.CONTAINER, "export", probe],
                stdout=subprocess.PIPE,
                pass_fds=(lock.fileno(),),
            ) as process:
                try:
                    skill = inspect_layout(process.stdout)
                finally:
                    process.stdout.close()
                if process.wait():
                    raise ValueError("Image export failed")
        except Exception as exc:
            k.put(
                db,
                "admission_failure",
                image,
                dict(
                    probe=probe, stage=k.get(db, "probe", key)["stage"], detail=str(exc)
                ),
                True,
            )
            db.commit()
            raise
        finally:
            phase("cleanup")
            try:
                cleanup(probe)
            except Exception as exc:
                raise RuntimeError(
                    f"Probe cleanup pending: {probe}; retry registration"
                ) from exc
        k.put(
            db,
            "admission",
            image.split("@")[-1],
            dict(
                spec=SPEC,
                image=image,
                **info,
                skill=skill,
                skill_digest=hashlib.sha256(skill.encode()).hexdigest(),
            ),
            True,
        )
        db.commit()
