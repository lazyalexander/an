"""Extrude the dimensioned plate, export an editable mesh and a projected turntable."""
import json
import math
import struct
from pathlib import Path
from xml.etree import ElementTree as ET

outline = json.loads(Path("/input/source/plate.json").read_text())
period = json.loads(Path("/input/choice/response.json").read_text())["seconds"]
width, depth, height, cut = (outline[key] for key in ("width", "depth", "height", "chamfer"))
ring = [(-width/2, -depth/2), (width/2-cut, -depth/2), (width/2, -depth/2+cut),
        (width/2, depth/2), (-width/2, depth/2)]
n = len(ring)
vertices = [(x, y, z) for z in (0, height) for x, y in ring]
faces = [list(reversed(range(n))), list(range(n, 2*n))]
faces.extend([[i, (i+1) % n, (i+1) % n+n, i+n] for i in range(n)])

triangles = []
for face in faces:
    for i in range(1, len(face)-1):
        a, b, c = (vertices[j] for j in (face[0], face[i], face[i+1]))
        u, v = [b[k]-a[k] for k in range(3)], [c[k]-a[k] for k in range(3)]
        normal = [u[(k+1)%3]*v[(k+2)%3] - u[(k+2)%3]*v[(k+1)%3] for k in range(3)]
        length = math.sqrt(sum(k*k for k in normal))
        triangles.append(struct.pack("<12fH", *(k/length for k in normal), *a, *b, *c, 0))
Path("/output/plate.stl").write_bytes(b"An Studio plate".ljust(80,b"\0")
                                      + struct.pack("<I", len(triangles)) + b"".join(triangles))

svg = ET.Element("svg", xmlns="http://www.w3.org/2000/svg", viewBox="-70 -50 140 100")
frames = 48
for frame in range(frames):
    angle = frame * math.tau / frames
    projected = []
    for x, y, z in vertices:
        rx, ry = x*math.cos(angle)-y*math.sin(angle), x*math.sin(angle)+y*math.cos(angle)
        projected.append((rx, .5*ry-.866*z, .866*ry+.5*z))
    group = ET.SubElement(svg, "g", opacity="1" if frame == 0 else "0")
    opacity = ["1" if i == frame else "0" for i in range(frames)]
    ET.SubElement(group, "animate", attributeName="opacity", calcMode="discrete",
                  values=";".join(opacity), dur=f"{period}s", repeatCount="indefinite")
    for index in sorted(range(len(faces)), key=lambda i: sum(projected[j][2] for j in faces[i])/len(faces[i])):
        points = " ".join("%.2f,%.2f" % projected[j][:2] for j in faces[index])
        ET.SubElement(group, "polygon", points=points, fill="#a5c5b5" if index==1 else "#71a493",
                      stroke="#183d34", **{"stroke-width":"0.7"})
Path("/output/turntable.svg").write_bytes(ET.tostring(svg))
