import os
import time

import pytest


def pytest_collection_modifyitems(config, items):
    if os.environ.get("STUDIO_DOCKER") == "1":
        return
    skip = pytest.mark.skip(reason="requires STUDIO_DOCKER=1 and a running Docker daemon")
    for item in items:
        if "docker" in item.keywords:
            item.add_marker(skip)


def wait_until(predicate, timeout=2.0, interval=0.02):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    raise AssertionError("wait_until timed out")


class FakeClock:
    def __init__(self, start=1_000_000.0):
        self._t = start

    def now(self):
        return self._t

    def advance(self, seconds):
        self._t += seconds


class FakePipeline:
    def __init__(self, barrier=None):
        self.calls = []
        self.barrier = barrier

    def render(self, session, scene_dir, out_dir):
        from orchestrator.pipeline_api import LayerSpec
        self.calls.append(session)
        if self.barrier:
            self.barrier.wait(2)
        return [LayerSpec(part="cube_a", png_path=out_dir / "cube_a.png")]
