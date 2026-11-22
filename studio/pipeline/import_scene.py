"""Import an OBJ asset, save scene.blend + parts.json. Runs inside the container.

blender --background --python import_scene.py -- <input.obj> <out_dir>
"""

import json
import pathlib
import sys

import bpy
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:]
src = pathlib.Path(argv[0])
out_dir = pathlib.Path(argv[1])
out_dir.mkdir(parents=True, exist_ok=True)

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.wm.obj_import(filepath=str(src))

scene = bpy.context.scene
parts = sorted(obj.name for obj in scene.objects if obj.type == "MESH")

meshes = [obj for obj in scene.objects if obj.type == "MESH"]
corners = [obj.matrix_world @ Vector(corner) for obj in meshes for corner in obj.bound_box]
if corners:
    lo = Vector(map(min, zip(*corners)))
    hi = Vector(map(max, zip(*corners)))
    center = (lo + hi) / 2
    radius = max((hi - lo).length / 2, 0.1)
else:
    center = Vector((0, 0, 0))
    radius = 1.0

camera_data = bpy.data.cameras.new("BridgeView")
camera = bpy.data.objects.new("BridgeView", camera_data)
scene.collection.objects.link(camera)
camera.location = center + Vector((1.0, -1.0, 0.7)).normalized() * (radius * 3.0)
camera.rotation_euler = (center - camera.location).to_track_quat("-Z", "Y").to_euler()
camera_data.clip_end = max(1000.0, radius * 100)
scene.camera = camera

bpy.ops.wm.save_as_mainfile(filepath=str(out_dir / "scene.blend"))
(out_dir / "parts.json").write_text(json.dumps({"parts": parts}))
print("import_scene: {} parts -> {}".format(len(parts), out_dir))
