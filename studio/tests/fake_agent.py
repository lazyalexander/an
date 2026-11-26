"""Minimal agent for runner tests: answers each instruction through An MCP.

It speaks MCP over stdio with only the standard library, so the test needs no
model and no client package. It keeps a session ID in its state directory to
show that the runner gives the same agent the same place to resume.
"""

import json
import os
import subprocess
import sys
import uuid
from pathlib import Path


class Mcp:
    def __init__(self, server):
        env = {**os.environ, **server.get("env", {})}
        self.proc = subprocess.Popen([server["command"], *server["args"]], env=env, text=True,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.next_id = 0
        self.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "fake-agent", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def call(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while True:
            reply = json.loads(self.proc.stdout.readline())
            if reply.get("id") == self.next_id:
                if "error" in reply:
                    raise RuntimeError(reply["error"])
                return reply["result"]

    def tool(self, name, arguments):
        result = self.call("tools/call", {"name": name, "arguments": arguments})
        text = result["content"][0]["text"]
        if result.get("isError"):
            raise RuntimeError(text)
        return json.loads(text)

    def tools(self):
        return {t["name"] for t in self.call("tools/list", {})["tools"]}

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


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
