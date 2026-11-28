"""Release the two formatting jobs after An accepts the opening decision."""

import json
from pathlib import Path
import sys

formatter = sys.argv[1]
nodes = []
for name, output, options in (
    ("format", "announcement.md", []),
    ("actions", "actions.md", ["--actions"]),
):
    nodes.append(
        {
            "id": name,
            "kind": "run",
            "spool": formatter,
            "argv": ["python3", "/spool/code/process.py", "--provenance", *options],
            "inputs": {"summary": {"node": "organize", "port": "summary.json"}},
            "outputs": [output, "blocks.json"],
        }
    )
Path("/output/subgraph.json").write_text(json.dumps({"nodes": nodes}))
