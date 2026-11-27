"""Codex CLI adapter for the An agent runner.

Configure Studio with
  {"agent": {"command": ["python3", "<repo>/examples/agents/codex_agent.py"],
             "env": ["CODEX_HOME"]}}
The runner starts this script with the task's state directory as its working
directory and the wake on stdin. Codex runs with its shell, web and file tools
disabled; it sees only An MCP tools, and it resumes the same thread per task.
"""

import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "markdown_review"))
from codex_session import Session

TOOLS = {"an_catalog", "an_state", "an_scope_propose", "an_skill", "an_graph_state", "an_graph_submit", "an_graph_say"}

BRIEF = """You are the agent for one An task. You act only through the An MCP tools.
Work backwards from the requested deliverables. Use an_catalog to discover task inputs and compatible spool interfaces. Read an_skill
for selected commands and filenames. Link each consumer input to its producer node and output port. When asked to change the
work, read an_graph_state and extend the existing graph using its published or completed node outputs. Reuse the
completed work. Pass the user event's instruction_cursor to an_graph_submit and an_scope_propose. Propose only the
additional routes the instruction needs. Check turn_scopes in an_graph_state. If prior decisions cover every step, submit directly.
For missing routes, obtain a pending card receipt before graph submission. If the proposal fails, correct it using the route schema.
Studio collects the human decision. Read progress with an_graph_state. Answer the person with an_graph_say: say what
you understood and what you will do, ask when the request is ambiguous, and report
results. Use reply_to with the instruction's event cursor. Do not claim work you
have not submitted. Ask for decisions with surfaces, not with messages."""


def main():
    wake = json.load(sys.stdin)
    lines = []
    for event in wake["events"]:
        data = event.get("data", {})
        if event["kind"] == "instruction":
            stale = " (sent against an older graph revision)" if data.get("stale") else ""
            lines.append(f"- instruction at cursor {event['cursor']}{stale}: {data.get('text', '')}")
        elif event["kind"] == "responded":
            lines.append(f"- the person answered surface {event.get('node')}")
        elif event["kind"] == "intent_decided":
            lines.append(f"- the person decided scope {data['intent']} for instruction at cursor "
                         f"{data['instruction_cursor']}: {json.dumps(data['decisions'])}. "
                         "Read graph state for that instruction. Continue using its original cursor "
                         "and the reviewed scope. Skip work already submitted and respect all denials.")
        elif event["kind"] == "resumed":
            lines.append("- the person resumed the task")
    state = Path(wake["state_dir"])
    session = Session(state, os.environ["AN_BINARY"], wake["task"], os.environ, tools=TOOLS)
    first = not session.session_file.exists()
    prompt = (BRIEF + "\n\n" if first else "") + "New events:\n" + "\n".join(lines)
    session.turn(prompt, f"wake-{wake['cursor']}")


if __name__ == "__main__":
    main()
