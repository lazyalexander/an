"""Task-bound stdio MCP tool gateway."""

import asyncio
import hashlib
import json
import os
import re
import shutil
import time
from contextlib import asynccontextmanager
from datetime import timedelta
from pathlib import Path

from mcp import ClientSession, StdioServerParameters, types
from mcp.client.stdio import stdio_client
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.shared.exceptions import McpError

from kernel import call, connect, get, put, uid


def register(name, argv):
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,32}", name) or not argv:
        raise ValueError("Expected server name and executable")
    program = shutil.which(argv[0])
    if not program:
        raise ValueError("Executable not found")
    config = {"command": os.path.abspath(program), "args": argv[1:], "cwd": os.getcwd()}
    with connect() as db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute(
            "SELECT body FROM objects WHERE kind='server' AND id=?", (name,)
        ).fetchone()
        if row and json.loads(row[0])["config"] == config:
            return
        put(db, "server", name, {"revision": uid(), "config": config}, True)


@asynccontextmanager
async def client(config):
    async with stdio_client(StdioServerParameters(**config)) as streams:
        async with ClientSession(
            *streams, read_timeout_seconds=timedelta(seconds=30)
        ) as session:
            await session.initialize()
            yield session


async def catalog(session):
    tools = {}
    cursor = None
    while True:
        page = await session.list_tools(cursor=cursor)
        tools.update((t.name, t) for t in page.tools)
        cursor = page.nextCursor
        if not cursor:
            return tools


async def list_server(name):
    with connect() as db:
        cfg = get(db, "server", name)["config"]
    async with client(cfg) as session:
        return [
            t.model_dump(by_alias=True, exclude_none=True)
            for t in (await catalog(session)).values()
        ]


def grant(db, names):
    grants = {}
    for ref in names:
        server, sep, tool = ref.partition("/")
        if not sep or not tool:
            raise ValueError("Expected SERVER/TOOL")
        grants[ref] = get(db, "server", server)["revision"]
    return grants


def allowed(task, ref):
    with connect() as db:
        grants = get(db, "task", task).get("tools", {})
        return (
            ref in grants
            and grants[ref] == get(db, "server", ref.split("/", 1)[0])["revision"]
        )


def tool_name(server, tool):
    ref = f"{server}/{tool}"
    prefix = re.sub(r"[^A-Za-z0-9_-]", "_", f"{server}_{tool}")[:43]
    return f"d_{prefix}_{hashlib.sha256(ref.encode()).hexdigest()[:12]}"


def schema(name, description, properties, required):
    return types.Tool(
        name="an_" + name,
        description=description,
        inputSchema={
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": False,
        },
    )


string = {"type": "string"}
strings = {"type": "array", "items": string}
BUILTINS = {
    t.name: t
    for t in [
        schema(
            "state",
            "Inspect granted images, inputs, tools, connections, calls and runs",
            {},
            [],
        ),
        schema(
            "run",
            "Run guest argv in a granted spool. Input is an input name or delivered run ID. Declare paths relative to /output. Returns run ID.",
            {
                "spool": string,
                "input": string,
                "argv": {**strings, "minItems": 1},
                "outputs": strings,
            },
            ["spool", "argv"],
        ),
        *[
            schema(name, desc, {"run": string}, ["run"])
            for name, desc in [
                ("status", "Inspect a run"),
                ("log", "Read run log"),
                ("cancel", "Request cancellation"),
            ]
        ],
    ]
}


async def serve(task):
    api = Server("an", version="0.2.0")
    with connect() as db:
        grants = get(db, "task", task).get("tools", {})
    routes = {}
    peers = {}
    states = {}
    stop = asyncio.Event()

    async def peer(name, ready):
        try:
            with connect() as db:
                config = get(db, "server", name)
            permitted = {
                ref.partition("/")[2]
                for ref, rev in grants.items()
                if ref.partition("/")[0] == name and rev == config["revision"]
            }
            if not permitted:
                states[name] = "requires authorization"
                return
            async with client(config["config"]) as session:
                for raw, tool in (await catalog(session)).items():
                    if raw in permitted:
                        alias = tool_name(name, raw)
                        routes[alias] = (
                            name,
                            raw,
                            tool.model_copy(update={"name": alias}),
                        )
                peers[name] = session
                states[name] = "connected"
                ready.set()
                await stop.wait()
        except Exception:
            states[name] = "disconnected"
        finally:
            peers.pop(name, None)
            ready.set()

    async def refresh(name, session):
        # A stdio child can exit between requests. Ping has no tool side effects.
        if states.get(name) == "connected":
            try:
                await asyncio.wait_for(session.send_ping(), timeout=3)
            except Exception:
                states[name] = "disconnected"

    @api.list_tools()
    async def list_tools():
        return [
            *BUILTINS.values(),
            *[
                tool
                for name, raw, tool in routes.values()
                if states.get(name) == "connected" and allowed(task, f"{name}/{raw}")
            ],
        ]

    @api.call_tool()
    async def invoke(name, arguments):
        if name in BUILTINS:
            result = await asyncio.to_thread(
                call, task, name[3:], {"input": None, "outputs": [], **arguments}
            )
            if name == "an_state":
                await asyncio.gather(
                    *(
                        refresh(server, session)
                        for server, session in list(peers.items())
                    )
                )
                with connect() as db:
                    calls = [
                        json.loads(body)
                        for (body,) in db.execute(
                            "SELECT body FROM objects WHERE kind='call' AND json_extract(body,'$.task')=? ORDER BY rowid DESC LIMIT 50",
                            (task,),
                        )
                    ]
                result.update(connections=states, calls=calls)
            return [
                types.TextContent(
                    type="text", text=json.dumps(result, ensure_ascii=False)
                )
            ]
        route = routes.get(name)
        if not route:
            raise ValueError("Tool is outside the task grant")
        server, raw, _ = route
        if not allowed(task, f"{server}/{raw}"):
            raise ValueError("Tool grant is stale")
        if states.get(server) != "connected" or server not in peers:
            raise ValueError("Downstream disconnected; reconnect the task session")
        key = uid()
        record = dict(
            task=task,
            id=key,
            server=server,
            tool=raw,
            start=time.time(),
            status="unknown",
        )
        with connect() as db:
            put(db, "call", key, record)
        try:
            result = await peers[server].call_tool(raw, arguments)
            record["status"] = "tool_error" if result.isError else "completed"
            return result
        except (Exception, asyncio.CancelledError) as exc:
            if isinstance(exc, McpError) and exc.error.code not in (
                types.CONNECTION_CLOSED,
                408,
            ):
                record["status"] = "protocol_error"
                raise ValueError(
                    f"Downstream MCP error {exc.error.code}: {exc.error.message}"
                ) from exc
            states[server] = "disconnected"
            raise RuntimeError(
                f"Downstream outcome unknown; call {key}; no replay"
            ) from exc
        finally:
            record["end"] = time.time()
            with connect() as db:
                put(db, "call", key, record, True)

    ready = []
    jobs = []
    for name in sorted({ref.partition("/")[0] for ref in grants}):
        event = asyncio.Event()
        ready.append(event.wait())
        jobs.append(asyncio.create_task(peer(name, event)))
    try:
        await asyncio.gather(*ready)
        async with stdio_server() as streams:
            await api.run(*streams, api.create_initialization_options())
    finally:
        stop.set()
        await asyncio.gather(*jobs)
