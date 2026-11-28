"""Minimal agent for runner tests: answers each instruction through An MCP.

It speaks MCP over stdio with only the standard library, so the test needs no
model and no client package. It keeps a session ID in its state directory to
show that the runner gives the same agent the same place to resume.
"""

import json
import os
import sys
import uuid
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tests"))
from mcp_client import Mcp


def main():
    wake = json.load(sys.stdin)
    session_file = Path("session")
    if not session_file.exists():
        session_file.write_text(uuid.uuid4().hex[:8])
    session = session_file.read_text()
    Path("env.json").write_text(json.dumps(sorted(os.environ)))
    with open("wakes", "a") as log:
        log.write(json.dumps({"cursor": wake["cursor"],
                              "kinds": [e["kind"] for e in wake["events"]]}) + "\n")
    if os.environ.get("FAKE_AGENT_SLEEP"):
        import time
        time.sleep(float(os.environ["FAKE_AGENT_SLEEP"]))
    mcp = Mcp(wake["mcp_servers"]["an"])
    try:
        assert not any("control" in name for name in mcp.tools()), "controls leaked to agent"
        for event in wake["events"]:
            if event["kind"] != "instruction":
                continue
            mcp.tool("an_graph_say", {
                "text": f"[{session}] 收到：{event['data']['text']}",
                "purpose": "progress",
                "reply_to": event["cursor"],
                "idempotency_key": f"reply-{event['cursor']}",
            })
    finally:
        mcp.close()


if __name__ == "__main__":
    main()
