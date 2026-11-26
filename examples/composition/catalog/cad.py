from pathlib import Path
from xml.etree import ElementTree as ET


def transform(data):
    svg = ET.Element("svg", xmlns="http://www.w3.org/2000/svg", viewBox="0 0 100 100")
    vertices = []
    for line in data.decode().splitlines():
        fields = line.split()
        if not fields:
            continue
        kind, *values = fields
        if kind == "v":
            vertices.append(",".join(values[:2]))
        elif kind == "l":
            points = " ".join(vertices[int(i) - 1] for i in values)
            ET.SubElement(svg, "polyline", points=points, stroke="gray", fill="none")
    return ET.tostring(svg)


if __name__ == "__main__":
    (source,) = Path("/input/mesh").iterdir()
    Path("/output/projection.svg").write_bytes(transform(source.read_bytes()))
