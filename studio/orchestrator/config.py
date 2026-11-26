"""Studio presentation settings and locale resources."""
import json
from pathlib import Path

from an_host.settings import CONFIG_DIR, load as load_settings


def load(override: Path | None = None) -> dict:
    settings = load_settings(override)
    strings(settings["locale"])
    return settings


def strings(locale: str) -> dict:
    path = CONFIG_DIR / "locales" / f"{locale}.json"
    if not path.is_file():
        raise ValueError(f"unknown locale {locale!r}")
    return json.loads(path.read_text(encoding="utf-8"))
