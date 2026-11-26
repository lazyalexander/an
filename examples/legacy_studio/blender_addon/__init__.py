"""Bridge Studio pose-stream addon for Blender.

Streams the scene camera's world matrix to the orchestrator 10x/s.
Every orchestrator call fails soft (Review Focus 1): OSError/ValueError
are swallowed and online<->offline transitions print once to the console.

Install (reference machine): symlink this package into Blender's addons
directory so `from .pose import ...` resolves:
  ln -sfn "$PWD/examples/legacy_studio/blender_addon" \
    "$HOME/Library/Application Support/Blender/5.2/scripts/addons/blender_addon"
then enable "Bridge Studio Pose Stream" in Edit > Preferences > Add-ons.
"""

bl_info = {
    "name": "Bridge Studio Pose Stream",
    "blender": (4, 0, 0),
    "category": "System",
}

import json
import pathlib
import urllib.request

try:
    import bpy
except ModuleNotFoundError:  # imported outside Blender (test collection)
    bpy = None

from .pose import matrix_to_pose

BASE = "http://127.0.0.1:8791"
TIMEOUT_S = 1
INTERVAL_S = 0.1

_last_ok = None


def _ui_token():
    """Per-boot token written next to the studio package, if present."""
    try:
        path = pathlib.Path(__file__).resolve().parents[1] / "run" / "ui_token"
        return path.read_text().strip() or None
    except (OSError, ValueError):
        return None


def _note(ok):
    global _last_ok
    if ok != _last_ok:
        print("bridge_studio: orchestrator {}".format("online" if ok else "offline"))
        _last_ok = ok


def _tick():
    scene = bpy.context.scene
    camera = scene.camera if scene is not None else None
    if camera is not None:
        try:
            pose = matrix_to_pose(list(map(list, camera.matrix_world)))
            headers = {"Content-Type": "application/json"}
            token = _ui_token()
            if token is not None:
                headers["X-Bridge-Key"] = token
            request = urllib.request.Request(
                BASE + "/api/input/pose", data=json.dumps(pose).encode(),
                headers=headers)
            with urllib.request.urlopen(request, timeout=TIMEOUT_S):
                pass
        except (OSError, ValueError):
            _note(False)
        else:
            _note(True)
    return INTERVAL_S


def register():
    bpy.app.timers.register(_tick)


def unregister():
    if bpy.app.timers.is_registered(_tick):
        bpy.app.timers.unregister(_tick)
