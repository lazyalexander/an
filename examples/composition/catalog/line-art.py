from pathlib import Path
from xml.etree import ElementTree as ET


def transform(data):
    svg = ET.fromstring(data)
    for line in svg.iter("{http://www.w3.org/2000/svg}polyline"):
        line.set("stroke", "black")
        line.set("stroke-linejoin", "round")
        line.set("fill", "none")
    return ET.tostring(svg)


if __name__ == "__main__":
    (source,) = Path("/input/drawing").iterdir()
    Path("/output/lines.svg").write_bytes(transform(source.read_bytes()))
