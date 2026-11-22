from orchestrator.ledger import Ledger


def test_record_and_read_back(tmp_path):
    led = Ledger(tmp_path / "ledger.jsonl", now=iter([1.0, 2.0]).__next__)
    e = led.record("lock", "session frozen", bytes_moved=128, latency_ms=3.5, session="s1")
    assert e["ts"] == 1.0 and e["session"] == "s1"
    led.record("unlock", "back to adjustment")
    entries = led.read_all()
    assert [x["kind"] for x in entries] == ["lock", "unlock"]
    assert entries[0]["bytes_moved"] == 128


def test_corrupt_line_is_skipped_with_default_fields(tmp_path):
    p = tmp_path / "ledger.jsonl"
    p.write_text('{"kind":"a","summary":"x"}\nnot json\n')
    entries = Ledger(p).read_all()
    assert len(entries) == 1 and entries[0]["bytes_moved"] == 0
