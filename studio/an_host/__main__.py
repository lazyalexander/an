"""Operate the local An controller without opening Studio."""
import argparse
import json
from pathlib import Path

from orchestrator.config import load
from .client import connect


def main():
    parser = argparse.ArgumentParser()
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--workspace")
    target.add_argument("--task")
    parser.add_argument("--an", default="an")
    parser.add_argument("--harness", default="studio")
    parser.add_argument("--config", type=Path)
    commands = parser.add_subparsers(dest="action", required=True)
    commands.add_parser("serve")
    commands.add_parser("stop")
    commands.add_parser("start").add_argument("text")
    commands.add_parser("agent").add_argument("task_id")
    args = parser.parse_args()
    request = connect(args.an, args.task, args.workspace, args.harness, load(args.config)["agent"])
    if args.action == "start":
        print(json.dumps(request("start", None, {"text": args.text})))
    elif args.action == "agent":
        print(json.dumps(request("host-agent", args.task_id, {})))
    elif args.action == "stop":
        request("host-stop", None, {})


if __name__ == "__main__":
    main()
