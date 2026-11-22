"""Import an OBJ asset, save scene.blend + parts.json. Runs inside the container.

blender --background --python import_scene.py -- <input.obj> <out_dir>
"""

import json
import pathlib
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1:]
src = pathlib.Path(argv[0])
out_dir = pathlib.Path(argv[1])
out_dir.mkdir(parents=True, exist_ok=True)

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.wm.obj_import(filepath=str(src))

parts = sorted(obj.name for obj in bpy.context.scene.objects if obj.type == "MESH")
bpy.ops.wm.save_as_mainfile(filepath=str(out_dir / "scene.blend"))
(out_dir / "parts.json").write_text(json.dumps({"parts": parts}))
print("import_scene: {} parts -> {}".format(len(parts), out_dir))
