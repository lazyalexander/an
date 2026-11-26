"""In-memory fake of the adapter Kis protocol, recording structure for assertions."""

import pathlib


class FakeNode:
    def __init__(self, name, parent=None):
        self.name = name
        self.parent = parent
        self.children = []
        self.locked = False
        self.png_path = None


class FakeDocument:
    def __init__(self, width, height, name):
        self.width = width
        self.height = height
        self.name = name
        self.root = FakeNode("")
        self.saved_to = None

    def group(self, name, parent=None):
        node = FakeNode(name, parent)
        (parent or self.root).children.append(node)
        return node

    def layer_from_png(self, name, parent, png_path):
        node = FakeNode(name, parent)
        node.png_path = pathlib.Path(png_path)
        (parent or self.root).children.append(node)
        return node

    def set_locked(self, node, locked):
        node.locked = locked

    def save(self, path):
        self.saved_to = pathlib.Path(path)

    def top_level_names(self):
        return [c.name for c in self.root.children]

    def _find(self, name, node=None):
        node = node or self.root
        for child in node.children:
            if child.name == name:
                return child
            found = self._find(name, child)
            if found:
                return found
        return None

    def children_of(self, name):
        node = self._find(name)
        return [c.name for c in node.children] if node else None

    def is_locked(self, name):
        return self._find(name).locked


class FakeKis:
    def __init__(self):
        self.documents = []

    def create_document(self, width, height, name):
        doc = FakeDocument(width, height, name)
        self.documents.append(doc)
        return doc
