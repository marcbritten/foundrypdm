#!/bin/sh
# Install the newest Dutchman Convert release (the native CAD → SolidCraft
# feature-tree converter) at image build time. Usage:
#   sh docker/install-dutchman.sh [arch] [dest]      arch: x86_64 | aarch64
# The image is only a starting point: at runtime the server keeps pulling the
# newest release on its own (System › Dutchman; PDM_DUTCHMAN_UPDATE=0 opts out),
# and a conversion job installs it on demand when the binary is missing.
set -eu
arch=${1:-$(uname -m)}
dest=${2:-/usr/local/bin/dutchman}
case "$arch" in
  x86_64|amd64) target=x86_64-unknown-linux-gnu ;;
  aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
  *) echo "install-dutchman: unsupported arch '$arch'" >&2; exit 1 ;;
esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
tag=$(curl --fail --silent --location \
    "https://api.github.com/repos/dutchman-cad/dutchman/releases/latest" \
  | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)
[ -n "$tag" ] || { echo "install-dutchman: no releases published yet" >&2; exit 1; }
version=${tag#v}
base="https://github.com/dutchman-cad/dutchman/releases/download/$tag"
# The versioned asset carries its .sha256 sidecar; the unversioned README name
# is the fallback.
name="dutchman-$version-$target.tar.gz"
curl --fail --location --retry 3 --output "$work/$name" "$base/$name" \
  || { name="dutchman-$target.tar.gz"; curl --fail --location --retry 3 --output "$work/$name" "$base/$name"; }
curl --fail --location --retry 3 --output "$work/$name.sha256" "$base/$name.sha256"
want=$(head -n1 "$work/$name.sha256" | cut -d' ' -f1 | tr 'A-F' 'a-f')
got=$(sha256sum "$work/$name" | cut -d' ' -f1)
[ "$want" = "$got" ] || { echo "install-dutchman: sha256 mismatch ($want != $got)" >&2; exit 1; }
tar xzf "$work/$name" -C "$work"
bin=$(find "$work" -type f -name dutchman | head -n1)
[ -n "$bin" ] || { echo "install-dutchman: archive holds no dutchman binary" >&2; exit 1; }
mkdir -p "$(dirname "$dest")"
install -m 0755 "$bin" "$dest"
echo "install-dutchman: $tag installed at $dest"
