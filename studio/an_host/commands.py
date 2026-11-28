"""Run the An CLI with the authority selected for one task."""

import json
import os
import subprocess


class Commands:
    def __init__(self, binary):
        self.binary = binary

    def __call__(self, command, body, turn=None):
        env = {**os.environ, "AN_TURN": turn} if turn is not None else None
        proc = subprocess.run(
            [self.binary, *command],
            input=json.dumps(body),
            text=True,
            capture_output=True,
            check=False,
            env=env,
        )
        if proc.returncode:
            raise ValueError(proc.stderr.strip() or "An command failed")
        return json.loads(proc.stdout)
