"""Task Studio configuration and localized UI text."""


import json
import tomllib
from pathlib import Path

CONFIG_DIR = Path(__file__).resolve().parents[1] / "config"


def _read(path: Path):
    if Path(path).suffix == ".toml":
        return tomllib.loads(Path(path).read_text(encoding="utf-8"))
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
    agent = config["agent"]
    command = agent["command"]
    if command is not None and (not isinstance(command, list) or not command
                                or not all(isinstance(part, str) and part for part in command)):
        raise ValueError("setting agent.command must be null or a list of strings")
    for key in ("poll_ms", "timeout_s"):
        value = agent[key]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"setting agent.{key} must be a positive integer")
    if not isinstance(agent["env"], list) or not all(isinstance(n, str) and n for n in agent["env"]):
        raise ValueError("setting agent.env must be a list of variable names")
    strings(config["locale"])  # fail at boot, not on the first page load
    return config


def strings(locale: str) -> dict:
    path = CONFIG_DIR / "locales" / f"{locale}.json"
    if not path.is_file():
        raise ValueError(f"unknown locale {locale!r}")
    return _read(path)
