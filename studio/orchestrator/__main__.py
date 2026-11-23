import argparse
import secrets
import webbrowser
from pathlib import Path

from .app import DEFAULT_RUN_DIR, Orchestrator
from .fake_pipeline import FakePipeline
from .ledger import Ledger
from .macro.instances import load as load_instance
from .macro.instances import names as instance_names
from .server import make_server


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(prog="orchestrator")
    parser.add_argument("--port", type=int, default=8791)
    parser.add_argument("--fake", action="store_true",
                        help="wire the deterministic FakePipeline")
    parser.add_argument("--width", type=int, default=1600)
    parser.add_argument("--height", type=int, default=1200)
    parser.add_argument("--inbox", type=Path, default=DEFAULT_RUN_DIR / "inbox")
    parser.add_argument("--scene-root", type=Path, default=DEFAULT_RUN_DIR / "scene")
    parser.add_argument("--open-ui", action="store_true",
                        help="open the macro UI in the default browser")
    parser.add_argument("--instance", default="bridge-studio",
                        choices=instance_names(),
                        help="macro UI instance manifest to serve")
    args = parser.parse_args(argv)

    run_dir = DEFAULT_RUN_DIR
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "preview").mkdir(parents=True, exist_ok=True)
    token = secrets.token_urlsafe(8)
    (run_dir / "ui_token").write_text(token)
    ledger = Ledger(run_dir / "ledger.jsonl")
    if args.fake:
        pipeline = FakePipeline()
    else:
        try:
            from pipeline.container_pipeline import ContainerPipeline
            pipeline = ContainerPipeline()
        except ImportError:
            parser.error("real pipeline unavailable; pass --fake")
    from .scenes import SceneStore
    scenes = SceneStore(args.scene_root)
    app = Orchestrator(ledger, pipeline, (args.width, args.height), run_dir=run_dir,
                       scenes=scenes, archives_dir=run_dir / "archives")
    watcher = None
    if hasattr(pipeline, "import_asset"):
        from .watch import Watcher
        watcher = Watcher(args.inbox, scenes, pipeline.import_asset, ledger)
        watcher.start()
    server = make_server(app, host="127.0.0.1", port=args.port, ui_token=token,
                         ledger=ledger, scenes=scenes,
                         instance=load_instance(args.instance))
    url = f"http://127.0.0.1:{server.server_address[1]}/ui?key={token}"
    print(f"orchestrator on 127.0.0.1:{args.port} (fake={args.fake})", flush=True)
    print(f"macro UI: {url}", flush=True)
    if args.open_ui:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        if watcher is not None:
            watcher.stop()


if __name__ == "__main__":
    main()
