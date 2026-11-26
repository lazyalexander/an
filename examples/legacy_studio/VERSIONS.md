# Bridge Studio — pinned versions (reference machine)

| Component | Version | Source of truth |
| --- | --- | --- |
| Krita (painting carrier) | 5.3.4 | `/Applications/krita.app`; gate PASS evidence: `examples/legacy_studio/spikes/sidecar-pressure/RESULTS.md` |
| Blender (adjustment carrier + pipeline) | 5.2.3 LTS | `/Applications/Blender.app` (`Blender --version`); pipeline container must match major.minor |
| Python (orchestrator + tests) | 3.13.12 | `python3` on PATH (miniconda); shims target Krita's bundled Python 3.10 and Blender's bundled Python |
| PadMac (injector host) | 1.3.3 | `/Applications/PadMac.app`; TCC: grant Screen Recording + Accessibility; see `docs/architecture.md` |
| PadPad (injector client) | version on device | iPad; paired with PadMac during gate PASS 2026-11-22 |
| Apple Container (pipeline runtime) | 1.7.1 | `container --version`; service must be running (`container system status`); required for pipeline tasks (STUDIO_CONTAINER=1); amd64 runs use `--rosetta` |

Install notes live with their components: the Krita plugin installs via
`examples/legacy_studio/adapter/install.sh` (Task 7); the Blender addon installs as a symlink
so the package's `from .pose import` resolves (Task 8):
`ln -sfn "$PWD/examples/legacy_studio/blender_addon" "$HOME/Library/Application Support/Blender/5.2/scripts/addons/blender_addon"`,
then enable "Bridge Studio Pose Stream" in Edit → Preferences → Add-ons.
