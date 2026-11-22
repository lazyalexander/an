import argparse

from .app import DEFAULT_RUN_DIR, Orchestrator
from .fake_pipeline import FakePipeline
from .ledger import Ledger
from .server import make_server


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(prog="orchestrator")
    parser.add_argument("--port", type=int, default=8791)
    parser.add_argument("--fake", action="store_true",
                        help="wire the deterministic FakePipeline")
    parser.add_argument("--width", type=int, default=1600)
    parser.add_argument("--height", type=int, default=1200)
    args = parser.parse_args(argv)

    run_dir = DEFAULT_RUN_DIR
    run_dir.mkdir(parents=True, exist_ok=True)
    ledger = Ledger(run_dir / "ledger.jsonl")
    if args.fake:
        pipeline = FakePipeline()
    else:
        try:
            from pipeline.docker_pipeline import DockerPipeline
            pipeline = DockerPipeline()
        except ImportError:
            parser.error("real pipeline unavailable; pass --fake")
    app = Orchestrator(ledger, pipeline, (args.width, args.height), run_dir=run_dir)
    server = make_server(app, port=args.port)
    print(f"orchestrator on 127.0.0.1:{args.port} (fake={args.fake})")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
