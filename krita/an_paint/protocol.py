import base64


class ProtocolError(Exception):
    pass


def render_body(document, revision, backend, description, strokes_png, selection):
    return {
        "document": document,
        "revision": revision,
        "backend": backend,
        "description": description,
        "png_base64": base64.b64encode(strokes_png).decode("ascii"),
        "selection": selection,
    }


def layers_to_replace(previous_names, new_names):
    for name in new_names:
        if not _an_layer(name):
            raise ProtocolError("图层 %s 不在 an/ 下" % name)
    if len(set(new_names)) != len(new_names):
        raise ProtocolError("图层名字重复")
    coming = set(new_names)
    delete = [name for name in previous_names if name.startswith("an/") and name not in coming]
    return delete, list(new_names)


def decode_layer_png(png_base64):
    try:
        raw = base64.b64decode(png_base64, validate=True)
    except Exception as error:
        raise ProtocolError("png_base64 不是 base64") from error
    if not raw.startswith(b"\x89PNG"):
        raise ProtocolError("图层不是 PNG")
    return raw


def _an_layer(name):
    if not name.startswith("an/"):
        return False
    rest = name[3:]
    return rest != "" and "/" not in rest
