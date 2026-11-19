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
import uuid
from pathlib import Path

ROOT = Path(os.environ.get("AN_HOME", Path.home() / ".an")).resolve()
CONTAINER = "/usr/local/bin/container"
DB = ROOT / "registry.sqlite3"


def connect():
    DB.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
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
    from admission import check

    check(db, image)
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


def stage(folder, files, existing=False):
    folder.mkdir(exist_ok=existing)
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


def legacy_status(db, r, key, op):
    if op == "cancel" and r["status"] == "running":
        r["status"] = "cancel_pending"
    if r["status"] in ("running", "cancel_pending") and r.get("worker_pid"):
        try:
            os.kill(r["worker_pid"], 0)
        except ProcessLookupError:
            r["status"] = "unknown"
    put(db, "run", key, r, True)
    return r


def call(task, op, args):
    if op == "deliveries":
        from workspaces import deliveries

        with connect() as db:
            return deliveries(db, task, args.get("after", 0))
    from instances import call as dispatch

    return dispatch(task, op, args)


def mcp_servers(selector):
    return {
        "an": {
            "command": os.environ.get("AN_EXECUTABLE", "an"),
            "args": ["mcp", *selector],
            "env": {"AN_HOME": str(ROOT), "AN_PYTHON": sys.executable},
        }
    }


def main():
    if sys.argv[1:2] == ["_worker"]:
        from instances import worker

        return worker(sys.argv[2], int(sys.argv[3]))
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    control = sub.add_parser("permission")
    control.add_argument("action", choices=["begin","end","pending","decide"])
    control.add_argument("--task", required=True)
    control.add_argument("--request-id")
    control.add_argument("--decision", choices=["once","turn","deny"])
    export = sub.add_parser("export")
    export.add_argument("--request-id")
    export.add_argument("delivery")
    export.add_argument("destination", type=Path)
    registration = sub.add_parser(
        "register",
        description="Register a spool image providing /bin/sleep infinity as its idle process.",
    )
    registration.add_argument("image")
    registration.add_argument(
        "--archive", type=Path, help="Load an OCI image archive before registration"
    )
    for cmd in ("load", "unload", "state", "run", "status", "cancel", "log", "publish"):
        action = sub.add_parser(cmd)
        action.add_argument("--request-id")
        action.add_argument("--task", required=True)
        if cmd == "load":
            action.add_argument("--spool", required=True)
        elif cmd == "unload":
            action.add_argument("--instance", required=True)
        elif cmd == "run":
            target = action.add_mutually_exclusive_group(required=True)
            target.add_argument("--spool")
            target.add_argument("--instance")
            action.add_argument("--input")
            action.add_argument("--output", dest="outputs", action="append", default=[])
            action.add_argument("argv", nargs=argparse.REMAINDER)
        elif cmd != "state":
            action.add_argument("run")

    import workspaces as ws

    mcp = sub.add_parser("mcp")
    mcp.add_argument("--task")
    ws.selector(mcp)
    workspace = sub.add_parser("workspace").add_subparsers(dest="action", required=True)
    create = workspace.add_parser("create")
    create.add_argument("--spool", action="append", required=True)
    create.add_argument("--editable", action="append", default=[])
    create.add_argument("--allow-tool", action="append", default=[])
    bind = workspace.add_parser("bind")
    bind.add_argument("workspace")
    ws.selector(bind)
    deliveries = sub.add_parser("deliveries")
    deliveries.add_argument("--task", required=True)
    deliveries.add_argument("--after", type=int, default=0)
    servers = sub.add_parser("server").add_subparsers(dest="action", required=True)
    reg = servers.add_parser("register")
    reg.add_argument("name")
    reg.add_argument("argv", nargs=argparse.REMAINDER)
    servers.add_parser("tools").add_argument("name")
    tasks = sub.add_parser("task").add_subparsers(dest="action", required=True)
    new = tasks.add_parser("new")
    ws.selector(new)
    new.add_argument("--spool", action="append", default=[])
    new.add_argument("--allow-tool", action="append", default=[])
    new.add_argument("--editable", action="append", default=[])
    new.add_argument("--input", type=Path, action="append", default=[])
    resume = tasks.add_parser("resume")
    resume.add_argument("delivery")
    resume.add_argument("--editable", action="append", default=[])
    a = parser.parse_args()
    import permissions
    if a.cmd == "permission":
        with connect() as db:
            print(json.dumps(permissions.control(db, a.task, a.action, a.request_id, a.decision)))
        return
    selected = any(
        getattr(a, n, None) is not None for n in ("session", "key", "harness")
    )
    if a.cmd == "workspace":
        with connect() as db:
            db.execute("BEGIN IMMEDIATE")
            if a.action == "create":
                result = {"workspace": ws.create(db, a)}
            else:
                result = {
                    "workspace": ws.bind(db, a),
                    "mcp_servers": mcp_servers(
                        [
                            "--harness",
                            a.harness,
                            "--session" if a.session else "--key",
                            a.session or a.key,
                        ]
                    ),
                }
        print(json.dumps(result))
        return
    if a.cmd == "deliveries":
        print(json.dumps(call(a.task, "deliveries", vars(a))))
        return
    if a.cmd == "task" and selected:
        if a.spool or a.allow_tool or a.editable or a.input:
            parser.error("Bound tasks inherit workspace configuration")
        print(json.dumps({"task": ws.resolve(a, fresh=True)}))
        return
    if a.cmd in ("server", "mcp"):
        import asyncio
        import gateway

        if a.cmd == "mcp":
            if bool(a.task) == selected:
                parser.error("Select either task ID or a bound identity")
            asyncio.run(
                gateway.serve(
                    ws.resolve(a) if selected else a.task,
                    ws.identity(a) if selected else None,
                )
            )
        elif a.action == "tools":
            print(json.dumps(asyncio.run(gateway.list_server(a.name))))
        else:
            gateway.register(a.name, a.argv[1:] if a.argv[:1] == ["--"] else a.argv)
        return
    if a.cmd in ("load", "unload", "state", "run", "status", "cancel", "log", "publish"):
        if a.cmd == "run" and a.argv[:1] == ["--"]:
            a.argv = a.argv[1:]
        result = call(a.task, a.cmd, vars(a))
        print(json.dumps(result, ensure_ascii=False))
        return
    with connect() as db:
        if a.cmd == "export":
            r = get(db, "run", a.delivery)
            if r["status"] != "delivered":
                raise ValueError("Expected published delivery")
            destination = a.destination.resolve()
            pending = permissions.gate(db, r["task"], "export", [a.delivery, r["output"], str(destination)], {}, a.request_id)
            db.commit()
            if pending:
                print(json.dumps(pending))
                return
            stage(destination, get(db, "revision", r["output"]))
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
        print(json.dumps({"task": key, "mcp_servers": mcp_servers(["--task", key])}))


if __name__ == "__main__":
    main()
