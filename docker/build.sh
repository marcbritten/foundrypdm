#!/usr/bin/env bash
#
# Build the PDM server image (bundles a matching-arch solvecraft-cli).
#
# Usage:
#   build.sh            multi-arch: linux/arm64 + linux/amd64
#   build.sh arm64      only linux/arm64
#   build.sh amd64      only linux/amd64
#
# Environment:
#   TAG  image tag (default: solvecraft-pdm:latest)
#   CTX  build context dir (default: parent of solidcraft-pdm)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
CTX="${CTX:-$(dirname "$REPO_DIR")}"
DOCKERFILE="$SCRIPT_DIR/Dockerfile"
TAG="${TAG:-solvecraft-pdm:latest}"

command -v docker >/dev/null 2>&1 || { echo "ERROR: docker not found" >&2; exit 1; }

# --- native arch -----------------------------------------------------------
native_arch() {
  case "$(uname -m)" in
    x86_64)        echo amd64 ;;
    aarch64|arm64) echo arm64 ;;
    *)             echo "$(uname -m)" ;;
  esac
}

# --- ensure buildx ---------------------------------------------------------
if ! docker buildx version >/dev/null 2>&1; then
  echo ">> buildx plugin not found, installing..."
  mkdir -p "${HOME}/.docker/cli-plugins"
  case "$(uname -m)" in
    x86_64)        _b=amd64 ;;
    aarch64|arm64) _b=arm64 ;;
    *)             _b="$(uname -m)" ;;
  esac
  # Resolve the latest buildx release tag (assets are version-pinned).
  _v="$(curl -fsSL https://api.github.com/repos/docker/buildx/releases/latest \
    | sed -n 's/.*"tag_name": *"v\([0-9.]*\)".*/\1/p' | head -1)"
  _v="${_v:-0.38.0}"
  curl -fsSL "https://github.com/docker/buildx/releases/download/v${_v}/buildx-v${_v}.linux-${_b}" \
    -o "${HOME}/.docker/cli-plugins/docker-buildx"
  chmod +x "${HOME}/.docker/cli-plugins/docker-buildx"
fi

BUILDER="pdm-multiarch"
docker buildx inspect "$BUILDER" >/dev/null 2>&1 || \
  docker buildx create --name "$BUILDER" --driver docker-container --bootstrap

# --- qemu binfmt for cross builds -----------------------------------------
# Best effort: buildx >= 0.12 can cross-build via its own binary emulation,
# but registering qemu host-wide keeps `docker run --platform linux/amd64`
# working too. The helper image is amd64-only; use the host-arch tag when
# the host is not amd64.
ensure_binfmt() {
  [ "$1" = "$(native_arch)" ] && return 0
  echo ">> Registering qemu binfmt handlers for cross-building $1..."
  local tag="latest"
  if [ "$(native_arch)" = "arm64" ]; then tag="x86_64-aarch64"; fi
  docker run --rm --privileged "multiarch/qemu-user-static:${tag}" --reset -k >/dev/null 2>&1 || {
    echo "WARN: could not register qemu binfmt; buildx may fall back to its" >&2
    echo "WARN: built-in binary emulation (builds work, docker run cross-arch won't)." >&2
  }
}

# --- .dockerignore at context root -----------------------------------------
# BuildKit reads .dockerignore only from the context root. Copy the repo's
# copy in place so .git and target/ dirs are skipped.
if [ -f "$SCRIPT_DIR/.dockerignore" ] && [ -d "$CTX" ]; then
  cp "$SCRIPT_DIR/.dockerignore" "$CTX/.dockerignore"
fi

# --- build -----------------------------------------------------------------
build_single() {
  local plat="$1" dest="$2"
  ensure_binfmt "${plat#linux/}"
  docker buildx build --builder "$BUILDER" \
    --platform "$plat" \
    -f "$DOCKERFILE" \
    -t "$dest" \
    --load \
    "$CTX"
}

case "${1:-}" in
  arm64|amd64)
    ensure_binfmt "$1"
    build_single "linux/$1" "$TAG"
    echo ">> Built $TAG (linux/$1)"
    ;;
  "")
    PLATFORMS="linux/arm64,linux/amd64"
    for p in arm64 amd64; do ensure_binfmt "$p"; done

    echo ">> Building multi-arch $TAG ($PLATFORMS)..."
    if docker buildx build --builder "$BUILDER" \
         --platform "$PLATFORMS" \
         -f "$DOCKERFILE" \
         -t "$TAG" \
         --load \
         "$CTX"; then
      echo ">> Multi-arch image loaded: $TAG"
    else
      echo ">> --load multi-arch not supported by this buildx; falling back to per-arch + manifest."
      A="${TAG}-arm64"
      M="${TAG}-amd64"
      build_single linux/arm64 "$A"
      build_single linux/amd64 "$M"
      docker manifest create -a "$TAG" "$A" "$M"
      docker manifest annotate "$TAG" "$A" --arch arm64 --os linux
      docker manifest annotate "$TAG" "$M" --arch amd64 --os linux
      docker manifest inspect "$TAG"
      echo ">> Multi-arch manifest created: $TAG (sources $A, $M)"
    fi
    ;;
  *)
    echo "Usage: $0 [arm64|amd64]" >&2
    exit 1
    ;;
esac
