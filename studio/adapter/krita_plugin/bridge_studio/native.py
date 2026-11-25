"""Task-scoped native painting. The only action that creates a receipt is Finish."""
from concurrent.futures import ThreadPoolExecutor
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket

from .native_text import text
from krita import DockWidget, Krita
from PyQt5.QtCore import QSettings, QTimer
from PyQt5.QtWidgets import QFileDialog, QLabel, QPushButton, QVBoxLayout, QWidget


class Rejected(ValueError):
    """The host received a request and explicitly rejected it."""


def call(connection, op, job=None, **extra):
    request = {"op": op, **extra}
    if job:
        request.update({key: job[key] for key in ("node", "input", "request")})
    config = json.loads(Path(connection).read_text())
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(8)
        channel.connect(config["socket"])
        channel.sendall(json.dumps(request).encode() + b"\n")
        with channel.makefile("rb") as reply:
            result = json.loads(reply.readline(16384))
    if "error" in result:
        raise Rejected(result["error"])
    return result


def snapshot(document, active, folder):
    work = folder / "working.kra"
    if document is None or active is None or active != document:
        raise ValueError(text("select_document"))
    if Path(document.fileName()) != work:
        raise ValueError(text("wrong_path"))
    document.waitForDone()
    if not document.save():
        raise ValueError(text("save_failed"))
    document.waitForDone()
    if document.modified():
        raise ValueError(text("dirty"))
    temporary = folder / "snapshot.partial"
    with work.open("rb") as src, temporary.open("wb") as dst:
        shutil.copyfileobj(src, dst)
        dst.flush()
        os.fsync(dst.fileno())
    os.replace(temporary, folder / "submitted.kra")


def send_receipt(connection, job):
    path = Path(job["directory"]) / "submitted.kra"
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    try:
        return call(connection, "receipt", job, digest=digest)
    except Rejected:
        os.replace(path, path.with_suffix(".rejected.kra"))
        raise


class NativeDocker(DockWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle(text("title"))
        self.settings = QSettings("An", "KritaNative")
        self.connection = os.environ.get("AN_KRITA_CONNECTION") or self.settings.value("connection", "")
        self.probe = None
        self.job = self.doc = self.lease = None
        widget = QWidget()
        layout = QVBoxLayout(widget)
        self.label = QLabel(text("awaiting"))
        self.label.setWordWrap(True)
        layout.addWidget(self.label)
        self.connect_button = QPushButton(text("connect"))
        self.connect_button.clicked.connect(self.connect_task)
        layout.addWidget(self.connect_button)
        self.finish_button = QPushButton(text("finish"))
        self.finish_button.setEnabled(False)
        self.finish_button.clicked.connect(self.finish)
        layout.addWidget(self.finish_button)
        self.setWidget(widget)
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.refresh)
        self.timer.start(2000)
        self.executor = ThreadPoolExecutor(max_workers=1)
        self.pending = None
        self.finish_request = None
        self.opened = False

    def canvasChanged(self, canvas):
        pass

    def connect_task(self):
        self.timer.stop()
        path, _ = QFileDialog.getOpenFileName(self, text("select_connection"), "/tmp", "JSON (*.json)")
        self.timer.start(2000)
        if path:
            self.connection = path
            self.settings.setValue("connection", path)
            self.release()
            self.refresh()

    def release(self):
        self.opened = False
        self.finish_request = None
        if self.probe:
            self.probe.stop()
            self.probe = None
        if self.lease:
            self.lease.close()
        self.job = self.doc = self.lease = None
        self.finish_button.setEnabled(False)

    def claim(self, job):
        if self.job and self.job["request"] == job["request"]:
            return
        self.release()
        lease = (Path(job["directory"]) / "editor.lock").open("a")
        try:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            lease.close()
            raise ValueError(text("busy"))
        self.job, self.lease = job, lease

    def open_work(self, job):
        app = Krita.instance()
        work = Path(job["directory"]) / "working.kra"
        for doc in app.documents():
            if Path(doc.fileName()) == work:
                return doc
        window = app.activeWindow()
        if window is None:
            raise ValueError(text("window"))
        doc = app.openDocument(str(work if work.exists() else job["source"]))
        if doc is None:
            raise ValueError(text("open_failed"))
        window.addView(doc)
        doc.waitForDone()
        if not work.exists():
            if not doc.saveAs(str(work)):
                raise ValueError(text("copy_failed"))
            doc.waitForDone()
            if doc.modified() or not work.is_file():
                raise ValueError(text("copy_unsaved"))
        return doc

    def submit(self, operation, handler, *args, **kwargs):
        # Only plain data crosses the thread boundary. Qt dispatches completion
        # on the GUI thread, including all document and widget operations.
        self.pending = self.executor.submit(operation, *args, **kwargs)
        self.connect_button.setEnabled(False)
        QTimer.singleShot(40, lambda: self.collect(handler))

    def collect(self, handler):
        if not self.pending.done():
            QTimer.singleShot(40, lambda: self.collect(handler))
            return
        future, self.pending = self.pending, None
        self.connect_button.setEnabled(True)
        try:
            handler(future.result())
        except Exception as exc:
            self.finish_request = None
            self.label.setText(str(exc))
            self.finish_button.setEnabled(self.opened)

    def deliver(self):
        self.finish_button.setEnabled(False)
        self.submit(send_receipt, self.delivered, self.connection, self.job)

    def delivered(self, result):
        if result["status"] != "done":
            raise ValueError(text("receipt_pending"))
        self.label.setText(text("delivered") + result["revision"][:12])
        self.release()

    def refresh(self):
        if not self.connection or self.pending is not None:
            return
        self.submit(call, self.polled, self.connection, "poll", self.job)

    def polled(self, job):
        if job["status"] == "idle":
            if self.job:
                self.label.setText(text("closed"))
            self.release()
            return
        self.claim(job)
        if (Path(job["directory"]) / "submitted.kra").exists():
            self.deliver()
            return
        if self.doc is not None and self.doc not in Krita.instance().documents():
            self.finish_request = None
            self.finish_button.setEnabled(False)
            self.label.setText(text("reconnect"))
            return
        if self.doc is None:
            self.doc = self.open_work(job)
        if not self.opened:
            self.submit(call, self.on_opened, self.connection, "opened", job)
            return
        if self.probe:
            self.probe.observe_save()
        self.finish_button.setEnabled(True)
        self.label.setText(job.get("error_detail") or (text("painting") + job["node"]))
        if self.finish_request == job["request"]:
            self.finish()

    def on_opened(self, result):
        if (Path(self.job["directory"]) / "check-pencil").is_file():
            from .pencil_check import PencilCheck
            self.show()
            self.raise_()
            self.probe = PencilCheck(self.doc, self.job)
        self.opened = True
        self.finish_button.setEnabled(True)
        self.label.setText(text("painting") + self.job["node"])

    def finish(self):
        if not self.job or not self.opened:
            return
        if self.pending is not None:
            self.finish_request = self.job["request"]
            return
        self.finish_request = None
        try:
            folder = Path(self.job["directory"])
            if self.probe:
                self.probe.check()
            if not (folder / "submitted.kra").exists():
                snapshot(self.doc, Krita.instance().activeDocument(), folder)
            self.deliver()
        except Exception as exc:
            self.label.setText(str(exc))
            self.submit(call, lambda _: None, self.connection, "failed", self.job, detail=str(exc))
