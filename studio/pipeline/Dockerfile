# Blender ships no linux-arm64 build; use the official linux-x64 tarball under
# Docker Desktop's amd64 emulation on Apple Silicon (ledger Ruling 2026-11-22).
FROM --platform=linux/amd64 ubuntu:24.04

ARG BLENDER_VERSION=5.2.3
ARG BLENDER_BASE=https://download.blender.org/release/Blender5.2

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl xz-utils \
      libx11-6 libxi6 libxrender1 libxfixes3 libxkbcommon0 libsm6 libice6 \
      libxxf86vm1 libgl1 \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSLO "$BLENDER_BASE/blender-$BLENDER_VERSION-linux-x64.tar.xz" \
    && curl -fsSLO "$BLENDER_BASE/blender-$BLENDER_VERSION.sha256" \
    && grep "linux-x64.tar.xz$" "blender-$BLENDER_VERSION.sha256" | sha256sum -c - \
    && tar -xJf "blender-$BLENDER_VERSION-linux-x64.tar.xz" -C /opt \
    && ln -s "/opt/blender-$BLENDER_VERSION-linux-x64/blender" /usr/local/bin/blender \
    && rm "blender-$BLENDER_VERSION-linux-x64.tar.xz" "blender-$BLENDER_VERSION.sha256"

ENTRYPOINT ["blender", "--background"]
