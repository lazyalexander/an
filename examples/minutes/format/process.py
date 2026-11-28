import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--actions", action="store_true")
parser.add_argument("--provenance", action="store_true")
options = parser.parse_args()
source = Path("/input/summary/summary.json")
assert {
    p.relative_to(Path("/input")).as_posix()
    for p in Path("/input").rglob("*")
    if p.is_file()
} == {"summary/summary.json"}
rows = json.loads(source.read_bytes())
if options.actions:
    Path("/output/actions.md").write_text(
        "# Actions\n\n- Update the sign: " + rows[0] + "\n"
    )
else:
    Path("/output/announcement.md").write_text(
        "# Announcement\n\n" + "".join("- " + row + "\n" for row in rows)
    )

if options.provenance:
    artifact = "actions" if options.actions else "announcement"
    file = Path(
        "/output/actions.md" if options.actions else "/output/announcement.md"
    ).read_bytes()
    blocks = []
    for i, (name, source, block) in enumerate(
        [("opening", "opening", "conclusion"), ("registration", "registration", "rule")]
    ):
        if options.actions and i:
            break
        text = rows[i]
        start = file.index(text.encode())
        blocks.append(
            {
                "id": name,
                "text": text,
                "bytes": [start, start + len(text.encode())],
                "cites": [
                    {
                        "relation": "quotes",
                        "source": {"artifact": source, "version": 1, "block": block},
                    }
                ],
            }
        )
    Path("/output/blocks.json").write_text(json.dumps({artifact: blocks}))
