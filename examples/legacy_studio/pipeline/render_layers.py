"""Per-part Cycles render of a scene at a pose. Runs inside the container.

blender --background --python render_layers.py -- \
    <scene.blend> <parts.json> <pose.json> <out_dir> <width> <height>
"""

import json
import pathlib
import sys

import bpy
from mathutils import Matrix

argv = sys.argv[sys.argv.index("--") + 1:]
blend = pathlib.Path(argv[0])
parts = json.loads(pathlib.Path(argv[1]).read_text())["parts"]
pose = json.loads(pathlib.Path(argv[2]).read_text())
out_dir = pathlib.Path(argv[3])
out_dir.mkdir(parents=True, exist_ok=True)
width, height = int(argv[4]), int(argv[5])

flat = pose["matrix"]
matrix = Matrix((flat[0:4], flat[4:8], flat[8:12], flat[12:16]))

bpy.ops.wm.open_mainfile(filepath=str(blend))
scene = bpy.context.scene

scene.render.engine = "CYCLES"
scene.cycles.device = "CPU"
scene.cycles.samples = 8
scene.render.film_transparent = True
scene.render.resolution_x = width
scene.render.resolution_y = height
scene.render.image_settings.file_format = "PNG"
scene.render.image_settings.color_mode = "RGBA"

camera_data = bpy.data.cameras.new("BridgeCamera")
camera = bpy.data.objects.new("BridgeCamera", camera_data)
scene.collection.objects.link(camera)
camera.matrix_world = matrix
scene.camera = camera

world = bpy.data.worlds.new("BridgeWorld")
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (1.0, 1.0, 1.0, 1.0)
world.node_tree.nodes["Background"].inputs[1].default_value = 1.0
scene.world = world

mesh_objects = [obj for obj in scene.objects if obj.type == "MESH"]
for part in parts:
    for obj in mesh_objects:
        obj.hide_render = obj.name != part
    scene.render.filepath = str(out_dir / "{}.png".format(part))
    bpy.ops.render.render(write_still=True)
    print("render_layers: {} -> {}".format(part, scene.render.filepath))
