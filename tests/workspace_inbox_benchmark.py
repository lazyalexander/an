"""Measure the production inbox projection and SQLite writer reservation in isolation."""
import sqlite3
import statistics
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
QUERY = (ROOT / "src/workspace/inbox.sql").read_text()


def measure(db):
    plan = [row[3] for row in db.execute("EXPLAIN QUERY PLAN " + QUERY, ("target",))]
    if not any("SEARCH i USING INDEX pv_workspace_inbox" in step for step in plan):
        raise AssertionError(plan)
    samples = []
    for _ in range(201):
        start = time.perf_counter_ns()
        rows = db.execute(QUERY, ("target",)).fetchall()
        samples.append((time.perf_counter_ns() - start) / 1000)
        assert len(rows) == 8
    print(f"median {statistics.median(samples[1:]):.1f} us; {plan}")


with tempfile.TemporaryDirectory() as folder:
    path = Path(folder) / "registry"
    with sqlite3.connect(path) as db, sqlite3.connect(path, timeout=0.02) as writer:
        db.execute("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
        db.executescript((ROOT / "src/provenance/schema.sql").read_text())
        db.execute("""INSERT INTO pv_block VALUES('a',1,'b','{"text":"sample"}')""")
        sql = "INSERT INTO pv_inbox(workspace,receipt,artifact,version,block,metadata) VALUES(?,?,'a',1,'b','{}')"
        db.executemany(sql, (("target", str(i)) for i in range(8)))
        measure(db)
        db.executemany(sql, (("other", f"other-{i}") for i in range(50000)))
        measure(db)
        db.commit()
        db.execute("BEGIN IMMEDIATE")
        db.execute(QUERY, ("target",)).fetchall()
        start = time.perf_counter()
        try:
            writer.execute("BEGIN IMMEDIATE")
        except sqlite3.OperationalError as error:
            if error.sqlite_errorcode != sqlite3.SQLITE_BUSY:
                raise
            print(f"writer blocked for {(time.perf_counter()-start)*1000:.1f} ms with 20 ms busy timeout")
        else:
            raise AssertionError("expected the reader's write reservation")
        db.rollback()
        writer.execute("BEGIN IMMEDIATE")
