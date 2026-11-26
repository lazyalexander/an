"""Acceptance oracle for the public mid-review instruction."""

import base64
import hashlib
import json


def check(graph, state, routes, handled, mode, fixtures, job):
    assert len(handled) == 1
    cursor = handled[0]
    submissions = [e for e in state["events"] if e["kind"] == "submitted"]
    linked = [e for e in submissions if e.get("instruction_cursor") == cursor]
    assert linked and any(e["cursor"] < cursor for e in submissions)
    surface = next(k for k, n in state["nodes"].items() if n["kind"] == "surface")
    assert state["activity"][surface]["status"] == "superseded"
    assert (
        len(state["model_calls"]) == 1
        and state["model_calls"][0]["state"] == "completed"
    )
    disconnect = json.loads((job / "disconnect.json").read_text())
    assert disconnect["thread"] == (job / "thread").read_text()
    assert disconnect["calls"] == [state["model_calls"][0]["attempt"]]
    assert (
        any(e["kind"] == "superseded" for e in state["events"])
        or surface in linked[0]["supersede"]
    )
    browser = json.loads((job / "browser.json").read_text())
    decisions = browser["confirmations"]
    assert decisions and len(decisions) == len({r["request"] for r in decisions})
    assert sum(r["decision"] == "deny" for r in decisions) == (mode == "deny")
    deliveries = graph("deliveries")
    if mode == "deny":
        assert not deliveries
        assert any(
            state["nodes"][k]["kind"] == "delivery"
            and "denied" in v.get("error", "").lower()
            for k, v in state["activity"].items()
            if v["status"] == "failed"
        )
    else:
        assert len(deliveries) == 1
        delivery = deliveries[0]
        opened = graph("open", delivery=delivery["id"])["files"]
        files = {}
        for binding in opened.values():
            for name, entry in binding.items():
                assert name not in files
                files[name] = base64.b64decode(
                    entry if isinstance(entry, str) else entry["data"]
                )
        assert browser["files"] == {
            k: hashlib.sha256(v).hexdigest() for k, v in files.items()
        }
        assert files.pop("original.md") == (fixtures / "original.md").read_bytes()
        assert len(files) == 1
        note = next(iter(files.values())).decode()
        for edit in json.loads((fixtures / "conflict/reviews.json").read_text()):
            assert edit["after"] in note and edit["reason"] in note, note
        assert any(
            s in note
            for s in (
                "未修改",
                "保留原文",
                "保持原文",
                "未改",
                "不变",
                "完整保留",
                "没有应用任何修改",
                "未采用",
                "unchanged",
            )
        ), note
        trace = graph("sources", delivery=delivery["id"])
        assert any(
            e.get("instruction_cursor") == cursor
            for n in trace.values()
            for e in n["events"]
        )
    return {
        "outcome": mode,
        "instruction_cursor": cursor,
        "initial_routes": len(routes),
        "new_flow_confirmations": len(decisions),
        "reused_model_calls": 1,
        "new_model_calls": 0,
        "local_model_calls": len(state["model_calls"]),
        "deliveries": len(deliveries),
    }
