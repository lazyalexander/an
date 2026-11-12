import json
import urllib.error
import urllib.request
from pathlib import Path

from PyQt5.QtCore import QBuffer, QByteArray, QIODevice
from PyQt5.QtGui import QImage, QPainter
from PyQt5.QtWidgets import (
    QComboBox,
    QInputDialog,
    QLabel,
    QLineEdit,
    QPushButton,
    QVBoxLayout,
    QWidget,
)
from krita import DockWidget, Krita

from .protocol import ProtocolError, decode_layer_png, layers_to_replace, render_body

# Krita RGBA U8 stores bytes as B, G, R, A. That is QImage.Format_ARGB32.
ARGB32 = QImage.Format_ARGB32


class AgentDown(Exception):
    pass


class AnDocker(DockWidget):
    def __init__(self):
        super().__init__()
        self.revision = 0
        self.attempt = None
        self.description = QLineEdit()
        self.description.setPlaceholderText("要渲染成什么样子")
        self.backend = QComboBox()
        self.backend.addItems(["code", "dsh", "codex"])
        self.render_button = QPushButton("渲染")
        self.edit_button = QPushButton("改这里")
        self.abort_button = QPushButton("放弃本次")
        self.status = QLabel("就绪")
        self.status.setWordWrap(True)
        layout = QVBoxLayout()
        for widget in (self.description, self.backend, self.render_button, self.edit_button, self.abort_button, self.status):
            layout.addWidget(widget)
        panel = QWidget()
        panel.setLayout(layout)
        self.setWindowTitle("An")
        self.setWidget(panel)
        self.render_button.clicked.connect(lambda: self.submit(False))
        self.edit_button.clicked.connect(lambda: self.submit(True))
        self.abort_button.clicked.connect(self.abandon)
        self.canvasChanged(None)

    def canvasChanged(self, canvas):
        doc = Krita.instance().activeDocument()
        ready = doc is not None and find_node(doc.rootNode(), "strokes") is not None
        self.render_button.setEnabled(ready)
        self.edit_button.setEnabled(ready)

    def submit(self, local):
        doc = Krita.instance().activeDocument()
        if doc is None:
            self.status.setText("没有打开的文档")
            return
        if doc.colorModel() != "RGBA" or doc.colorDepth() != "U8":
            self.status.setText("需要 8 位 RGBA 文档")
            return
        node = find_node(doc.rootNode(), "strokes")
        if node is None:
            self.status.setText("需要名为 strokes 的绘图层")
            return
        description = self.sentence()
        if description is None:
            return
        selection = self.selection(doc) if local else None
        if local and selection is None:
            return
        port = read_port()
        if port is None:
            self.status.setText("代理没启动")
            return
        try:
            health = exchange(port, "GET", "/v1/health", None, 5)
        except AgentDown:
            self.status.setText("代理没启动")
            return
        self.revision = int(health.get("revision", 0))
        if health.get("locked"):
            self.status.setText("有一次还没确认，按放弃本次")
            return
        try:
            png = strokes_png(node, doc.width(), doc.height())
            body = render_body(doc.fileName() or "untitled", self.revision, self.backend.currentText(), description, png, selection)
            outcome = exchange(port, "POST", "/v1/render", body, 180)
        except AgentDown:
            self.status.setText("代理没启动")
            return
        except (OSError, ProtocolError, ValueError) as error:
            self.status.setText(str(error))
            return
        if outcome.get("status") != "ready":
            self.show_stopped(outcome)
            return
        self.attempt = outcome.get("attempt")
        self.apply_ready(doc, outcome)

    def abandon(self):
        port = read_port()
        if port is None:
            self.status.setText("代理没启动")
            return
        try:
            outcome = exchange(port, "POST", "/v1/abort", {"attempt": self.attempt or ""}, 30)
        except AgentDown:
            self.status.setText("代理没启动")
            return
        if outcome.get("status") == "failed":
            self.status.setText(outcome.get("detail") or "放弃失败")
            return
        self.attempt = None
        if "revision" in outcome:
            self.revision = int(outcome["revision"])
        self.status.setText("已放弃")

    def sentence(self):
        description = self.description.text().strip()
        if description:
            return description
        text, ok = QInputDialog.getText(self, "An", "要渲染成什么样子")
        if not ok:
            return None
        description = text.strip()
        if not description:
            return None
        self.description.setText(description)
        return description

    def selection(self, doc):
        selected = doc.selection()
        if selected is None or selected.width() <= 0 or selected.height() <= 0:
            self.status.setText("需要选区")
            return None
        if selected.x() < 0 or selected.y() < 0:
            self.status.setText("选区要在画布里面")
            return None
        return {"x": int(selected.x()), "y": int(selected.y()), "w": int(selected.width()), "h": int(selected.height())}

    def show_stopped(self, outcome):
        reason = outcome.get("reason")
        if reason == "locked":
            self.status.setText("有一次还没确认，按放弃本次")
            return
        if reason == "stale":
            self.status.setText("修订已过期")
            return
        self.status.setText(str(outcome.get("detail") or reason or outcome.get("status")))

    def apply_ready(self, doc, outcome):
        layers = outcome.get("layers") or []
        root = doc.rootNode()
        added = []
        try:
            delete, replace = layers_to_replace(node_names(root), [layer["name"] for layer in layers])
            by_name = {layer["name"]: layer for layer in layers}
            anchor = None
            for name in replace:
                raw = png_to_krita_bytes(decode_layer_png(by_name[name]["png_base64"]), doc.width(), doc.height())
                node = find_node(root, name)
                if node is None:
                    node = doc.createNode(name, "paintlayer")
                    root.addChildNode(node, anchor)
                    added.append(node)
                node.setPixelData(QByteArray(raw), 0, 0, doc.width(), doc.height())
                anchor = node
            for name in delete:
                node = find_node(root, name)
                if node is not None:
                    node.remove()
            doc.waitForDone()
            doc.refreshProjection()
        except Exception as error:
            for node in added:
                try:
                    node.remove()
                except Exception:
                    pass
            self.abort_quietly()
            self.status.setText(str(error))
            return
        port = read_port()
        if port is None:
            self.status.setText("代理没启动")
            return
        try:
            committed = exchange(port, "POST", "/v1/commit", {"attempt": outcome.get("attempt") or ""}, 30)
        except AgentDown:
            self.status.setText("图层已写上，但确认失败")
            return
        if committed.get("status") == "failed":
            self.status.setText(committed.get("detail") or "确认失败")
            return
        self.attempt = None
        self.revision = int(committed.get("revision", self.revision))
        recognized = outcome.get("recognized")
        if recognized:
            self.status.setText("认出的是%s" % recognized)
        else:
            self.status.setText("已写回 %d 层" % len(replace))

    def abort_quietly(self):
        port = read_port()
        if port is None:
            return
        try:
            exchange(port, "POST", "/v1/abort", {"attempt": self.attempt or ""}, 30)
        except AgentDown:
            return
        self.attempt = None


def read_port():
    path = Path(__file__).resolve().parents[2] / "state" / "port"
    try:
        return int(path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None


def exchange(port, method, path, payload, timeout):
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request("http://127.0.0.1:%s%s" % (port, path), data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read()
    except urllib.error.URLError as error:
        raise AgentDown(str(error.reason)) from error
    try:
        return json.loads(raw.decode("utf-8"))
    except json.JSONDecodeError as error:
        raise ProtocolError("代理返回的不是 JSON") from error


def strokes_png(node, width, height):
    canvas = QImage(int(width), int(height), ARGB32)
    canvas.fill(0)
    placed = layer_image(node, int(width), int(height))
    if placed is not None:
        x, y, image = placed
        painter = QPainter(canvas)
        try:
            painter.drawImage(x, y, image)
        finally:
            painter.end()
    return qimage_png(canvas)


def layer_image(node, width, height):
    bounds = node.bounds()
    x0 = max(0, int(bounds.x()))
    y0 = max(0, int(bounds.y()))
    x1 = min(width, int(bounds.x() + bounds.width()))
    y1 = min(height, int(bounds.y() + bounds.height()))
    if x1 <= x0 or y1 <= y0:
        return None
    w, h = x1 - x0, y1 - y0
    reader = getattr(node, "projectionPixelData", None) or node.pixelData
    data = reader(x0, y0, w, h)
    if data is None or data.size() < w * h * 4:
        return None
    return x0, y0, QImage(data, w, h, ARGB32).copy()


def png_to_krita_bytes(png, width, height):
    image = QImage.fromData(QByteArray(png))
    if image.isNull():
        raise ProtocolError("图层 PNG 解不开")
    canvas = QImage(int(width), int(height), ARGB32)
    canvas.fill(0)
    painter = QPainter(canvas)
    try:
        painter.drawImage(0, 0, image)
    finally:
        painter.end()
    bits = canvas.constBits()
    bits.setsize(canvas.sizeInBytes())
    raw = bytes(bits)
    if len(raw) != int(width) * int(height) * 4:
        raise ProtocolError("图层尺寸和文档不一致")
    return raw


def qimage_png(image):
    buffer = QBuffer()
    if not buffer.open(QIODevice.WriteOnly):
        raise OSError("写不出笔触 PNG")
    if not image.save(buffer, "PNG"):
        raise OSError("写不出笔触 PNG")
    return bytes(buffer.data())


def find_node(node, name):
    for child in node.childNodes():
        if child.name() == name:
            return child
        found = find_node(child, name)
        if found is not None:
            return found
    return None


def node_names(node):
    names = []
    for child in node.childNodes():
        names.append(child.name())
        names.extend(node_names(child))
    return names
