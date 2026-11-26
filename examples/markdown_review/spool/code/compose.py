"""Compose a Markdown document from headings, prose and bound UTF-8 files."""

import json
from pathlib import Path
import sys

root = Path("/input")
sections = json.loads(sys.argv[2])
name = sys.argv[1]
if Path(name).name != name or not name.endswith(".md"):
    raise ValueError("Expected a Markdown output filename")
parts = []
for section in sections:
    if isinstance(section, str):
        parts.append(section)
    elif isinstance(section, dict) and set(section) == {"file"}:
        path = (root / section["file"]).resolve()
        if not path.is_relative_to(root):
            raise ValueError("Expected a bound input file")
        parts.append(path.read_text(encoding="utf-8").rstrip("\n"))
    else:
        raise ValueError("Expected Markdown text or a file reference")
(Path("/output") / name).write_text("\n\n".join(parts) + "\n", encoding="utf-8")
