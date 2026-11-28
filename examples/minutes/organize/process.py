import sys
import json
from pathlib import Path

source = Path("/input/minutes/minutes.json")
target = Path("/output/summary.json")
rows = json.loads(source.read_text())["public_items"]
assert isinstance(rows, list) and all(isinstance(row, str) for row in rows)
target.write_text(json.dumps(rows))
print(f"Selected {len(rows)} public items", flush=True)

# Only the provenance workflow requests these declared outputs.

if "--provenance" in sys.argv:
    declarations = {}
    for artifact, block, source_block, text in (
        ("opening", "conclusion", "opening", rows[0]),
        ("registration", "rule", "registration", rows[1]),
    ):
        start = target.read_bytes().index(text.encode())
        declarations[artifact] = [
            {
                "id": block,
                "text": text,
                "bytes": [start, start + len(text.encode())],
                "cites": [
                    {
                        "relation": "quotes",
                        "source": {
                            "artifact": "minutes",
                            "version": 1,
                            "block": source_block,
                        },
                    }
                ],
            }
        ]
    Path("/output/blocks.json").write_text(json.dumps(declarations))
