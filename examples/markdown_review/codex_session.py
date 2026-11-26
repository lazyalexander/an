"""Restricted, reconnectable Codex CLI session for the public Markdown experiment."""

import json
import shutil
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

TOOLS = {"an_skill", "an_graph_state", "an_graph_submit"}
MODEL = "gpt-6.2-sol"


def inventory(tools):
    names = set()
    for tool in tools:
        if tool["type"] == "namespace":
            for child in tool["tools"]:
                names.add(tool["name"].rstrip("_") + "__" + child["name"])
        else:
            names.add(tool.get("name", tool["type"]))
    return names


class Session:
    def __init__(self, directory, binary, task, env, tools=TOOLS):
        self.tools = set(tools)
        self.work = directory / "agent"
        self.work.mkdir(exist_ok=True)
        home = directory / "codex"
        home.mkdir(mode=0o700, exist_ok=True)
        env = dict(env)
        if not env.get("AN_AGENT_TOKEN") and env.get("AN_TURN"):
            grant = subprocess.check_output([binary, "permission", "delegate", "--task", task], env=env)
            env["AN_AGENT_TOKEN"] = json.loads(grant)["agent_token"]
        self.env = {
            k: env[k]
            for k in ("HOME", "PATH", "TMPDIR", "AN_HOME", "AN_AGENT_TOKEN")
            if k in env
        }
        self.env["CODEX_HOME"] = str(home)
        self.session_file = directory / "thread"
        self.codex = shutil.which("codex")
        catalog = json.loads(
            subprocess.check_output([self.codex, "debug", "models", "--bundled"])
        )
        model = next(m for m in catalog["models"] if m["slug"] == MODEL)
        model.update(
            shell_type="disabled",
            apply_patch_tool_type=None,
            tool_mode="direct",
            experimental_supported_tools=[],
            supports_search_tool=False,
            node_repl_disabled=True,
            multi_agent_version=None,
            use_responses_lite=False,
        )
        catalog_file = directory / "catalog.json"
        catalog_file.write_text(json.dumps({"models": [model]}))
        self.args = [
            self.codex,
            "exec",
            "--ignore-user-config",
            "--ignore-rules",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--json",
            "-m",
            MODEL,
        ]
        for feature in (
            "shell_tool",
            "unified_exec",
            "view_image",
            "image_generation",
            "computer_use",
            "apps",
            "plugins",
            "multi_agent",
            "multi_agent_v2",
            "goals",
            "code_mode",
            "skill_search",
            "tool_suggest",
        ):
            self.args.extend(["--disable", feature])
        settings = {
            "web_search": "disabled",
            "model_catalog_json": str(catalog_file),
            "model_reasoning_effort": "low",
            "project_doc_max_bytes": 0,
            "mcp_servers.an.command": str(binary),
            "mcp_servers.an.args": ["mcp", "--task", task],
            "mcp_servers.an.env_vars": ["AN_HOME", "AN_AGENT_TOKEN"],
            "mcp_servers.an.enabled_tools": sorted(self.tools),
            "mcp_servers.an.default_tools_approval_mode": "approve",
            "mcp_servers.an.required": True,
        }
        for k, v in settings.items():
            self.args.extend(["-c", k + "=" + json.dumps(v)])
        self.check_tools(directory)
        auth = Path(env.get("CODEX_HOME", Path.home() / ".codex")) / "auth.json"
        if not (home / "auth.json").exists():
            (home / "auth.json").symlink_to(auth.resolve(strict=True))

    def check_tools(self, directory):
        cache = directory / "tools.json"
        expected = sorted({"mcp__an__" + name for name in self.tools})
        if cache.exists() and set(expected) <= set(json.loads(cache.read_text())):
            return  # preflight already passed for this tool set in this state dir
        # Inspect the model-facing request produced by the installed CLI. The
        # probe has no provider credentials and never returns a model action.
        captured = []

        class Capture(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                captured.append(inventory(body["tools"]))
                self.send_error(400, "Preflight complete")

        with HTTPServer(("127.0.0.1", 0), Capture) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            options = [
                "-c",
                'model_provider="inventory"',
                "-c",
                'model_providers.inventory.name="inventory"',
                "-c",
                'model_providers.inventory.wire_api="responses"',
                "-c",
                f'model_providers.inventory.base_url="http://127.0.0.1:{server.server_port}/v1"',
            ]
            try:
                probe = subprocess.run(
                    self.args + options + ["--ephemeral", "List tools."],
                    cwd=self.work,
                    check=False,
                    env=self.env,
                    capture_output=True,
                    timeout=75,
                )
            finally:
                server.shutdown()
        if not captured:
            raise RuntimeError(
                "Codex tool preflight failed: "
                + probe.stderr.decode()[-1500:]
                + probe.stdout.decode()[-1000:]
            )
        names = captured[0]
        required = {"mcp__an__" + name for name in self.tools}
        harmless = {
            "request_user_input",
            "update_plan",
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource",
        }
        if not required <= names or names - required - harmless:
            raise RuntimeError("Unexpected model-visible tools: " + repr(sorted(names)))
        (directory / "tools.json").write_text(json.dumps(sorted(names)))

    def turn(self, prompt, label):
        args = list(self.args)
        if self.session_file.exists():
            args.extend(["resume", self.session_file.read_text().strip()])
        result = subprocess.run(
            args + [prompt],
            cwd=self.work,
            check=False,
            env=self.env,
            capture_output=True,
            text=True,
            timeout=300,
        )
        (self.work.parent / (label + ".jsonl")).write_text(result.stdout)
        for line in result.stdout.splitlines():
            event = json.loads(line)
            if event.get("type") == "thread.started":
                thread = event["thread_id"]
                if (
                    self.session_file.exists()
                    and self.session_file.read_text() != thread
                ):
                    raise RuntimeError("Codex resumed a different thread")
                self.session_file.write_text(thread)
        if result.returncode or not self.session_file.exists():
            raise RuntimeError(
                "Codex turn failed: " + result.stderr[-1200:] + result.stdout[-1000:]
            )
