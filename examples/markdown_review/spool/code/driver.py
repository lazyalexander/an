"""Container entry points for the Markdown review workflow."""
import difflib
import json
import re
from pathlib import Path
import sys

INPUT = Path("/input")
OUTPUT = Path("/output")
DOC = {"input": "original.md"}
REVIEWS = {"input": "reviews.json"}


def ref(node, port):
    return {"node": node, "port": port}


def run(stage, spool, inputs, outputs, kind="run"):
    return {"id": stage, "kind": kind, "spool": spool, "inputs": inputs, "outputs": outputs,
            "argv": ["python3", "/spool/code/driver.py", stage, spool]}


def suggestions():
    original = (INPUT / "source/original.md").read_bytes().decode("utf-8")
    edits = json.loads((INPUT / "reviews/reviews.json").read_text())
    if len(edits) != 2:
        raise ValueError("Exactly two review suggestions required")
    for edit in edits:
        if not all(isinstance(edit[k], str) and edit[k].strip() for k in ("before", "after", "reason")):
            raise ValueError("Incomplete review suggestion")
        if original.replace("\r\n", "\n").rstrip("\n").split("\n\n").count(edit["before"]) != 1:
            raise ValueError("Each review must target one unique paragraph")
    return original, edits


def execute(stage, spool):
    sources = {"source": DOC, "reviews": REVIEWS}
    if stage == "markdown":
        return [run("prompt", spool, sources, ["request.json", "A.txt", "B.txt"]),
                {"id":"analysis", "kind":"model", "model":"local",
                 "inputs":{"request":ref("prompt","request.json")}, "outputs":["result.json"]},
                run("route", spool, {**sources, "analysis":ref("analysis","result.json")}, ["subgraph.json"], "expand")]
    original, edits = suggestions()
    if stage == "prompt":
        system = ('Decide whether both proposed paragraph replacements can be applied together. '
                  'Check both the meaning of the edits and their target paragraphs. Mutually exclusive policies or different replacements of one paragraph conflict. '
                  'Return only JSON with exactly one field "conflict" (boolean).')
        messages = [{"role":"system","content":system},
                    {"role":"user","content":json.dumps({"original":original,"edits":edits},ensure_ascii=False)}]
        (OUTPUT / "request.json").write_text(json.dumps({"messages":messages}))
        for key, edit in zip("AB", edits):
            (OUTPUT / f"{key}.txt").write_text(edit["before"] + "\n→ " + edit["after"] + "\n" + edit["reason"])
    elif stage == "route":
        raw = json.loads((INPUT / "analysis/result.json").read_text())["text"]
        decision = json.loads(raw)
        if not isinstance(decision, dict) or set(decision) != {"conflict"} or not isinstance(decision["conflict"], bool):
            raise ValueError("Invalid or inconsistent model review decision")
        conflict = decision["conflict"]
        if not conflict and edits[0]["before"] == edits[1]["before"] and edits[0]["after"] != edits[1]["after"]:
            raise ValueError("Model selected two competing replacements for the same paragraph")
        nodes = []
        if conflict:
            nodes.append({"id":"resolve","kind":"surface","spool":spool,"panel":"resolve",
                          "title":"选择本次采用的审阅意见","inputs":{k:ref("prompt",k+".txt") for k in "AB"},
                          "outputs":["response"]})
            sources["choice"] = ref("resolve","response")
        return nodes + [run("revise", spool, sources, ["revised.md","changes.diff"]),
                        {"id":"delivery","kind":"delivery","title":"修订稿与变更记录","inputs":{
                            "document":ref("revise","revised.md"),"diff":ref("revise","changes.diff")},"outputs":[]}]
    elif stage == "revise":
        answer = INPUT / "choice/response.json"
        if answer.exists():
            selected = json.loads(answer.read_text())["choice"]
            edits = [edits[{"A":0,"B":1}[selected]]]
        replacements = {e["before"]:e["after"] for e in edits}
        body = original.rstrip("\r\n")
        parts = re.split(r"((?:\r?\n){2,})", body)
        for index in range(0, len(parts), 2):
            paragraph = parts[index]
            replacement = replacements.get(paragraph.replace("\r\n", "\n"))
            if replacement is not None:
                context = paragraph
                if "\n" not in context:
                    context = parts[index + 1] if index + 1 < len(parts) else original[len(body):] or original
                ending = "\r\n" if "\r\n" in context else "\n"
                parts[index] = replacement.replace("\r\n", "\n").replace("\n", ending)
        revised = "".join(parts) + original[len(body):]
        (OUTPUT/"revised.md").write_bytes(revised.encode("utf-8"))
        diff = difflib.unified_diff(original.splitlines(True), revised.splitlines(True),
                                    fromfile="original.md", tofile="revised.md")
        with (OUTPUT/"changes.diff").open("w") as patch:
            for line in diff:
                patch.write(line)
                if not line.endswith("\n"):
                    patch.write("\n\\ No newline at end of file\n")
    else:
        raise ValueError("Unknown review stage")


if __name__ == "__main__":
    nodes = execute(*sys.argv[1:])
    if nodes is not None:
        (OUTPUT / "subgraph.json").write_text(json.dumps({"nodes":nodes}))
