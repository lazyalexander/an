"""Build five independent fixture images and their public interfaces."""

import json
from pathlib import Path
import shutil
import subprocess
import tomllib
import hashlib

HERE = Path(__file__).resolve().parent
SPOOLS = tomllib.loads((HERE / "catalog.toml").read_text())
CONTAINER = "/usr/local/bin/container"
COMMAND = ["python3", "/spool/code/process.py"]


def manifest(name, spec):
    return {
        "schema": "an.spool/v1",
        "name": name,
        "summary": spec["summary"],
        "inputs": [
            {
                "name": spec["input"],
                "description": "Source artifact",
                "media_types": [spec["accepts"]],
                "required": True,
            }
        ],
        "outputs": [
            {
                "name": "result",
                "description": spec["output"],
                "media_types": [spec["produces"]],
            }
        ],
    }


def skill(spec):
    return (
        f"{spec['summary']} Run {' '.join(COMMAND)} with input port {spec['input']}. "
        f"Declare the output filename {spec['output']} and use it as the graph output port."
    )


def build(directory, binary, cli, names=SPOOLS):
    images = {}
    for name in names:
        spec = SPOOLS[name]
        context = directory / name
        context.mkdir(parents=True)
        interface = context / "spool.yaml"
        interface.write_text(json.dumps(manifest(name, spec), indent=2))
        subprocess.run([binary, "check", interface], check=True)
        (context / "SKILL.md").write_text(skill(spec))
        shutil.copy(HERE / "catalog" / f"{name}.py", context / "process.py")
        (context / "Dockerfile").write_text(
            "FROM python:3.13-alpine\nCOPY spool.yaml SKILL.md /spool/\n"
            "COPY process.py /spool/code/process.py\n"
        )
        digest = hashlib.sha256()
        for path in sorted(context.iterdir()):
            digest.update(hashlib.sha256(path.read_bytes()).digest())
        tag = f"an-composition-{name}:{digest.hexdigest()[:12]}"
        subprocess.run([CONTAINER, "build", "--tag", tag, str(context)], check=True)
        images[name] = cli("register", tag)
    return images
