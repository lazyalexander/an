"""Task-owned Apple containers. Serialize control decisions per task."""

import base64
import hashlib
import fcntl
import json
import os
import shutil
import subprocess
import sys
import time
from contextlib import contextmanager
from pathlib import Path

import permissions
import admission
import kernel as k


@contextmanager
def locked(task):
    # Validate the task before using its identifier as a filesystem component.
    with k.connect() as db:
        k.get(db, "task", task)
    directory = k.ROOT / "locks"
    directory.mkdir(exist_ok=True)
    with (directory / task).open("ab") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield lock.fileno()


def save(db, kind, rec):
    k.put(db, kind, rec["id"], rec, True)
    db.commit()


def apple(argv, lock):
    result = subprocess.run(
        [k.CONTAINER, *argv], capture_output=True, text=True, pass_fds=(lock,)
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"container {argv[0]} failed")
    return result.stdout


def observe(i, lock):
    try:
        items = json.loads(apple(["list", "--all", "--format", "json"], lock))
        hit = next((item for item in items if item["id"] == i["name"]), None)
        if hit is None:
            return "absent"
        cfg = hit["configuration"]
        if (
            cfg["labels"].get("an.instance") != i["id"]
            or cfg["image"]["descriptor"]["digest"] != i["spool"].split("@")[-1]
        ):
            return "identity_mismatch"
        return hit["status"]["state"]
    except (OSError, ValueError, KeyError, RuntimeError) as exc:
        i["error"] = str(exc)
        return "unknown"


def lease_alive(run):
    with (k.ROOT / "runs" / run / "lease").open("ab") as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return False
        except BlockingIOError:
            return True


def reconcile(db, i, lock):
    if i["status"] == "unloaded":
        return i
    actual = observe(i, lock)
    i.update(observed=actual, checked_at=time.time())
    active = i["active"]
    if active:
        r = k.get(db, "run", active)
        if not lease_alive(active) and r["status"] == "running":
            r["status"] = "unknown"
            save(db, "run", r)
    if i["intent"] in ("unload", "stop"):
        # No retries of guest commands. Only converge recorded lifecycle intent.
        if actual == "running":
            try:
                apple(["stop", i["name"]], lock)
            except RuntimeError as exc:
                i["error"] = str(exc)
            actual = observe(i, lock)
        if actual in ("stopped", "created", "exited") and i["intent"] == "unload":
            try:
                apple(["delete", i["name"]], lock)
            except RuntimeError as exc:
                i["error"] = str(exc)
            actual = observe(i, lock)
        confirmed = actual == "absent" or (
            actual in ("stopped", "exited", "created") and i["intent"] == "stop"
        )
        i["status"] = (
            ("unloaded" if actual == "absent" else "stopped")
            if confirmed
            else "cleanup_pending"
        )
        if confirmed and active:
            r = k.get(db, "run", active)
            r["status"] = "cancelled"
            k.put(db, "run", active, r, True)
            i["active"] = None
    elif actual == "running":
        i["status"] = "running"
    elif actual in ("absent", "stopped", "created", "exited"):
        i["status"] = "stopped"
    else:
        i["status"] = "unknown"
    if i["status"] == "unloaded":
        # Committed revisions and per-run logs live outside disposable mounts.
        try:
            shutil.rmtree(k.ROOT / "instances" / i["id"])
        except FileNotFoundError:
            pass
        except OSError as exc:
            i.update(status="cleanup_pending", error=str(exc), intent="unload")
    i["observed"] = actual
    save(db, "instance", i)
    return i


def instances(db, task):
    return [
        json.loads(body)
        for (body,) in db.execute(
            "SELECT body FROM objects WHERE kind='instance' AND json_extract(body,'$.task')=? ORDER BY rowid",
            (task,),
        )
    ]


def load(db, task, t, spool, lock):
    work = t["work"][spool]
    platform = admission.require(db, spool)["platform"]
    for old in instances(db, task):
        if old["spool"] != spool or old["status"] == "unloaded":
            continue
        reconcile(db, old, lock)
        if old["status"] == "running":
            if old.get("platform") != platform:
                raise ValueError("Unload the instance before loading the admitted platform")
            return old
        if old["status"] == "stopped" and not old["active"]:
            old["intent"] = "unload"
            save(db, "instance", old)
            reconcile(db, old, lock)
        if old["status"] != "unloaded":
            raise ValueError(f"Resolve instance {old['id']} before loading")
    ident = k.uid()
    i = dict(
        id=ident,
        name="an-" + ident,
        task=task,
        spool=spool,
        platform=platform,
        status="loading",
        intent="load",
        active=None,
    )
    save(db, "instance", i)
    path = k.ROOT / "instances" / ident
    try:
        if "," in str(path):
            raise ValueError("Unsupported comma in mount path")
        path.mkdir(parents=True)
        k.stage(path / "work", k.get(db, "revision", work))
        (path / "input").mkdir()
        (path / "output").mkdir()
        if k.inspect_image(spool)["descriptor"]["digest"] != spool.split("@")[-1]:
            raise ValueError("Image digest changed")
        apple(
            admission.runtime_args(
                spool, i["name"], "an.instance=" + ident, path, spool in t["editable"], platform
            ),
            lock,
        )
    except Exception as exc:
        i.update(error=str(exc), intent="unload")
        save(db, "instance", i)
        reconcile(db, i, lock)
        raise ValueError(
            f"Failed to load {ident}: {exc}; cleanup: {i['status']}"
        ) from exc
    reconcile(db, i, lock)
    if i["status"] != "running":
        raise ValueError(f"Instance {ident} failed to enter running state: {i}")
    return i


def reset_folder(path, files):
    # Preserve matching inodes on live virtiofs mounts (including executables).
    for root, dirs, names in os.walk(path, topdown=False, followlinks=False):
        for name in names + dirs:
            child = Path(root) / name
            rel = str(child.relative_to(path))
            if child.is_symlink():
                child.unlink()
            elif child.is_dir():
                if not any(key.startswith(rel + "/") for key in files):
                    child.rmdir()
            elif rel not in files or not child.is_file():
                child.unlink()
    for rel, entry in files.items():
        target = path / k.safe(rel)
        data = base64.b64decode(entry["data"] if isinstance(entry, dict) else entry)
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.exists() or target.read_bytes() != data:
            if target.exists():
                target.chmod(0o600)
            target.write_bytes(data)
        if isinstance(entry, dict):
            target.chmod(entry["mode"])


def worker(run, lease):
    # The inherited lease closes on exit, including abnormal termination.
    os.set_inheritable(lease, False)
    with k.connect() as db:
        r = k.get(db, "run", run)
        task = r["task"]
        guest_quiet = True
        try:
            with locked(task) as lock:
                i = k.get(db, "instance", r["instance"])
                if i["active"] != run or i["intent"] != "load":
                    return
                if observe(i, lock) != "running":
                    raise RuntimeError(
                        "Could not confirm owned running instance before exec"
                    )
                path = k.ROOT / "instances" / i["id"]
                if r.get("source") is not None:
                    db.execute("BEGIN IMMEDIATE")
                    if permissions.stamp(db, task) != r.get("input_stamp"):
                        raise ValueError("Input authority changed")
                reset_folder(path / "input", k.get(db, "revision", r["input"]))
                db.commit()
                reset_folder(path / "output", {})
                reset_folder(path / "work", k.get(db, "revision", r["work"]))
                with (k.ROOT / "runs" / run / "log").open("wb") as log:
                    guest_quiet = False
                    process = subprocess.Popen(
                        [k.CONTAINER, "exec", i["name"], *r["argv"]],
                        stdout=log,
                        stderr=log,
                    )
            code = process.wait()
            with locked(task) as lock:
                i = k.get(db, "instance", i["id"])
                if i["active"] != run or i["intent"] != "load":
                    return
                # An exec client only waits for the foreground process. Stop the
                # VM before observing shared mounts, including on nonzero exit.
                if observe(i, lock) != "running":
                    raise RuntimeError("Cannot verify instance at command settlement")
                apple(["stop", i["name"]], lock)
                if observe(i, lock) != "stopped":
                    raise RuntimeError("Cannot confirm guest termination")
                guest_quiet = True
                apple(["start", i["name"]], lock)
                if observe(i, lock) != "running":
                    raise RuntimeError("Instance restart not confirmed")
                if code:
                    raise RuntimeError(f"Guest command exited {code}")
                files = {}
                for name in r["outputs"]:
                    p = path / "output" / name
                    if p.resolve() != p or not p.is_file():
                        raise ValueError(f"Missing or invalid output: {name}")
                    files[name] = k.blob(p.read_bytes())
                r.update(status="awaiting_publication" if files else "completed")
                if files:
                    r["candidate"] = k.revision(db, files)
                if r["editable"]:
                    t = k.get(db, "task", task)
                    r["result_work"] = k.revision(db, k.snapshot(path / "work"))
                    t["work"][i["spool"]] = r["result_work"]
                    k.put(db, "task", task, t, True)
                k.put(db, "run", run, r, True)
                i["active"] = None
                save(db, "instance", i)
        except Exception as exc:
            db.rollback()
            with locked(task):
                i = k.get(db, "instance", r["instance"])
                if i["active"] == run and i["intent"] == "load":
                    r = k.get(db, "run", run)
                    r.update(status="failed", error=str(exc))
                    if not guest_quiet:
                        i.update(intent="stop", status="cleanup_pending")
                    elif r["editable"]:
                        try:
                            reset_folder(
                                k.ROOT / "instances" / i["id"] / "work",
                                k.get(db, "revision", r["work"]),
                            )
                        except OSError as restore_error:
                            i.update(
                                intent="stop",
                                status="cleanup_pending",
                                error=str(restore_error),
                            )
                    k.put(db, "run", run, r, True)
                    i["active"] = None
                    save(db, "instance", i)


def submit(db, i, args):
    db.execute("BEGIN IMMEDIATE")
    t = k.get(db, "task", i["task"])
    if i["status"] != "running" or i["intent"] != "load" or i["active"]:
        raise ValueError("Instance is not ready")
    if (
        i["spool"] in t["editable"]
        and db.execute(
            """SELECT 1 FROM objects WHERE kind='run'
           AND json_extract(body,'$.task')=? AND json_extract(body,'$.spool')=?
           AND json_extract(body,'$.instance') IS NULL
           AND json_extract(body,'$.status') IN ('running','cancel_pending','unknown') LIMIT 1""",
            (i["task"], i["spool"]),
        ).fetchone()
    ):
        raise ValueError("Unresolved legacy run owns this editable spool")
    argv = args["argv"]
    if not argv or not argv[0] or argv[0].startswith("-"):
        raise ValueError("Expected a guest executable")
    outputs = [k.safe(p) for p in args["outputs"]]
    source = args["input"]
    ticket = None
    if source is None:
        inp = k.revision(db, {})
    elif source in t["inputs"]:
        inp = t["inputs"][source]
    elif source.startswith("handoff-"):
        ticket = k.get(db, "handoff", source)
        if (ticket["task"] != i["task"] or ticket["recipient"] != i["spool"]
                or ticket.get("used") or ticket["stamp"] != permissions.stamp(db, i["task"])):
            raise ValueError("Invalid or revoked handoff")
        inp = ticket["revision"]
    else:
        from workspaces import upstream

        inp = upstream(db, i["task"], source)
    if source is not None and ticket is None:
        flow = (["input", source, i["spool"]] if source in t["inputs"]
                else ["handoff", k.get(db, "run", source)["spool"], i["spool"]])
        pending = permissions.gate(db, i["task"], "input", [source, inp, i["spool"]], [i["id"], argv, outputs], args.get("request_id"), flow)
        if pending:
            db.commit()
            return pending
    run = k.uid()
    path = k.ROOT / "runs" / run
    path.mkdir(parents=True)
    with (path / "lease").open("xb") as lease:
        fcntl.flock(lease, fcntl.LOCK_EX)
        r = dict(
            id=run,
            instance=i["id"],
            task=i["task"],
            spool=i["spool"],
            work=t["work"][i["spool"]],
            argv=argv,
            input=inp,
            source=source,
            outputs=outputs,
            editable=i["spool"] in t["editable"],
            status="running",
        )
        if source is not None:
            r["input_stamp"] = permissions.stamp(db, i["task"])
        if ticket:
            ticket["used"] = run
            k.put(db, "handoff", source, ticket, True)
        i["active"] = run
        k.put(db, "run", run, r)
        save(db, "instance", i)
        try:
            subprocess.Popen(
                [sys.executable, k.__file__, "_worker", run, str(lease.fileno())],
                pass_fds=(lease.fileno(),),
                start_new_session=True,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except OSError as exc:
            if ticket:
                ticket.pop("used")
                k.put(db, "handoff", source, ticket, True)
            r.update(status="failed", error=f"Worker launch failed: {exc}")
            i["active"] = None
            k.put(db, "run", run, r, True)
            save(db, "instance", i)
    return run


def call(task, op, args):
    with locked(task) as lock, k.connect() as db:
        t = k.get(db, "task", task)
        if op == "load":
            return load(db, task, t, args["spool"], lock)
        if op == "skill":
            image = args["spool"]
            if image not in t["work"]:
                raise ValueError("Spool outside task")
            return admission.require(db, image)
        if op == "state":
            items = [reconcile(db, i, lock) for i in instances(db, task)]
            runs = {
                key: json.loads(body)["status"]
                for key, body in db.execute(
                    "SELECT id,body FROM objects WHERE kind='run' AND json_extract(body,'$.task')=?",
                    (task,),
                )
            }
            return {
                **t,
                "spools": {s: admission.summary(db, s) for s in t["work"]},
                "task": task,
                "instances": items,
                "runs": runs,
            }
        if op in ("status", "log", "cancel", "publish", "handoff"):
            r = k.owned(db, task, args["run"])
            if op in ("handoff", "publish"):
                if r["status"] != "awaiting_publication":
                    raise ValueError("Expected private candidate")
                files = k.get(db, "revision", r["candidate"])
                manifest = []
                for name, blob in sorted(files.items()):
                    data = base64.b64decode(blob)
                    manifest.append([name, len(data), hashlib.sha256(data).hexdigest()])
                destination = t.get("workspace") or task
                if op == "handoff":
                    destination = args["recipient_spool"]
                    if destination not in t["work"]:
                        raise ValueError("Recipient outside task")
                pending = permissions.gate(db, task, op, [r["spool"], sorted(files), destination], [r["id"], manifest], args.get("request_id"), [op, r["spool"], destination])
                if pending:
                    return pending
                if op == "handoff":
                    key = "handoff-" + k.uid()
                    k.put(db, "handoff", key, dict(task=task, recipient=destination, source=r["id"],
                          revision=r["candidate"], stamp=permissions.stamp(db, task)))
                    return {"handoff": key}
                r.update(output=r.pop("candidate"), status="delivered")
                save(db, "run", r)
                return {"status": "delivered", "run": r["id"]}
            if op == "log":
                pending = permissions.gate(db, task, op, [r["id"], "agent"], {}, args.get("request_id"))
                if pending:
                    return pending
                path = k.ROOT / "runs" / args["run"] / "log"
                try:
                    with path.open("rb") as log:
                        log.seek(max(0, os.fstat(log.fileno()).st_size - 65536))
                        content = log.read(65536).decode(errors="replace")
                except FileNotFoundError:
                    content = ""
                if r.get("error"):
                    return f"{content}\nAn: {r['error']}".lstrip("\n")
                return content
            if "instance" not in r:
                return k.legacy_status(db, r, args["run"], op)
            i = k.get(db, "instance", r["instance"])
            if op == "cancel" and i["active"] == args["run"] and i["intent"] == "load":
                i["intent"] = "stop"
                r["status"] = "cancel_pending"
                k.put(db, "run", args["run"], r, True)
                save(db, "instance", i)
            reconcile(db, i, lock)
            result = k.owned(db, task, args["run"])
            return {key: result[key] for key in ("id", "status", "spool") if key in result}
        if args.get("instance"):
            i = k.get(db, "instance", args["instance"])
            if i["task"] != task or (args.get("spool") and args["spool"] != i["spool"]):
                raise ValueError("Instance outside requested task or spool")
        else:
            matches = [
                i
                for i in instances(db, task)
                if i["spool"] == args.get("spool") and i["status"] != "unloaded"
            ]
            if not matches:
                raise ValueError("Load the spool first")
            i = matches[-1]
        if op == "unload" and i["status"] != "unloaded":
            i["intent"] = "unload"
            if i["active"]:
                r = k.get(db, "run", i["active"])
                r["status"] = "cancel_pending"
                k.put(db, "run", r["id"], r, True)
            save(db, "instance", i)
        reconcile(db, i, lock)
        if op == "unload":
            return i
        if i.get("platform") != admission.require(db, i["spool"])["platform"]:
            raise ValueError("Unload and reload the instance on the admitted platform")
        return submit(db, i, args)
