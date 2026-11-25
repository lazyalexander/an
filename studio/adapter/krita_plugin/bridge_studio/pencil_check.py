"""Opt-in reference-machine check for the active native request."""
import json
from pathlib import Path

from .native_text import text
from krita import Krita
from PyQt5.QtCore import QObject, QEvent
from PyQt5.QtWidgets import QApplication


class PencilCheck(QObject):
    def __init__(self, doc, job):
        super().__init__()
        self.doc = doc
        folder = Path(job["directory"])
        self.path = folder / "pressure.json"
        self.report = dict(request=job["request"],strokes=[],ordinary_save=False)
        if self.path.exists():
            self.report = json.loads(self.path.read_text())
        self.samples = []
        app = Krita.instance()
        layer = doc.nodeByName("Pencil")
        if layer is None:
            layer = doc.createNode("Pencil","paintlayer")
            doc.rootNode().addChildNode(layer,None)
        doc.setActiveNode(layer)
        brush = next((v for k,v in app.resources("preset").items() if "basic-5 size" in k.lower()),None)
        if brush is None:
            raise ValueError(text("missing_brush"))
        view = app.activeWindow().activeView()
        view.setCurrentBrushPreset(brush)
        view.setBrushSize(16)
        QApplication.instance().installEventFilter(self)
        self.persist()
        (folder/"ready.json").write_text(json.dumps({"request":job["request"],"input":job["input"]}))

    def persist(self):
        temporary = self.path.with_suffix(".partial")
        temporary.write_text(json.dumps(self.report))
        temporary.replace(self.path)

    def eventFilter(self,target,event):
        kind = event.type()
        if kind in (QEvent.TabletPress,QEvent.TabletMove,QEvent.TabletRelease) and Krita.instance().activeDocument()==self.doc:
            if kind==QEvent.TabletPress:
                self.samples = []
            if event.pressure()>0:
                self.samples.append(float(event.pressure()))
            if kind==QEvent.TabletRelease and self.samples:
                self.report["strokes"].append(dict(samples=len(self.samples),minimum=min(self.samples),maximum=max(self.samples)))
                self.samples = []
                self.report["ordinary_save"] = False
                self.persist()
        return False

    def observe_save(self):
        if self.report["strokes"] and not self.report["ordinary_save"] and not self.doc.modified():
            self.report["ordinary_save"] = True
            self.persist()

    def check(self):
        self.observe_save()
        if not any(s["samples"]>=3 and s["maximum"]-s["minimum"]>=0.1 for s in self.report["strokes"]):
            raise ValueError(text("draw"))
        if not self.report["ordinary_save"]:
            raise ValueError(text("ordinary_save"))

    def stop(self):
        QApplication.instance().removeEventFilter(self)
