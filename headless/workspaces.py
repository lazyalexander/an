"""Operator-owned workspace configuration and session bindings."""

import json
import hashlib
import fcntl
from contextlib import contextmanager
import kernel as k


def selector(parser):
    parser.add_argument("--harness")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--session")
    group.add_argument("--key")


def identity(args):
    if not args.harness or not (args.session or args.key):
        raise ValueError("Expected --harness with --session or --key")
    return json.dumps(
        [args.harness, "session" if args.session else "key", args.session or args.key]
    )


def create(db, args):
    if not set(args.editable) <= set(args.spool):
        raise ValueError("Editable image must belong to workspace")
    images = [k.get(db, "image", ref) for ref in args.spool]
    tools = {}
    if args.allow_tool:
        from gateway import grant

        tools = grant(db, args.allow_tool)
    wid = k.uid()
    k.put(
        db, "workspace", wid, dict(images=images, editable=args.editable, tools=tools)
    )
    return wid


def bind(db, args):
    key = identity(args)
    k.get(db, "workspace", args.workspace)
    row = db.execute(
        "SELECT body FROM objects WHERE kind='binding' AND id=?", (key,)
    ).fetchone()
    if row:
        if json.loads(row[0])["workspace"] != args.workspace:
            raise ValueError("Identity already bound to another workspace")
    else:
        k.put(db, "binding", key, dict(workspace=args.workspace, task=None))
    return args.workspace


@contextmanager
def binding_guard(key, *, rotate=False, task=None):
    if key is None:
        yield
        return
    directory = k.ROOT / "locks"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / ("binding-" + hashlib.sha256(key.encode()).hexdigest())
    with path.open("ab") as lock:
        try:
            fcntl.flock(
                lock, (fcntl.LOCK_EX if rotate else fcntl.LOCK_SH) | fcntl.LOCK_NB
            )
        except BlockingIOError:
            raise ValueError("Binding busy; retry after current dispatch") from None
        if task is not None:
            with k.connect() as db:
                if k.get(db, "binding", key)["task"] != task:
                    raise ValueError("Task binding changed; reconnect the MCP session")
        yield


def resolve(args, fresh=False):
    key = identity(args)
    with binding_guard(key, rotate=fresh), k.connect() as db:
        db.execute("BEGIN IMMEDIATE")
        binding = k.get(db, "binding", key)
        old = binding["task"]
        if old and not fresh:
            return old
        if old:
            active = [
                r[0]
                for r in db.execute(
                    """SELECT id FROM objects WHERE kind='instance'
            AND json_extract(body,'$.task')=? AND json_extract(body,'$.status')!='unloaded'""",
                    (old,),
                )
            ]
            if active:
                raise ValueError(f"Unload instances first: {active}")
        wid = binding["workspace"]
        cfg = k.get(db, "workspace", wid)
        task = k.task_new(db, cfg["images"], cfg["editable"], {})
        t = k.get(db, "task", task)
        t.update(workspace=wid, tools=cfg["tools"])
        k.put(db, "task", task, t, True)
        binding["task"] = task
        k.put(db, "binding", key, binding, True)
        return task


def upstream(db, task, source):
    run = k.get(db, "run", source)
    if run["task"] != task:
        wid = k.get(db, "task", task).get("workspace")
        if (
            not wid
            or not run.get("outputs")
            or k.get(db, "task", run["task"]).get("workspace") != wid
        ):
            raise ValueError("Delivery outside workspace")
    if run["status"] != "delivered":
        raise ValueError("Input has not been delivered")
    return run["output"]


def deliveries(db, task, after):
    wid = k.get(db, "task", task).get("workspace")
    if not wid:
        raise ValueError("Task has no workspace")
    rows = db.execute(
        """SELECT r.rowid,r.id,json_extract(r.body,'$.spool'),json_extract(r.body,'$.outputs'),json_extract(r.body,'$.output')
        FROM objects r JOIN objects t ON t.kind='task' AND t.id=json_extract(r.body,'$.task')
        WHERE r.kind='run' AND json_extract(t.body,'$.workspace')=? AND json_extract(r.body,'$.status')='delivered'
        AND json_array_length(r.body,'$.outputs')>0 AND r.rowid>? ORDER BY r.rowid LIMIT 51""",
        (wid, after),
    ).fetchall()
    return dict(
        items=[
            dict(id=id, spool=s, files=json.loads(f), revision=v)
            for _, id, s, f, v in rows[:50]
        ],
        next=rows[49][0] if len(rows) > 50 else None,
    )
