"""Image spool registry and task scheduler. Requires Python 3.11+."""

import argparse
import base64
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
import uuid
from pathlib import Path

ROOT = Path(os.environ.get("AN_HOME", Path.home() / ".an")).resolve()
ROOT.mkdir(mode=0o700, parents=True, exist_ok=True)
CONTAINER = "/usr/local/bin/container"
DB = ROOT / "registry.sqlite3"


def connect():
    db = sqlite3.connect(DB, timeout=20)
    db.execute(
        "CREATE TABLE IF NOT EXISTS objects (kind, id, body, PRIMARY KEY(kind,id))"
    )
    return db


def put(db, kind, key, value, replace=False):
    body = json.dumps(value, sort_keys=True)
    if replace:
        db.execute("INSERT OR REPLACE INTO objects VALUES (?,?,?)", (kind, key, body))
    else:
        db.execute("INSERT INTO objects VALUES (?,?,?)", (kind, key, body))


def get(db, kind, key):
    row = db.execute(
        "SELECT body FROM objects WHERE kind=? AND id=?", (kind, key)
    ).fetchone()
    if not row:
        raise ValueError(f"Unknown {kind}: {key}")
    return json.loads(row[0])


def safe(name):
    if (
        not isinstance(name, str)
        or not name
        or any(p in ("", ".", "..") for p in name.split("/"))
    ):
        raise ValueError("Expected a relative file path")
    return name


def blob(data):
    return base64.b64encode(data).decode("ascii")


def snapshot(folder):
    files = {}
    if folder.is_symlink() or not folder.is_dir():
        raise ValueError("Expected generator directory")
    for path in folder.rglob("*"):
        if path.is_symlink() or not (path.is_dir() or path.is_file()):
            raise ValueError("Only regular generator files supported")
        if path.is_file():
            files[str(path.relative_to(folder))] = {
                "data": blob(path.read_bytes()),
                "mode": path.stat().st_mode & 0o777,
            }
    return files


def revision(db, files):
    key = hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    put(db, "revision", key, files, True)
    return key


def inspect_image(ref):
    if not ref or ref.startswith("-"):
        raise ValueError("Expected a local image reference")
    return json.loads(subprocess.check_output([CONTAINER, "image", "inspect", ref]))[0][
        "configuration"
    ]


def register(db, ref):
    config = inspect_image(ref)
    digest = config["descriptor"]["digest"]
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError("Expected a SHA-256 image descriptor")
    name = config["name"].split("@")[0]
    repo, sep, last = name.rpartition("/")
    image = repo + sep + last.split(":")[0] + "@" + digest
    if "@" in ref and ref.split("@")[-1] != digest:
        raise ValueError("Requested digest does not match the local image")
    if image != config["name"]:
        subprocess.run(
            [CONTAINER, "image", "tag", ref, image],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    if inspect_image(image)["descriptor"]["digest"] != digest:
        raise ValueError("Image changed during registration")
    put(db, "image", image, image, True)
    return image


def uid():
    return uuid.uuid4().hex


def task_new(db, releases, editable, inputs):
    if not set(editable) <= set(releases):
        raise ValueError("Editable spool must be granted")
    t = dict(
        work={get(db, "image", r): revision(db, {}) for r in releases},
        editable=editable,
        inputs=inputs,
    )
    key = uid()
    put(db, "task", key, t)
    return key


def stage(folder, files):
    folder.mkdir()
    for name, data in files.items():
        path = folder / safe(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(data, dict):
            path.write_bytes(base64.b64decode(data["data"]))
            path.chmod(data["mode"])
        else:
            path.write_bytes(base64.b64decode(data))


def owned(db, task, run):
    r = get(db, "run", run)
    if r["task"] != task:
        raise ValueError("Run outside task")
    return r


def worker(key):
    with connect() as db:
        db.execute("BEGIN IMMEDIATE")
        r = get(db, "run", key)
        db.commit()
        image = get(db, "image", r["spool"])
        path = ROOT / "runs" / key
        path.mkdir(parents=True)
        try:
            if inspect_image(image)["descriptor"]["digest"] != image.split("@")[-1]:
                raise ValueError("Registered image digest changed")
            stage(path / "work", get(db, "revision", r["work"]))
            stage(path / "input", get(db, "revision", r["input"]))
            out = path / "output"
            out.mkdir()
            if "," in str(path):
                raise ValueError("Container mount path contains a comma")
            cmd = [
                CONTAINER,
                "run",
                "--name",
                "an-" + key,
                "--read-only",
                "--network",
                "none",
                "-w",
                "/work",
                "--entrypoint",
                r["argv"][0],
            ]
            for directory in ("input", "work", "output"):
                mount = f"type=bind,source={path / directory},target=/{directory}"
                cmd += [
                    "--mount",
                    mount
                    + (
                        ",readonly"
                        if directory == "input"
                        or (directory == "work" and not r["editable"])
                        else ""
                    ),
                ]
            with (path / "log").open("wb") as log:
                process = subprocess.Popen(
                    cmd + [image, *r["argv"][1:]], stdout=log, stderr=log
                )
                while process.poll() is None:
                    if get(db, "run", key)["status"] == "cancel_pending":
                        subprocess.run(
                            [CONTAINER, "stop", "an-" + key], capture_output=True
                        )
                    time.sleep(0.2)
                if process.returncode:
                    raise RuntimeError(
                        f"container exited {process.returncode}; see {path / 'log'}"
                    )
            files = {}
            for name in r["outputs"]:
                p = out / name
                if p.resolve() != p or not p.is_file():
                    raise ValueError("Missing or invalid declared output")
                files[name] = blob(p.read_bytes())
            db.execute("BEGIN IMMEDIATE")
            r = get(db, "run", key)
            if r["status"] == "running":
                r.update(status="delivered", output=revision(db, files))
                if r["editable"]:
                    t = get(db, "task", r["task"])
                    r["result_work"] = revision(db, snapshot(path / "work"))
                    t["work"][image] = r["result_work"]
                    put(db, "task", r["task"], t, True)
        except Exception as e:
            db.rollback()
            db.execute("BEGIN IMMEDIATE")
            r = get(db, "run", key)
            if r["status"] == "running":
                r.update(status="failed", error=str(e))
        if r["status"] == "cancel_pending":
            stopped = subprocess.run(
                [CONTAINER, "delete", "an-" + key], capture_output=True
            )
            if stopped.returncode == 0:
                r["status"] = "cancelled"
        put(db, "run", key, r, True)
    if Path(CONTAINER).exists() and r["status"] != "cancelled":
        subprocess.run(
            [CONTAINER, "delete", "an-" + key], capture_output=True, check=False
        )


def call(task, name, args):
    with connect() as db:
        db.execute("BEGIN IMMEDIATE")
        t = get(db, "task", task)
        if "work" not in t:
            raise ValueError(
                "Legacy directory task. Start a new task with a registered image."
            )
        if name == "state":
            runs = {
                key: json.loads(body)["status"]
                for key, body in db.execute(
                    "SELECT id,body FROM objects WHERE kind='run' AND json_extract(body,'$.task')=?",
                    (task,),
                )
            }
            return {**t, "runs": runs}
        if name == "run":
            s = args["spool"]
            gen = t["work"][s]
            if (
                not args["argv"]
                or not args["argv"][0]
                or args["argv"][0].startswith("-")
            ):
                raise ValueError("Supply a guest executable and its arguments after --")
            outputs = [safe(p) for p in args["outputs"]]
            if (
                s in t["editable"]
                and db.execute(
                    "SELECT 1 FROM objects WHERE kind='run' AND json_extract(body,'$.task')=? AND json_extract(body,'$.spool')=? AND json_extract(body,'$.status') IN ('running','cancel_pending','unknown')",
                    (task, s),
                ).fetchone()
            ):
                raise ValueError(
                    "Another command still owns the editable work revision"
                )
            ref = args["input"]
            inp = t["inputs"].get(ref)
            if ref is None:
                inp = revision(db, {})
            elif inp is None:
                upstream = owned(db, task, ref)
                if upstream["status"] != "delivered":
                    raise ValueError("Input has not been delivered")
                inp = upstream["output"]
            key = uid()
            put(
                db,
                "run",
                key,
                dict(
                    task=task,
                    spool=s,
                    work=gen,
                    argv=args["argv"],
                    outputs=outputs,
                    editable=s in t["editable"],
                    input=inp,
                    source=ref,
                    status="running",
                ),
            )
            worker_process = subprocess.Popen(
                [sys.executable, __file__, "_worker", key],
                start_new_session=True,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            run = get(db, "run", key)
            run["worker_pid"] = worker_process.pid
            put(db, "run", key, run, True)
            return key
        r = owned(db, task, args["run"])
        if name == "cancel" and r["status"] == "running":
            r["status"] = "cancel_pending"
            put(db, "run", args["run"], r, True)
            db.commit()
        if (
            name == "status"
            and r["status"] in ("running", "cancel_pending")
            and r.get("worker_pid")
        ):
            try:
                os.kill(r["worker_pid"], 0)
            except ProcessLookupError:
                inspect = subprocess.run(
                    [CONTAINER, "inspect", "an-" + args["run"]], capture_output=True
                )
                r.update(status="unknown", container_observed=inspect.returncode == 0)
                put(db, "run", args["run"], r, True)
        if name == "log":
            return (
                (ROOT / "runs" / args["run"] / "log")
                .read_bytes()[-65536:]
                .decode(errors="replace")
            )
        return r


def main():
    if sys.argv[1:2] == ["_worker"]:
        return worker(sys.argv[2])
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    export = sub.add_parser("export")
    export.add_argument("delivery")
    export.add_argument("destination", type=Path)
    registration = sub.add_parser("register")
    registration.add_argument("image")
    registration.add_argument(
        "--archive", type=Path, help="Load an OCI image archive before registration"
    )
    for cmd in ("state", "run", "status", "cancel", "log"):
        action = sub.add_parser(cmd)
        action.add_argument("--task", required=True)
        if cmd == "run":
            action.add_argument("--spool", required=True)
            action.add_argument("--input")
            action.add_argument("--output", dest="outputs", action="append", default=[])
            action.add_argument("argv", nargs=argparse.REMAINDER)
        elif cmd != "state":
            action.add_argument("run")

    sub.add_parser("mcp").add_argument("--task", required=True)
    servers = sub.add_parser("server").add_subparsers(dest="action", required=True)
    reg = servers.add_parser("register")
    reg.add_argument("name")
    reg.add_argument("argv", nargs=argparse.REMAINDER)
    servers.add_parser("tools").add_argument("name")
    tasks = sub.add_parser("task").add_subparsers(dest="action", required=True)
    new = tasks.add_parser("new")
    new.add_argument("--spool", action="append", default=[])
    new.add_argument("--allow-tool", action="append", default=[])
    new.add_argument("--editable", action="append", default=[])
    new.add_argument("--input", type=Path, action="append", default=[])
    resume = tasks.add_parser("resume")
    resume.add_argument("delivery")
    resume.add_argument("--editable", action="append", default=[])
    a = parser.parse_args()
    if a.cmd in ("server", "mcp"):
        import asyncio
        import gateway

        if a.cmd == "mcp":
            asyncio.run(gateway.serve(a.task))
        elif a.action == "tools":
            print(json.dumps(asyncio.run(gateway.list_server(a.name))))
        else:
            gateway.register(a.name, a.argv[1:] if a.argv[:1] == ["--"] else a.argv)
        return
    if a.cmd in ("state", "run", "status", "cancel", "log"):
        if a.cmd == "run" and a.argv[:1] == ["--"]:
            a.argv = a.argv[1:]
        result = call(a.task, a.cmd, vars(a))
        print(json.dumps(result, ensure_ascii=False))
        return
    with connect() as db:
        if a.cmd == "export":
            r = get(db, "run", a.delivery)
            stage(a.destination, get(db, "revision", r["output"]))
            return
        if a.cmd == "register":
            if a.archive:
                subprocess.run(
                    [CONTAINER, "image", "load", "--input", str(a.archive)],
                    check=True,
                    stdout=sys.stderr,
                )
            print(register(db, a.image))
            return
        if a.action == "new":
            inputs = {
                p.name: revision(db, {p.name: blob(p.read_bytes())}) for p in a.input
            }
            if len(inputs) != len(a.input):
                raise ValueError("Duplicate input name")
            if not a.spool and not a.allow_tool:
                raise ValueError("Grant a spool or a downstream tool")
            key = task_new(db, a.spool, a.editable, inputs)
            if a.allow_tool:
                from gateway import grant

                task = get(db, "task", key)
                task["tools"] = grant(db, a.allow_tool)
                put(db, "task", key, task, True)
        else:
            r = get(db, "run", a.delivery)
            if r["status"] != "delivered":
                raise ValueError("Expected a delivered run")
            key = task_new(db, [r["spool"]], a.editable, {"original": r["input"]})
            t = get(db, "task", key)
            t["work"][r["spool"]] = r.get("result_work", r["work"])
            put(db, "task", key, t, True)
        print(
            json.dumps(
                {
                    "task": key,
                    "mcp_servers": {
                        "an": {
                            "command": os.environ.get("AN_EXECUTABLE", "an"),
                            "args": ["mcp", "--task", key],
                            "env": {"AN_HOME": str(ROOT), "AN_PYTHON": sys.executable},
                        }
                    },
                }
            )
        )


if __name__ == "__main__":
    main()
