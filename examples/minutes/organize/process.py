import json
from pathlib import Path

source = Path("/input/minutes/minutes.json")
target = Path("/output/summary.json")
rows = json.loads(source.read_text())["public_items"]
assert isinstance(rows, list) and all(isinstance(row, str) for row in rows)
target.write_text(json.dumps(rows))
