"""Turn-scoped boundary decisions. Control commands belong to the trusted host."""
import hashlib
import json
import os

import kernel as k


def control(db, task, action, request, decision):
    db.execute("BEGIN IMMEDIATE")
    k.get(db, "task", task)
    if action == "begin":
        turn = k.uid()
        k.put(db, "turn", task, turn, True)
        return {"turn": turn}
    turn = current(db, task)
    if action == "end":
        k.put(db, "turn", task, None, True)
        return {"closed": True}
    if action == "pending":
        revisions = stamp(db, task)
        records = (json.loads(row[0]) for row in db.execute(
            "SELECT body FROM objects WHERE kind='request' AND json_extract(body,'$.turn')=? AND json_extract(body,'$.state')='pending'", (turn,)
        ))
        return [record for record in records if record.get("revisions") == revisions]
    r = k.get(db, "request", request)
    if r["turn"] != turn or r["task"] != task or r["state"] != "pending":
        raise ValueError("Invalid or stale decision")
    if r.get("revisions") != stamp(db, task):
        raise ValueError("Policy changed")
    if action == "effect":
        _, _, host = context(db, task)
        effect = {field: r[field] for field in ("operation", "scope", "details")}
        key = effect_key(effect)
        host["effects"] = [item for item in host["effects"] if effect_key(item) != key]
        host["effects"].append({**effect, "decision": decision})
        return {"revision": set_host(db, task, host)}
    if action != "decide" or decision not in ("once", "turn", "deny"):
        raise ValueError("Invalid user decision")
    r["state"] = decision
    k.put(db, "request", request, r, True)
    if decision == "turn":
        k.put(db, "grant", r["scope_key"], True, True)
    return {"request_id": request, "decision": decision}


def current(db, task):
    turn = os.environ.get("AN_TURN")
    if not turn or k.get(db, "turn", task) != turn:
        raise ValueError("Active host turn required")
    return turn


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def gate(db, task, op, scope, details, request=None, flow=None):
    if not db.in_transaction:
        db.execute("BEGIN IMMEDIATE")
    turn, flows, host = context(db, task)
    revisions = [turn, flows["revision"], host["revision"]]
    if flow and "flows" in flows and flow not in flows["flows"]:
        raise ValueError("Denied by workspace flow policy")
    decision = host["default"]
    effect_key = digest([op, scope, details])
    for effect in host["effects"]:
        if effect_key == digest([effect["operation"], effect["scope"], effect["details"]]):
            decision = effect["decision"]
            break
    if decision in ("deny", "unavailable"):
        raise ValueError(f"Host policy: {decision}")
    key = digest([revisions, task, op, scope])
    if decision == "allow" and not request:
        k.put(db, "effect", k.uid(), dict(task=task, operation=op, scope=scope, details=details, revisions=revisions, authority="host"))
        return None
    if request:
        r = k.get(db, "request", request)
        if r["scope_key"] != key or r["details"] != details:
            raise ValueError("Request changed")
        if r["state"] == "pending":
            return {"status": "permission_required", "request_id": request}
        if r["state"] not in ("turn", "once"):
            raise ValueError("Request denied or already used")
        r["state"] = "used"
        k.put(db, "request", request, r, True)
        return None
    if db.execute("SELECT 1 FROM objects WHERE kind='grant' AND id=?", (key,)).fetchone():
        return None
    request = k.uid()
    k.put(db, "request", request, dict(id=request, task=task, turn=turn, operation=op,
          revisions=revisions, scope=scope, scope_key=key, details=details, state="pending"))
    return {"status": "permission_required", "request_id": request}


_SHOW = object()


def policy(db, workspace, rules=_SHOW):
    config = k.get(db, "workspace", workspace)
    if rules is not _SHOW:
        images = config["images"]
        if not isinstance(rules, list):
            raise ValueError("Expected flow list")
        for op, source, dest in rules:
            valid = (
                (op == "input" and isinstance(source, str) and dest in images)
                or (op == "handoff" and source in images and dest in images)
                or (op == "publish" and source in images and dest == workspace)
                or (op == "export" and source in images and isinstance(dest, str) and os.path.isabs(dest))
            )
            if not valid:
                raise ValueError("Invalid workspace flow")
        config["policy"] = {"revision": k.uid(), "flows": rules}
        k.put(db, "workspace", workspace, config, True)
    return config.get("policy", {"revision": "legacy"})


def context(db, task):
    turn = current(db, task)
    workspace = k.get(db, "task", task).get("workspace")
    flows = policy(db, workspace) if workspace else {"revision": "legacy"}
    row = db.execute("SELECT body FROM objects WHERE kind='host_policy' AND id=?", (turn,)).fetchone()
    host = json.loads(row[0]) if row else {"revision": "legacy", "default": "ask", "effects": []}
    return turn, flows, host


def effect_key(effect):
    return digest([effect["operation"], effect["scope"], effect["details"]])


def set_host(db, task, config):
    if not db.in_transaction:
        db.execute("BEGIN IMMEDIATE")
    turn = current(db, task)
    choices = ("allow", "deny", "ask", "unavailable")
    if not isinstance(config.get("effects"), list):
        raise ValueError("Expected host effects list")
    if config["default"] not in ("deny", "ask", "unavailable"):
        raise ValueError("Host default must not grant unscoped access")
    seen = set()
    for effect in config["effects"]:
        key = effect_key(effect)
        if effect["decision"] not in choices or key in seen:
            raise ValueError("Invalid or duplicate host decision")
        seen.add(key)
    config["revision"] = k.uid()
    k.put(db, "host_policy", turn, config, True)
    return config["revision"]


def stamp(db, task):
    turn, flows, host = context(db, task)
    return [turn, flows["revision"], host["revision"]]
