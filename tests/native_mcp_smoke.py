"""Exercise the compiled Rust server via an independent MCP client and echo peer."""
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.server.fastmcp import FastMCP


async def exercise():
    binary = str(Path("target/debug/an").resolve())
    with tempfile.TemporaryDirectory() as directory:
        env = {**os.environ, "AN_HOME": directory, "AN_PYTHON": "/missing"}
        def cli(*args):
            return subprocess.check_output([binary, *args], env=env, text=True)
        for name in ("peer", "broken"):
            cli("server", "register", name, "--", sys.executable,
                str(Path(__file__).resolve()), name, directory)
        task = json.loads(cli("task", "new", "--allow-tool", "peer/echo",
                              "--allow-tool", "broken/echo"))["task"]
        for bootstrap in (True, False):
            if not bootstrap:
                env["AN_TURN"] = json.loads(cli("permission", "begin", "--task", task))["turn"]
            config = StdioServerParameters(command=binary, args=["mcp", "--task", task], env=env)
            async with stdio_client(config) as streams, ClientSession(*streams) as session:
                await session.initialize()
                tools = [tool.name for tool in (await session.list_tools()).tools]
                if bootstrap:
                    assert tools == ["an_state"]
                    assert (await session.call_tool("an_unload", {"instance":"unknown"})).isError
                    continue
                assert not any(n.startswith("d_broken_") for n in tools)
                state = json.loads((await session.call_tool("an_state", {})).content[0].text)
                assert state["connections"] == {"broken": "disconnected", "peer": "connected"}
                assert (Path(directory) / "overlap").exists()
                name = next(n for n in tools if n.startswith("d_peer_echo_"))
                args = {"value": "hello"}
                request = json.loads((await session.call_tool(name, args)).content[0].text)
                cli("permission", "decide", "--task", task, "--request-id", request["request_id"], "--decision", "once")
                args["_an_request_id"] = request["request_id"]
                result = await session.call_tool(name, args)
                assert not result.isError and result.content[0].text == "hello"
                assert (await session.call_tool(name, args)).isError
                cli("permission", "end", "--task", task)
                assert (await session.call_tool("an_state", {})).isError


if __name__ == "__main__":
    if len(sys.argv) == 3:
        name, directory = sys.argv[1:]
        root = Path(directory)
        (root / name).touch()
        # The broken peer can finish its handshake attempt only after the
        # healthy peer has started. Serial discovery cannot meet this barrier.
        if name == "broken":
            deadline = time.monotonic() + 5
            while not (root / "peer").exists() and time.monotonic() < deadline:
                time.sleep(.02)
            if (root / "peer").exists():
                (root / "overlap").touch()
            sys.exit(1)
        server = FastMCP("echo")
        @server.tool()
        def echo(value: str) -> str:
            return value
        server.run()
    else:
        asyncio.run(exercise())
