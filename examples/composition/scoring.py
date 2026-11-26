"""Evaluate the planner's actual bindings, independently of node IDs or submission order."""

from collections import Counter
from graphlib import CycleError, TopologicalSorter
from catalog import COMMAND, SPOOLS


def assess(plan, case, images):
    empty = {"selection": False, "wiring": False, "delivery": False}
    if plan.get("decision") == "unsupported":
        valid = not case["parents"] and plan.get("nodes") == []
        return dict.fromkeys(empty, valid)
    if plan.get("decision") != "plan" or not case["parents"]:
        return empty
    nodes = plan["nodes"]
    runs = [node for node in nodes if node["kind"] == "run"]
    outputs = [node for node in nodes if node["kind"] == "delivery"]
    names = {image: name for name, image in images.items()}
    selected = {node["id"]: names.get(node["spool"]) for node in runs}
    choice = Counter(selected.values()) == Counter(case["parents"].keys())
    if not choice or len(set(n["id"] for n in nodes)) != len(nodes):
        return {**empty, "selection": choice}
    ids = {name: node for node, name in selected.items()}

    def output(name):
        return {"node": ids[name], "port": SPOOLS[name]["output"]}

    dependencies = {
        node["id"]: set(node.get("after", []))
        | {r["node"] for r in node["inputs"].values() if "node" in r}
        for node in nodes
    }
    ordered = all(parents <= dependencies.keys() for parents in dependencies.values())
    try:
        tuple(TopologicalSorter(dependencies).static_order())
    except CycleError:
        ordered = False
    wired = ordered and len(nodes) == len(runs) + len(outputs)
    for node in runs:
        name = selected[node["id"]]
        parent = case["parents"][name]
        source = {"input": parent} if parent in case["inputs"] else output(parent)
        wired &= (
            node["inputs"] == {SPOOLS[name]["input"]: source}
            and node["outputs"] == [SPOOLS[name]["output"]]
            and node["argv"] == COMMAND
        )
    expected = [output(name) for name in case["deliver"]]
    delivered = [ref for node in outputs for ref in node["inputs"].values()]
    delivery = (
        len(delivered) == len(expected)
        and all(ref in delivered for ref in expected)
        and all(n["inputs"] and n["outputs"] == [] for n in outputs)
    )
    return {"selection": choice, "wiring": wired, "delivery": delivery}


def assess_safely(plan, case, images):
    try:
        return assess(plan, case, images)
    except (ValueError, KeyError, TypeError, AttributeError):
        return {"selection": False, "wiring": False, "delivery": False}
