FROM docker.io/library/rust:1-slim-bookworm

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy

# External-adapter binaries (goose, opencode): fetched on the host by
# scripts/fetch-adapter-binaries.sh with pinned sha256, so the build needs
# no network. Absent until fetched — the adapter canary then reports them
# as missing instead of failing the image build.
COPY container/bin/ /usr/local/bin/

WORKDIR /work

CMD ["bash"]
