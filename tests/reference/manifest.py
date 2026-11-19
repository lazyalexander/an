"""Bounded reader for the spool interface manifest."""
import re

import yaml

LIMIT = 65536
NAME = re.compile(r"[a-z][a-z0-9-]{0,63}")
TOKEN = r"[A-Za-z0-9][A-Za-z0-9!#$&^_.+\-]{0,126}"
MEDIA = re.compile(f"{TOKEN}/{TOKEN}")


class Loader(yaml.SafeLoader):
    def construct_mapping(self, node, deep=False):
        seen = set()
        for key, _ in node.value:
            if key.tag != "tag:yaml.org,2002:str" or key.value in seen:
                raise ValueError(f"Invalid or duplicate mapping key at line {key.start_mark.line + 1}")
            seen.add(key.value)
        return super().construct_mapping(node, deep)


def fields(value, names, path):
    if not isinstance(value, dict) or set(value) != set(names):
        raise ValueError(f"{path}: expected fields {', '.join(names)}")


def name(value, path):
    if not isinstance(value, str) or not NAME.fullmatch(value):
        raise ValueError(f"{path}: invalid name")


def description(value, path):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{path}: expected nonempty text")


def check(raw):
    if len(raw) > LIMIT:
        raise ValueError("spool.yaml exceeds 64 KiB")
    text = raw.decode("utf-8")
    depth = nodes = 0
    try:
        for event in yaml.parse(text, Loader=Loader):
            if getattr(event, "anchor", None) or getattr(event, "tag", None):
                raise ValueError("Anchors, aliases and explicit tags are unsupported")
            if isinstance(event, (yaml.MappingStartEvent, yaml.SequenceStartEvent)):
                depth += 1
            elif isinstance(event, (yaml.MappingEndEvent, yaml.SequenceEndEvent)):
                depth -= 1
            if isinstance(event, yaml.events.NodeEvent):
                nodes += 1
            if depth > 16 or nodes > 4096:
                raise ValueError("Manifest exceeds structure budget")
        data = yaml.load(text, Loader=Loader)
    except yaml.YAMLError as exc:
        raise ValueError(f"Invalid YAML: {exc}") from exc
    fields(data, ("schema","name","summary","inputs","outputs"), "manifest")
    if data["schema"] != "an.spool/v1":
        raise ValueError("schema: expected an.spool/v1")
    name(data["name"], "name")
    description(data["summary"], "summary")
    for direction in ("inputs", "outputs"):
        ports = data[direction]
        if not isinstance(ports, list):
            raise ValueError(f"{direction}: expected list")
        seen = set()
        for index, port in enumerate(ports):
            path = f"{direction}[{index}]"
            keys = ["name", "description", "media_types"]
            if direction == "inputs":
                keys.append("required")
            fields(port, keys, path)
            name(port["name"], path + ".name")
            if port["name"] in seen:
                raise ValueError(f"{path}.name: duplicate")
            seen.add(port["name"])
            description(port["description"], path + ".description")
            types = port["media_types"]
            if (not isinstance(types, list) or not types
                    or not all(isinstance(t, str) and MEDIA.fullmatch(t) for t in types)
                    or len({t.lower() for t in types}) != len(types)):
                raise ValueError(f"{path}.media_types: expected distinct concrete media types")
            if direction == "inputs" and type(port["required"]) is not bool:
                raise ValueError(f"{path}.required: expected boolean")
    return data
