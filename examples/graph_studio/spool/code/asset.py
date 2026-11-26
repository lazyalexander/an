"""Fixed spool entry points for the graph studio fixtures."""
import json
import shutil
import sys
import time
from pathlib import Path
from xml.etree import ElementTree as ET

operation = sys.argv[1]
output = Path("/output")
if operation in ("A", "B"):
    if sys.argv[2:] == ["--verify-overlap"]:
        time.sleep(5)
    color, x = {"A": ("#75988b", "40"), "B": ("#b78050", "130")}[operation]
    svg = ET.Element("svg", xmlns="http://www.w3.org/2000/svg", viewBox="0 0 220 110")
    ET.SubElement(svg, "rect", width="220", height="110", fill=color)
    ET.SubElement(svg, "circle", cx=x, cy="49", r="31", fill="#f2dbac")
    (output / "candidate.svg").write_bytes(ET.tostring(svg))
elif operation == "plate":
    (output / "plate.json").write_text(json.dumps(dict(width=76, depth=45, height=12, chamfer=8)))
elif operation == "preview":
    shutil.copyfile("/input/source/selected/selected.svg", output / "selected.svg")
elif operation == "note":
    choice = json.loads(Path("/input/choice/response.json").read_text())
    (output / "annotation.txt").write_text(choice["note"])
else:
    raise ValueError(f"Unknown fixture operation: {operation}")
