import pytest

from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.phase import InvalidTransition, NoPose, Phase

from conftest import FakePipeline


def test_initial_phase_is_adjustment(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    assert app.phase is Phase.ADJUSTMENT


def test_lock_without_pose_raises(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    with pytest.raises(NoPose):
        app.lock()


def test_unlock_without_lock_raises(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    with pytest.raises(InvalidTransition):
        app.unlock()
