"""Serve a task, or a workspace that starts tasks from a sentence, through the macro UI."""
import argparse
from pathlib import Path

from . import config, graph_server


def main(argv=None):
    parser = argparse.ArgumentParser(prog="orchestrator")
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--task", help="serve an existing task")
    target.add_argument("--workspace", help="start tasks in this workspace from the text box")
    parser.add_argument("--harness", default="studio",
                        help="harness name for tasks started in a workspace")
    parser.add_argument("--an", default="an")
    parser.add_argument("--port", type=int, default=8791)
    parser.add_argument("--open-ui", action="store_true")
    parser.add_argument("--config", type=Path)
    args = parser.parse_args(argv)
    try:
        settings = config.load(args.config)
    except (OSError, ValueError) as exc:
        parser.error(f"config: {exc}")
    graph_server.serve(args.task, args.an, args.port, args.open_ui, settings,
                       workspace=args.workspace, harness=args.harness)


if __name__ == "__main__":
    main()
