#!/usr/bin/env bash
# Install the Bridge Studio Krita plugin as a symlink into pykrita,
# so the running plugin is always the repo's governed code.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
PYKRITA="$HOME/Library/Application Support/krita/pykrita"

mkdir -p "$PYKRITA"
ln -sfn "$REPO/studio/adapter/krita_plugin/bridge_studio" "$PYKRITA/bridge_studio"
cp "$REPO/studio/adapter/krita_plugin/bridge_studio.desktop" "$PYKRITA/bridge_studio.desktop"

echo "Installed into: $PYKRITA"
echo "Next: restart Krita, enable 'Bridge Studio' in"
echo "  Settings → Configure Krita → Python Plugin Manager,"
echo "then open the panel via Settings → Dockers → Bridge."
