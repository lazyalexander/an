import json
from pathlib import Path

source = Path("/input/summary/summary.json")
target = Path("/output/announcement.md")
assert {
    p.relative_to(Path("/input")).as_posix()
    for p in Path("/input").rglob("*")
    if p.is_file()
} == {"summary/summary.json"}
target.write_text(
    "# Announcement\n\n"
    + "\n".join("- " + row for row in json.loads(source.read_text()))
    + "\n"
)
