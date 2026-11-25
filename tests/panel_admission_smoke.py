"""Exercise image admission with legacy, declared, and damaged panel images."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/graph_studio/spool"


def main():
    with tempfile.TemporaryDirectory() as temp:
        image = Path(temp) / "image"
        shutil.copytree(SOURCE, image)
        env = {**os.environ, "AN_HOME": str(Path(temp) / "registry")}
        path = image / "spool.yaml"
        manifest = path.read_text()
        cases = {
            "legacy": (manifest.split("panels:")[0].replace("an.spool/v2", "an.spool/v1"), None),
            "declared": (manifest, None),
            "missing": (manifest.replace("panels/edit.html", "panels/missing.html"), "Missing or oversized panel HTML"),
            "fallback": (manifest.replace("    decision: {type: point}\n", ""), "panels.edit"),
            "escape": (manifest.replace("panels/edit.html","panels/../edit.html"), "panels.edit"),
        }
        for case, (content, error) in cases.items():
            path.write_text(content)
            tag = f"an-panel-admission:{case}-{uuid.uuid4().hex[:8]}"
            subprocess.run(["container", "build", "-t", tag, str(image)], check=True, stdout=subprocess.DEVNULL)
            try:
                result = subprocess.run([ROOT / "target/debug/an", "register", tag], env=env, text=True, capture_output=True)
                if error:
                    assert result.returncode and error in result.stderr, result.stderr
                else:
                    assert result.returncode == 0, result.stderr
                print(case, "passed", flush=True)
            finally:
                subprocess.run(["container", "image", "delete", tag], check=True, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
