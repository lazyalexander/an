"""Verify explicit completion against Krita's synchronous Document API."""
import importlib
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace

import pytest


@pytest.fixture
def adapter(monkeypatch):
    package = ModuleType("native_test_plugin")
    package.__path__ = [str(Path(__file__).resolve().parents[1] / "adapter/krita_plugin/bridge_studio")]
    monkeypatch.setitem(sys.modules, package.__name__, package)
    monkeypatch.setitem(sys.modules, "krita", SimpleNamespace(DockWidget=object, Krita=object))
    monkeypatch.setitem(sys.modules, "PyQt5", ModuleType("PyQt5"))
    monkeypatch.setitem(sys.modules, "PyQt5.QtCore", SimpleNamespace(QSettings=object,QTimer=object))
    monkeypatch.setitem(sys.modules, "PyQt5.QtWidgets", SimpleNamespace(
        QFileDialog=object,QLabel=object,QPushButton=object,QVBoxLayout=object,QWidget=object))
    return importlib.import_module("native_test_plugin.native")


@pytest.mark.parametrize("case", ["closed","wrong-tab","save-as","failed","still-dirty","saved"])
def test_snapshot_requires_the_bound_clean_document(tmp_path, adapter, case):
    work = tmp_path / "working.kra"
    work.write_bytes(b"previously saved")
    def save():
        if case == "failed":
            return False
        work.write_bytes(b"explicitly saved")
        return True
    doc = SimpleNamespace(fileName=lambda: str(work if case != "save-as" else tmp_path/"other.kra"),
                          save=save,waitForDone=lambda:None,modified=lambda:case=="still-dirty")
    output = tmp_path / "submitted.kra"
    if case == "saved":
        adapter.snapshot(doc,doc,tmp_path)
        assert output.read_bytes() == b"explicitly saved"
    else:
        with pytest.raises(ValueError):
            adapter.snapshot(None if case=="closed" else doc, object() if case=="wrong-tab" else doc, tmp_path)
        assert not output.exists()


def test_busy_host_does_not_block_gui_or_duplicate_poll(adapter, monkeypatch):
    from concurrent.futures import ThreadPoolExecutor
    from threading import Event, get_ident

    gui = get_ident()
    callbacks, completed = [], []
    started, unblock = Event(), Event()
    panel = object.__new__(adapter.NativeDocker)
    panel.pending = None
    panel.connection = "test"
    panel.job = {"request": "current"}
    panel.connect_button = SimpleNamespace(setEnabled=lambda _: None)
    panel.polled = lambda result: completed.append((get_ident(), result))
    monkeypatch.setattr(adapter, "QTimer", SimpleNamespace(singleShot=lambda _, cb: callbacks.append(cb)))

    def slow_call(connection, operation, job):
        assert get_ident() != gui and job["request"] == "current"
        started.set()
        assert unblock.wait(2)
        return {"status": "waiting"}

    monkeypatch.setattr(adapter, "call", slow_call)
    with ThreadPoolExecutor(max_workers=1) as panel.executor:
        panel.refresh()
        assert started.wait(1)
        panel.refresh()
        assert len(callbacks) == 1
        callbacks.pop()()
        assert not completed and len(callbacks) == 1
        unblock.set()
        panel.pending.result(timeout=1)
        callbacks.pop()()
        assert completed == [(gui, {"status": "waiting"})]
