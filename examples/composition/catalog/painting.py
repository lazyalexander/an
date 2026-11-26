from pathlib import Path
from xml.etree import ElementTree as ET


def transform(data):
    svg = ET.fromstring(data)
    lines = list(svg.iter("{http://www.w3.org/2000/svg}polyline"))
    if not lines or any(line.get("stroke") != "black" for line in lines):
        raise ValueError("Flat colour requires inked lines")
    for line in lines:
        line.set("fill", "#db9258")
    return ET.tostring(svg)


if __name__ == "__main__":
    (source,) = Path("/input/lines").iterdir()
    Path("/output/painting.svg").write_bytes(transform(source.read_bytes()))
