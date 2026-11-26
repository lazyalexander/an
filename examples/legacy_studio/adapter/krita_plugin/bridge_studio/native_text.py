"""Read native adapter labels from the shared Studio configuration."""
import json
from pathlib import Path

_config = Path(__file__).resolve().parents[3] / "config"
_locale = json.loads((_config / "studio.json").read_text())["locale"]
_labels = json.loads((_config / "locales" / (_locale + ".json")).read_text())["native_krita"]


def text(key):
    return _labels[key]
