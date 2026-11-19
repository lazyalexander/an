"""Use a real stdio peer to exercise private artifact transfer and revocation."""

import asyncio
import fcntl
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.server.fastmcp import FastMCP


def peer():
    server = FastMCP("files")

    @server.tool()
    def exchange(action: str, an_transfer: dict) -> str:
        output = Path(an_transfer["output"])
        if action == "replace":
            output.unlink()
            output.symlink_to(__file__)
        elif action == "oversize":
            with output.open("wb") as stream:
                stream.truncate(32 * 1024**2 + 1)
        elif action == "delay":
            home = output.parents[2]
            (home / "entered").touch()
            for _ in range(100):
                if (home / "continue").exists():
                    break
                time.sleep(0.03)
            output.write_bytes(b"late")
        elif action == "fail":
            raise ValueError("private diagnostic")
        elif action == "consume":
            assert (Path(an_transfer["input"]) / "note").read_bytes() == b"ok"
        else:
            output.write_bytes(b"sample")
        return str(Path(__file__).resolve())

    server.run()


async def check():
    binary = str(Path("target/debug/an").resolve())
    with tempfile.TemporaryDirectory() as tmp:
        env = {**os.environ, "AN_HOME": tmp}

        def cli(*args):
            return subprocess.check_output([binary, *args], env=env, text=True)

        cli(
            "server",
            "register",
            "files",
            "--artifact-tool",
            "exchange",
            "--",
            sys.executable,
            str(Path(__file__).resolve()),
            "peer",
        )
        task = json.loads(cli("task", "new", "--allow-tool", "files/exchange"))["task"]
        env["AN_TURN"] = json.loads(cli("permission", "begin", "--task", task))["turn"]
        with sqlite3.connect(Path(tmp) / "registry.sqlite3") as db:

            def put(kind, ref, data):
                db.execute(
                    "INSERT OR REPLACE INTO objects VALUES(?,?,?)",
                    (kind, ref, json.dumps(data)),
                )

            put("revision", "revision", {"note": "b2s="})
            for owner in (task, "other"):
                put(
                    "run",
                    owner,
                    {
                        "task": owner,
                        "status": "awaiting_publication",
                        "candidate": "revision",
                        "spool": "one",
                    },
                )
        areas = Path(tmp, "transfers")
        areas.mkdir()
        orphan = areas / uuid.uuid4().hex
        orphan.mkdir()
        (orphan / "lease").touch()
        active = areas / uuid.uuid4().hex
        active.mkdir()
        lease = (active / "lease").open("w")
        fcntl.flock(lease, fcntl.LOCK_EX)
        params = StdioServerParameters(
            command=binary, args=["mcp", "--task", task], env=env
        )
        async with stdio_client(params) as streams, ClientSession(*streams) as session:
            await session.initialize()
            assert not orphan.exists() and active.exists()
            lease.close()
            (active / "lease").unlink()
            active.rmdir()
            tool = next(
                t
                for t in (await session.list_tools()).tools
                if t.name.startswith("d_files")
            )
            assert (
                "an_transfer" not in tool.inputSchema["properties"]
                and "an_source" in tool.inputSchema["properties"]
            )

            async def approved(**args):
                first = await session.call_tool(tool.name, args)
                req = json.loads(first.content[0].text)["request_id"]
                if args.get("an_source"):
                    args_preview = [
                        "permission",
                        "preview",
                        "--task",
                        task,
                        "--request-id",
                        req,
                    ]
                    assert json.loads(cli(*args_preview)) == {
                        "revision": "revision",
                        "files": {"note": "b2s="},
                    }
                cli(
                    "permission",
                    "decide",
                    "--task",
                    task,
                    "--request-id",
                    req,
                    "--decision",
                    "once",
                )
                return await session.call_tool(
                    tool.name, {**args, "_an_request_id": req}
                )

            data = json.loads((await approved(action="produce")).content[0].text)
            assert (
                data["bytes"] == 6
                and data["sha256"] == hashlib.sha256(b"sample").hexdigest()
            )
            assert data["artifact"].startswith("artifact-")
            for args in (
                {"action": "consume", "an_transfer": {}},
                {"action": "consume", "an_source": "other"},
            ):
                assert (await session.call_tool(tool.name, args)).isError
            for args in (
                {"action": "consume", "an_source": task},
                {"action": "replace"},
            ):
                assert json.loads((await approved(**args)).content[0].text) == {
                    "status": "completed"
                }
            assert (await approved(action="oversize")).isError
            failed = await approved(action="fail")
            assert failed.isError and "private diagnostic" not in failed.content[0].text
            state = json.loads(
                (await session.call_tool("an_state", {})).content[0].text
            )
            assert any(c.get("result") == data for c in state["calls"])
            assert {c["status"] for c in state["calls"]} == {
                "completed",
                "import_failed",
                "tool_error",
            }
            inflight = asyncio.create_task(approved(action="delay"))
            for _ in range(100):
                if Path(tmp, "entered").exists():
                    break
                await asyncio.sleep(0.03)
            assert Path(tmp, "entered").is_file()
            cli("permission", "end", "--task", task)
            Path(tmp, "continue").touch()
            assert (await inflight).isError
            with sqlite3.connect(Path(tmp, "registry.sqlite3")) as db:
                assert db.execute(
                    "SELECT count(*) FROM objects WHERE kind='artifact'"
                ).fetchone() == (1,)
            assert not list(Path(tmp, "transfers").iterdir())


if __name__ == "__main__":
    if sys.argv[1:] == ["peer"]:
        peer()
    else:
        asyncio.run(check())
