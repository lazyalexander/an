import subprocess
import sys
from pathlib import Path

STUDIO = Path(__file__).resolve().parent.parent


def test_help_exits_zero():
    proc = subprocess.run(
        [sys.executable, "-m", "orchestrator", "--help"],
        cwd=STUDIO, capture_output=True, text=True, timeout=30)
    assert proc.returncode == 0, proc.stderr
    assert "--inbox" in proc.stdout
