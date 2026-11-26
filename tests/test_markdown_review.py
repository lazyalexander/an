"""Check model output validation and retry against the shipped continuation."""
import importlib.util
import json
from pathlib import Path
import shutil

import pytest

EXAMPLE = Path(__file__).resolve().parents[1] / "examples/markdown_review"


@pytest.mark.parametrize("answer", ["not JSON", '{"conflict":"false"}', '{"conflict":false}'])
def test_invalid_route_then_valid_analysis(tmp_path, answer):
    spec = importlib.util.spec_from_file_location("review", EXAMPLE/"spool/code/driver.py")
    driver = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(driver)
    driver.INPUT = tmp_path
    for name in ("source","reviews","analysis"):
        (tmp_path/name).mkdir()
    shutil.copy(EXAMPLE/"fixtures/original.md", tmp_path/"source")
    shutil.copy(EXAMPLE/"fixtures/conflict/reviews.json", tmp_path/"reviews")
    analysis = tmp_path/"analysis/result.json"
    analysis.write_text(json.dumps({"text":answer}))
    with pytest.raises(ValueError):
        driver.execute("route","test")
    analysis.write_text(json.dumps({"text":'{"conflict":true}'}))
    assert driver.execute("route","test")[0]["panel"] == "resolve"


def test_crlf_patch_replays_on_original_bytes(tmp_path):
    import subprocess
    spec = importlib.util.spec_from_file_location("review", EXAMPLE/"spool/code/driver.py")
    driver = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(driver)
    driver.INPUT = driver.OUTPUT = tmp_path
    (tmp_path/"source").mkdir()
    (tmp_path/"reviews").mkdir()
    original = tmp_path/"source/original.md"
    original.write_bytes((EXAMPLE/"fixtures/original.md").read_bytes().replace(b"\n", b"\r\n"))
    shutil.copy(EXAMPLE/"fixtures/compatible/reviews.json", tmp_path/"reviews")
    driver.execute("revise","test")
    revised = (tmp_path/"revised.md").read_bytes()
    assert revised.count(b"\r\n") == original.read_bytes().count(b"\n")
    subprocess.run(["patch",str(original)],input=(tmp_path/"changes.diff").read_bytes(),check=True)
    assert original.read_bytes() == revised
