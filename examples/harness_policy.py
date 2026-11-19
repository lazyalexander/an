"""Install the trusted harness decision for one pending effect and this turn."""
import argparse
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task")
    parser.add_argument("request")
    parser.add_argument("decision", choices=["allow", "deny", "ask", "unavailable"])
    args = parser.parse_args()
    subprocess.run([
        "an", "permission", "effect", "--task", args.task,
        "--request-id", args.request, "--decision", args.decision,
    ], check=True)


if __name__ == "__main__":
    main()
