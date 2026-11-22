"""Bridge docker — the only Bridge Studio UI inside Krita.

Stdlib + krita + PyQt5 only (Krita 5.3.4 bundles Python 3.10: keep this
file 3.10-compatible). Every orchestrator call fails soft into the
offline state; nothing raises into Krita.
"""

import json
import pathlib
import sys
import urllib.error
import urllib.request

from krita import DockWidget, Krita
from PyQt5.QtCore import QTimer
from PyQt5.QtGui import QImage
from PyQt5.QtWidgets import QLabel, QPushButton, QVBoxLayout, QWidget

DOCKER_ID = "bridge_studio_docker"
BASE = "http://127.0.0.1:8791"
TIMEOUT_S = 1


def _studio_root():
    root = pathlib.Path(__file__).resolve().parents[3]
    if (root / "adapter" / "core" / "push.py").is_file():
        return root
    return None


_STUDIO = _studio_root()
if _STUDIO is not None and str(_STUDIO) not in sys.path:
    sys.path.insert(0, str(_STUDIO))


def _get(path):
    with urllib.request.urlopen(BASE + path, timeout=TIMEOUT_S) as response:
        return json.loads(response.read())


def _post(path, payload):
    request = urllib.request.Request(
        BASE + path, data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
        return json.loads(response.read())


class RealDocument:
    """Kis Document implementation over a live Krita document."""

    def __init__(self, doc):
        self._doc = doc
        root = doc.rootNode()
        for child in list(root.childNodes()):
            child.remove()

    def group(self, name, parent=None):
        node = self._doc.createNode(name, "grouplayer")
        (parent or self._doc.rootNode()).addChildNode(node, None)
        return node

    def layer_from_png(self, name, parent, png_path):
        from adapter.core.pixels import rgba8888_to_krita_u8
        layer = self._doc.createNode(name, "paintlayer")
        (parent or self._doc.rootNode()).addChildNode(layer, None)
        image = QImage(str(png_path)).convertToFormat(QImage.Format_RGBA8888)
        ptr = image.bits()
        ptr.setsize(image.byteCount())
        layer.setPixelData(
            rgba8888_to_krita_u8(ptr.asstring(image.byteCount())), 0, 0,
            image.width(), image.height())
        return layer

    def set_locked(self, node, locked):
        node.setLocked(locked)

    def save(self, path):
        self._doc.saveAs(str(path))


class RealKis:
    """Kis implementation wrapping the running Krita instance."""

    def __init__(self):
        self._app = Krita.instance()

    def create_document(self, width, height, name):
        doc = self._app.createDocument(width, height, name, "RGBA", "U8", "", 300.0)
        window = self._app.activeWindow()
        if window is not None:
            window.addView(doc)
        return RealDocument(doc)


class BridgeDocker(DockWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Bridge")
        self._status_label = QLabel("orchestrator offline")
        self._lock_button = QPushButton("Lock")
        self._unlock_button = QPushButton("Unlock")
        self._lock_button.clicked.connect(self._on_lock)
        self._unlock_button.clicked.connect(self._on_unlock)
        layout = QVBoxLayout()
        layout.addWidget(self._status_label)
        layout.addWidget(self._lock_button)
        layout.addWidget(self._unlock_button)
        widget = QWidget()
        widget.setLayout(layout)
        self.setWidget(widget)
        self._pushed = set()
        self._broken = _STUDIO is None
        if self._broken:
            self._status_label.setText("install broken: not a symlink into the repo")
        self._timer = QTimer(self)
        self._timer.timeout.connect(self._tick)
        self._timer.start(500)
        self._tick()

    def canvasChanged(self, canvas):
        pass

    def _set_offline(self):
        self._status_label.setText("orchestrator offline")
        self._lock_button.setEnabled(False)
        self._unlock_button.setEnabled(False)

    def _tick(self):
        if self._broken:
            return
        try:
            from adapter.core.statusview import lock_enabled, parse_status
            status = parse_status(_get("/api/status"))
        except (OSError, ValueError):
            self._set_offline()
            return
        if status is None:
            self._set_offline()
            return
        phase = status["phase"]
        session = status["session"]
        age = status["pose_age_ms"]
        text = "phase={}".format(phase)
        if age is not None:
            text += " pose_age={}ms".format(age)
        if session:
            text += " session={}".format(session["state"])
        self._status_label.setText(text)
        self._lock_button.setEnabled(lock_enabled(phase, age))
        self._unlock_button.setEnabled(phase == "LOCKED")
        if (phase == "LOCKED" and session and session["state"] == "rendered"
                and session["id"] not in self._pushed):
            self._push(session)

    def _on_lock(self):
        try:
            _post("/api/lock", {})
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode(errors="replace")
            self._status_label.setText("lock refused: {}".format(detail))
        except (OSError, ValueError):
            self._set_offline()

    def _on_unlock(self):
        try:
            _post("/api/unlock", {})
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode(errors="replace")
            self._status_label.setText("unlock refused: {}".format(detail))
        except (OSError, ValueError):
            self._set_offline()

    def _push(self, session):
        try:
            from adapter.core.push import push_session
            from orchestrator.pipeline_api import LayerSpec
            payload = _get("/api/session/layers")
            layers = [LayerSpec(part=item["part"],
                                png_path=pathlib.Path(item["png_path"]))
                      for item in payload["layers"]]
            archives = _STUDIO / "run" / "archives"
            kra = push_session(RealKis(), session, layers, archives)
            self._pushed.add(session["id"])
            self._status_label.setText("pushed {}".format(kra.name))
        except FileExistsError:  # the user's painted archive wins; never clobber
            self._pushed.add(session["id"])
            self._status_label.setText(
                "archive kept: pose_{}.kra".format(session["id"]))
        except Exception as exc:  # never raise into Krita
            self._pushed.add(session["id"])  # no retry storm on the 500ms tick
            self._status_label.setText("push failed: {}".format(exc))
