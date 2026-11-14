#!/usr/bin/env bash
# Rebuild and redeploy the local PDM stack (web portal + API server + worker)
# as the next build version: bumps PDM_BUILD_TAG (V.01-NN) from the running
# container (or docker/.build-tag), bakes this checkout's commit into
# GET /version, and recreates the containers. This is the "add a version of
# the webui and backend server" step — each run ships one new, testable
# version; the running stack always reflects the last build.
#
#   bash docker/build-dev.sh                 # next version, default CAD checkout
#   PDM_BUILD_TAG=V.01-42 bash ...           # pin a specific version
#   CAD_DIR=solidcraft CAD_CLI_PKG=solvecraft-cli bash ...        # older CAD repo
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

PDM_GIT_COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"

# ── next build tag ────────────────────────────────────────────────────────
bump_tag() {
  local tag="${1:-}"
  local re='^(.*)-([0-9]+)$'
  if [[ -z "$tag" || ! "$tag" =~ $re ]]; then
    echo "V.01-01"
    return
  fi
  echo "${BASH_REMATCH[1]}-$(( 10#${BASH_REMATCH[2]} + 1 ))"
}
last_tag=""
if docker inspect docker-pdm-1 >/dev/null 2>&1; then
  last_tag="$(docker inspect docker-pdm-1 --format '{{range .Config.Env}}{{println .}}{{end}}' | sed -n 's/^PDM_BUILD_TAG=//p' | head -1)"
elif [[ -f docker/.build-tag ]]; then
  last_tag="$(cat docker/.build-tag)"
fi
PDM_BUILD_TAG="${PDM_BUILD_TAG:-$(bump_tag "$last_tag")}"

# BuildKit reads .dockerignore only from the context root (the parent dir
# that holds the CAD checkout and this repo). Skip silently where the context
# root is not writable (e.g. this box mounts the repos one level shallower).
CTX="$(cd ../.. && pwd)"
if [[ -f docker/.dockerignore && -w "$CTX" ]]; then
  cp docker/.dockerignore "$CTX/.dockerignore"
fi

echo "[build-dev] baking commit ${PDM_GIT_COMMIT}, build tag ${PDM_BUILD_TAG} into the image…"
PDM_GIT_COMMIT="$PDM_GIT_COMMIT" PDM_BUILD_TAG="$PDM_BUILD_TAG" \
  docker compose -f docker/pdm-dev.yml up -d --build pdm pdm-worker
echo "$PDM_BUILD_TAG" > docker/.build-tag

# ── verify ────────────────────────────────────────────────────────────────
for _ in $(seq 1 60); do
  curl -sf http://127.0.0.1:8080/healthz >/dev/null 2>&1 && break
  sleep 1
done
curl -sf http://127.0.0.1:8080/healthz >/dev/null
echo "[build-dev] server:      $(curl -s http://127.0.0.1:8080/version)"
echo "[build-dev] worker log:  docker logs docker-pdm-worker-1 2>&1 | grep 'worker started'"
echo "[build-dev] done — check: http://127.0.0.1:8080/  (GET /version reports ${PDM_BUILD_TAG})"
