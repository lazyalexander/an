"""Studio configuration: settings and every user-facing string live in studio/config/.

`studio.json` holds the locale and operational settings; `locales/<locale>.json`
holds UI text; `instances/<name>.json` holds instance manifests. An override
file may replace existing settings but never introduce unknown keys, so a typo
fails loudly instead of being silently ignored.
"""

import json
from pathlib import Path

CONFIG_DIR = Path(__file__).resolve().parents[1] / "config"


def _read(path: Path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def _merge(base: dict, extra: dict, where: str) -> None:
    for key, value in extra.items():
        if key not in base:
            raise ValueError(f"unknown setting {where}{key}")
        if isinstance(base[key], dict):
            if not isinstance(value, dict):
                raise ValueError(f"setting {where}{key} must be an object")
            _merge(base[key], value, f"{where}{key}.")
        else:
            base[key] = value


def load(override: Path | None = None) -> dict:
    config = _read(CONFIG_DIR / "studio.json")
    if override is not None:
        _merge(config, _read(override), "")
    if not isinstance(config["locale"], str) or not config["locale"]:
        raise ValueError("setting locale must be a name")
    for key, value in config["graph"].items():
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"setting graph.{key} must be a positive integer")
    strings(config["locale"])  # fail at boot, not on the first page load
    return config


def strings(locale: str) -> dict:
    path = CONFIG_DIR / "locales" / f"{locale}.json"
    if not path.is_file():
        raise ValueError(f"unknown locale {locale!r}")
    return _read(path)


def instance_manifest(name: str) -> dict:
    return _read(CONFIG_DIR / "instances" / f"{name}.json")
