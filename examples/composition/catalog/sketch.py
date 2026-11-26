import json
from pathlib import Path
from xml.etree import ElementTree as ET


def transform(data):
    svg = ET.Element("svg", xmlns="http://www.w3.org/2000/svg", viewBox="0 0 100 100")
    for stroke in json.loads(data)["strokes"]:
        points = " ".join(f"{x},{y}" for x, y in stroke)
        ET.SubElement(svg, "polyline", points=points, stroke="gray", fill="none")
    return ET.tostring(svg)


if __name__ == "__main__":
    (source,) = Path("/input/strokes").iterdir()
    Path("/output/sketch.svg").write_bytes(transform(source.read_bytes()))
