#!/bin/sh
# Fetch the external-adapter binaries into container/bin/ — pinned version
# and sha256, so the image build itself stays offline and reproducible
# (doc/14: "extend the container image: goose, opencode"). The Containerfile
# COPYs this directory to /usr/local/bin.
#
# Run on the host; honors https_proxy when the network needs a proxy.
set -eu
cd "$(dirname "$0")/.."

dest=container/bin
mkdir -p "$dest"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() {
    name=$1
    url=$2
    sha=$3
    if [ -f "${dest}/${name}" ] && [ "${FORCE:-0}" != "1" ]; then
        echo "skip ${name} (already installed; FORCE=1 to refetch)"
        return 0
    fi
    echo "fetching ${name} from ${url}"
    curl --fail -L --retry 3 -o "${tmp}/${name}.download" "${url}"
    echo "${sha}  ${tmp}/${name}.download" | sha256sum -c - > /dev/null || {
        echo "sha256 mismatch for ${name}" >&2
        exit 1
    }
    mkdir -p "${tmp}/${name}"
    tar -xzf "${tmp}/${name}.download" -C "${tmp}/${name}"
    bin=$(find "${tmp}/${name}" -type f -name "${name}" | head -1)
    if [ -z "${bin}" ]; then
        echo "no ${name} binary inside the archive" >&2
        exit 1
    fi
    install -m 0755 "${bin}" "${dest}/${name}"
    echo "installed ${dest}/${name}"
}

# Pinned: opencode v1.19.6 (glibc build — the "musl" asset is dynamically
# linked against a musl loader that neither the host nor the debian image
# has) and goose v1.61.1.
fetch opencode \
    "https://github.com/anomalyco/opencode/releases/download/v1.19.6/opencode-linux-x64.tar.gz" \
    "47734e08968b30cc22a0bffb6d9feeed76ad4aa743213a12122040392d09a057"
fetch goose \
    "https://github.com/aaif-goose/goose/releases/download/v1.61.1/goose-x86_64-unknown-linux-gnu.tar.gz" \
    "f578c76447adf07f561201f1984318accc8f907f53e8bc07c350fe31b9c4264b"

echo "done:"
ls -la "${dest}"
