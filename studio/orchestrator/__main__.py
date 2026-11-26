"""Serve a task through the shared An macro UI."""
import argparse
from pathlib import Path

from . import config, graph_server


def main(argv=None):
    parser = argparse.ArgumentParser(prog="orchestrator")
    parser.add_argument("--task", required=True)
    parser.add_argument("--an", default="an")
    parser.add_argument("--port", type=int, default=8791)
    parser.add_argument("--open-ui", action="store_true")
    parser.add_argument("--config", type=Path)
    args = parser.parse_args(argv)
    try:
        settings = config.load(args.config)
    except (OSError, ValueError) as exc:
        parser.error(f"config: {exc}")
    graph_server.serve(args.task, args.an, args.port, args.open_ui, settings)


if __name__ == "__main__":
    main()
