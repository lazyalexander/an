"""Bridge Studio Krita plugin entrypoint (pykrita convention)."""

from krita import DockWidgetFactory, DockWidgetFactoryBase, Extension, Krita

from .docker import DOCKER_ID, BridgeDocker


class BridgeStudioExtension(Extension):
    def __init__(self, parent):
        super().__init__(parent)

    def setup(self):
        pass

    def createActions(self, window):
        pass


Krita.instance().addExtension(BridgeStudioExtension(Krita.instance()))
Krita.instance().addDockWidgetFactory(
    DockWidgetFactory(DOCKER_ID, DockWidgetFactoryBase.DockRight, BridgeDocker)
)
