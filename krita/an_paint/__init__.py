def register():
    try:
        from krita import DockWidgetFactory, DockWidgetFactoryBase, Krita
    except ImportError:
        return
    from .docker import AnDocker

    Krita.instance().addDockWidgetFactory(
        DockWidgetFactory("an_paint", DockWidgetFactoryBase.DockPosition.DockRight, AnDocker)
    )


register()
