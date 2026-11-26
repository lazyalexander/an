"""Turn sketch strokes into a clean SVG line drawing.

A deterministic finisher: each stroke becomes one filled outline whose width
follows the recorded pressure, smoothed with a moving average. It is the first
spool after a sketch surface; a model-backed finisher can replace it with the
same input and output.
"""
import json
import math
from pathlib import Path

WIDTH, HEIGHT = 800, 600


def smooth(points, radius=2):
    out = []
    for i in range(len(points)):
        window = points[max(0, i - radius): i + radius + 1]
        out.append([sum(p[k] for p in window) / len(window) for k in range(3)])
    return out


def outline(stroke):
    pts = smooth([[x * WIDTH, y * HEIGHT, p] for x, y, p in stroke["points"]])
    if len(pts) == 1:
        x, y, p = pts[0]
        r = max(0.5, stroke["size"] * p / 2)
        return f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{r:.2f}" fill="{stroke["color"]}"/>'
    left, right = [], []
    for i, (x, y, p) in enumerate(pts):
        a, b = pts[max(0, i - 1)], pts[min(len(pts) - 1, i + 1)]
        dx, dy = b[0] - a[0], b[1] - a[1]
        length = math.hypot(dx, dy) or 1
        r = max(0.4, stroke["size"] * (p * p * (3 - 2 * p)) / 2)
        nx, ny = -dy / length * r, dx / length * r
        left.append(f"{x + nx:.1f},{y + ny:.1f}")
        right.append(f"{x - nx:.1f},{y - ny:.1f}")
    path = "M" + " L".join(left + right[::-1]) + " Z"
    return f'<path d="{path}" fill="{stroke["color"]}" stroke="{stroke["color"]}" stroke-width="0.6" stroke-linejoin="round"/>'


response = json.loads(Path("/input/choice/response.json").read_text())
body = "\n".join(outline(s) for s in response["strokes"] if s["points"])
Path("/output/finished.svg").write_text(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}">'
    f'<rect width="{WIDTH}" height="{HEIGHT}" fill="#fff"/>\n{body}\n</svg>\n')
