"""Small stdio client shared by backend and Studio integration tests."""

import json
import os
import subprocess


class Mcp:
    def __init__(self, server):
        env = {**os.environ, **server.get("env", {})}
        self.proc = subprocess.Popen(
            [server["command"], *server["args"]],
            env=env,
            text=True,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )
        self.next_id = 0
        self.call(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "fake-agent", "version": "1"},
            },
        )
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def call(self, method, params):
        self.next_id += 1
        self.send(
            {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        )
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
