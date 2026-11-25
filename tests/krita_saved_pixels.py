"""Run inside Krita's kritarunner. Inspect the unmerged paint layer in an exported KRA."""
import hashlib
import json
import os
from pathlib import Path

from krita import Krita


def verify(*args):
    path = Path(os.environ["AN_VERIFY_KRA"])
    doc = Krita.instance().openDocument(str(path))
    if not doc:
        raise ValueError("Krita could not reopen the exported delivery")
    doc.waitForDone()
    layers = [node for node in doc.rootNode().childNodes() if node.name() == "Pencil"]
    if len(layers) != 1 or layers[0].type() != "paintlayer":
        raise ValueError("Expected one independent Pencil paint layer")
    node = layers[0]
    if node.colorDepth() != "U8" or node.colorModel() != "RGBA":
        raise ValueError("Expected an 8-bit RGBA paint layer")
    w,h = doc.width(),doc.height()
    data = bytes(node.pixelData(0,0,w,h))
    columns = [sum(data[(y*w+x)*4+3] > 16 for y in range(h)) for x in range(w)]
    widths = sorted(n for n in columns if n)
    if not widths:
        raise ValueError("The exported Pencil layer has no painted pixels")
    evidence = dict(layer="Pencil", pixels=sum(columns), columns=len(widths),
                    width_p10=widths[len(widths)//10],width_p90=widths[len(widths)*9//10],
                    layer_sha256=hashlib.sha256(data).hexdigest(),
                    kra_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
    Path(os.environ["AN_PIXEL_REPORT"]).write_text(json.dumps(evidence,indent=2))


def fixture(*args):
    """Produce an ordinary saved test canvas. This is synthetic pixel evidence."""
    job = json.loads(Path(os.environ["AN_NATIVE_JOB"]).read_text())
    doc = Krita.instance().openDocument(job["source"])
    doc.waitForDone()
    layer = doc.createNode("Pencil","paintlayer")
    doc.rootNode().addChildNode(layer,None)
    layer.setPixelData(bytes([0,0,0,255])*16, 10,10,8,2)
    doc.refreshProjection()
    if not doc.saveAs(str(Path(job["directory"]) / "working.kra")):
        raise ValueError("Krita did not save the fixture")
    doc.waitForDone()
    if doc.modified():
        raise ValueError("Krita still reports an unsaved fixture")
