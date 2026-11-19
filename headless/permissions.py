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
        return [json.loads(row[0]) for row in db.execute(
            "SELECT body FROM objects WHERE kind='request' AND json_extract(body,'$.turn')=? AND json_extract(body,'$.state')='pending'", (turn,)
        )]
    r = k.get(db, "request", request)
    if r["turn"] != turn or r["state"] != "pending" or decision not in ("once", "turn", "deny"):
        raise ValueError("Invalid or stale decision")
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


def gate(db, task, op, scope, details, request=None):
    if not db.in_transaction:
        db.execute("BEGIN IMMEDIATE")
    turn = current(db, task)
    key = digest([turn, task, op, scope])
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
          scope=scope, scope_key=key, details=details, state="pending"))
    return {"status": "permission_required", "request_id": request}
