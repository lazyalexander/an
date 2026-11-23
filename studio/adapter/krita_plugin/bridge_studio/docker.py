"""Bridge docker — a pure agent inside Krita (no controls).

The macro UI web page is the sole control surface; this docker pushes
rendered sessions, sends a gated heartbeat, periodically exports a
flattened preview PNG, and picks up restore requests from the page.

Stdlib + krita + PyQt5 only (Krita 5.3.4 bundles Python 3.10: keep this
file 3.10-compatible). Every orchestrator call fails soft into the
offline state; nothing raises into Krita.
"""

import json
import pathlib
import sys
import time
import urllib.request

from krita import DockWidget, Krita
from PyQt5.QtCore import QBuffer, QIODevice, QTimer, Qt
from PyQt5.QtGui import QImage
from PyQt5.QtWidgets import QLabel, QVBoxLayout, QWidget

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


def _post(path, payload, key=None):
    headers = {"Content-Type": "application/json"}
    if key is not None:
        headers["X-Bridge-Key"] = key
    request = urllib.request.Request(
        BASE + path, data=json.dumps(payload).encode(), headers=headers)
    with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
        return json.loads(response.read())


def _ui_token():
    """Read the per-boot UI token; None means legacy mode (no gating)."""
    if _STUDIO is None:
        return None
    try:
        return (_STUDIO / "run" / "ui_token").read_text().strip() or None
    except (OSError, ValueError):  # ValueError covers corrupt non-UTF-8
        return None


def _flattened_png_bytes(doc):
    """Read-only flattened PNG of the open document.

    doc.pixelData() reads the composited projection without touching the
    user's document (Krita 5's Document has no projection()/duplicate()).

    Only RGBA/U8 documents are read — anything else raises before QImage
    sees the bytes: PyQt5's QImage(data, ...) does no length validation,
    so a shorter buffer (e.g. GRAYA/U8 at 2 bytes/pixel) would read out
    of bounds at save() time. The byte count is double-checked too.

    Byte order: an RGBA/U8 document returns B,G,R,A per pixel (see
    adapter/core/pixels.py; api.kde.org notes the channel order is "most
    often BGR"). QImage.Format_ARGB32 is 0xAARRGGBB in a native uint32,
    which in memory is exactly B,G,R,A on little-endian — the only
    endianness Krita 5 ships (x86_64/arm64) — so the bytes hand over
    with no swap. Alpha is straight (non-premultiplied) on both sides.
    """
    if doc.colorModel() != "RGBA" or doc.colorDepth() != "U8":
        raise RuntimeError(
            f"preview needs an RGBA/U8 document, "
            f"got {doc.colorModel()}/{doc.colorDepth()}")
    width = doc.width()
    height = doc.height()
    data = doc.pixelData(0, 0, width, height)
    if len(data) != width * height * 4:
        raise RuntimeError(
            f"preview pixel data is {len(data)} bytes, "
            f"expected {width * height * 4}")
    image = QImage(data, width, height, QImage.Format_ARGB32)
    if image.isNull():
        raise RuntimeError("preview projection read failed")
    from adapter.core.preview import scaled_size
    target = scaled_size(width, height)
    if target != (width, height):
        image = image.scaled(target[0], target[1], Qt.IgnoreAspectRatio,
                             Qt.SmoothTransformation)
    buffer = QBuffer()
    buffer.open(QIODevice.WriteOnly)
    saved = image.save(buffer, "PNG")
    buffer.close()
    if not saved:
        raise RuntimeError("preview PNG encode failed")
    return bytes(buffer.data())


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
        self.document = None

    def create_document(self, width, height, name):
        doc = self._app.createDocument(width, height, name, "RGBA", "U8", "", 300.0)
        self.document = doc
        window = self._app.activeWindow()
        if window is not None:
            window.addView(doc)
        return RealDocument(doc)


class BridgeDocker(DockWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Bridge")
        self._status_label = QLabel("orchestrator offline")
        layout = QVBoxLayout()
        layout.addWidget(QLabel("Bridge agent — 控制请在宏UI页面进行"))
        layout.addWidget(self._status_label)
        widget = QWidget()
        widget.setLayout(layout)
        self.setWidget(widget)
        self._pushed = set()
        self._restored = set()
        self._acked = set()
        self._kis = None
        self._doc = None           # the live document the adapter owns
        self._doc_session = None   # which session that document belongs to
        self._last_preview = 0.0
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

    def _tick(self):
        if self._broken:
            return
        try:
            from adapter.core.statusview import parse_status
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
        token = _ui_token()
        if token is not None:
            decision = status["pending_decision"]
            if decision is not None:
                self._apply_decision(decision, token)
        if (phase == "LOCKED" and session and session["state"] == "rendered"
                and session["id"] not in self._pushed):
            self._push(session)
        if token is not None:
            self._heartbeat(token)
            pending = status["pending_restore"]
            if pending is not None and pending["id"] not in self._restored:
                self._restore(pending, token)
        self._maybe_export_preview(time.monotonic())

    def _heartbeat(self, token):
        try:
            doc = self._document()
            document = None
            if doc is not None:
                document = {
                    "name": str(getattr(doc, "name", lambda: "")()),
                    "dirty": bool(getattr(doc, "isModified", lambda: False)()),
                    "session": self._doc_session,
                }
            _post("/api/macro/heartbeat", {"document": document}, key=token)
        except Exception:  # best-effort; the status GET owns offline state
            pass

    def _apply_decision(self, decision, token):
        """Persist painted work, then acknowledge; never claim it happened."""
        key = (decision["kind"], decision["session"])
        if key in self._acked:
            return
        from adapter.core.decisions import execute
        archive = _STUDIO / "run" / "archives" / "pose_{}.kra".format(
            decision["session"])
        try:
            ok, detail = execute(decision["kind"], self._document(), archive)
        except Exception as exc:  # never raise into Krita
            ok, detail = False, str(exc)
        try:
            _post("/api/decision/ack",
                  {"kind": decision["kind"], "ok": ok, "detail": detail},
                  key=token)
        except OSError:
            return  # orchestrator keeps it pending; retry on the next tick
        self._acked.add(key)
        if ok and decision["kind"] in ("unlock", "re-render"):
            doc, self._doc, self._doc_session = self._doc, None, None
            try:
                if doc is not None:  # saved above, so this must not prompt
                    doc.close()
            except Exception:  # noqa: BLE001 — closing is best-effort
                pass
        self._status_label.setText(
            "{} {}".format(decision["kind"], "saved" if ok else "failed: " + str(detail)))

    def _document(self):
        """The adapter-owned live document; never an unrelated user tab."""
        if self._doc is not None:
            return self._doc
        try:
            doc = Krita.instance().activeDocument()
        except Exception:  # noqa: BLE001 — shim must never raise into Krita
            return None
        if doc is None or _STUDIO is None:
            return None
        name = str(getattr(doc, "fileName", lambda: "")())
        if name.startswith(str(_STUDIO / "run" / "archives")):
            self._doc = doc
            return doc
        return None

    def _restore(self, pending, token):
        self._restored.add(pending["id"])  # no retry storm on the 500ms tick
        try:
            app = Krita.instance()
            doc = app.openDocument(pending["kra"])
            window = app.activeWindow()
            if window is not None:
                window.addView(doc)
            self._doc = doc
            self._doc_session = pending["id"]
            _post("/api/decision/restore-ack", {"id": pending["id"]},
                  key=token)
            self._status_label.setText(
                "restored {}".format(pathlib.Path(pending["kra"]).name))
        except Exception as exc:  # never raise into Krita
            self._status_label.setText("restore failed: {}".format(exc))

    def _maybe_export_preview(self, now):
        try:
            from adapter.core.preview import atomic_write, clear, due
            if not due(self._last_preview, now):
                return
            self._last_preview = now  # failures retry on the next 3s boundary
            path = _STUDIO / "run" / "preview" / "current.png"
            doc = self._document()
            if doc is None:
                clear(path)
                return
            atomic_write(path, _flattened_png_bytes(doc))
        except Exception as exc:  # never raise into Krita
            self._status_label.setText("preview failed: {}".format(exc))

    def _push(self, session):
        try:
            from adapter.core.push import push_session
            from orchestrator.pipeline_api import LayerSpec
            payload = _get("/api/session/layers")
            layers = [LayerSpec(part=item["part"],
                                png_path=pathlib.Path(item["png_path"]))
                      for item in payload["layers"]]
            archives = _STUDIO / "run" / "archives"
            if self._kis is None:
                self._kis = RealKis()
            kra = push_session(self._kis, session, layers, archives)
            self._doc = self._kis.document
            self._doc_session = session["id"]
            self._pushed.add(session["id"])
            self._status_label.setText("pushed {}".format(kra.name))
        except FileExistsError:  # the user's painted archive wins; never clobber
            self._pushed.add(session["id"])
            self._status_label.setText(
                "archive kept: pose_{}.kra".format(session["id"]))
        except Exception as exc:  # never raise into Krita
            self._pushed.add(session["id"])  # no retry storm on the 500ms tick
            self._status_label.setText("push failed: {}".format(exc))
