FROM docker.io/library/rust:1-slim-bookworm

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy

WORKDIR /work

CMD ["bash"]
