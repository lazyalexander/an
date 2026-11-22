"""Pure pose math for the Blender addon — no bpy imports (testable)."""


def matrix_to_pose(m):
    """Flatten a 4x4 row-major matrix into the /api/pose payload shape."""
    rows = [list(row) for row in m]
    if len(rows) != 4 or any(len(row) != 4 for row in rows):
        raise ValueError("matrix must be 4x4")
    flat = [float(value) for row in rows for value in row]
    location = [float(rows[0][3]), float(rows[1][3]), float(rows[2][3])]
    return {"matrix": flat, "location": location}
