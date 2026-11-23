# PDM server image — bundles pdm-server, pdm-cli, a matching-arch solvecraft-cli
# (design.analyze + preview.render) and the newest Dutchman release
# (native CAD → SolidCraft conversion with feature history).
#
# Build context MUST be the parent directory that contains both repos,
# i.e. the directory holding `solidcraft/` and `solidcraft-pdm/`:
#
#   docker buildx build --platform linux/arm64,linux/amd64 \
#     -f solidcraft-pdm/docker/Dockerfile -t solvecraft-pdm:latest .
#
# (A .dockerignore at the context root skips .git and target/ dirs;
#  docker/.dockerignore in this repo is copied there by build.sh.)
#
# Cross-compilation: the builder stage always runs natively
# (--platform=$BUILDPLATFORM) and cross-compiles with cargo --target,
# so qemu emulation is only needed for the tiny final-stage apt install.

ARG BUILDPLATFORM

FROM --platform=$BUILDPLATFORM debian:trixie-slim AS web-editors
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
COPY solidcraft-pdm/docker/install-web-editors.sh /install-web-editors.sh
RUN sh /install-web-editors.sh /portal

FROM --platform=$BUILDPLATFORM rust:1.99-slim AS builder
ARG TARGETARCH
# The CAD workspace's CLI package (the PDM line names it solvecraft-cli; the
# SolidCraft-WPDM line names it solidcraft-cli): --build-arg CAD_PKG=solidcraft-cli.
ARG CAD_PKG=solidcraft-cli
WORKDIR /src

# CAD workspace (kernel + solvecraft-cli).
COPY solidcraft/ /solidcraft/
# PDM workspace (server + ops CLI).
COPY solidcraft-pdm/ /solidcraft-pdm/

# Install the cross C toolchain for the target arch (needed by cc crates
# like ring when compiling for a foreign target; harmless for native).
RUN apt-get update \
    && if [ "$TARGETARCH" = "amd64" ]; then \
         apt-get install -y --no-install-recommends \
           gcc-x86-64-linux-gnu g++-x86-64-linux-gnu binutils-x86-64-linux-gnu; \
       elif [ "$TARGETARCH" = "arm64" ]; then \
         apt-get install -y --no-install-recommends \
           gcc-aarch64-linux-gnu g++-aarch64-linux-gnu binutils-aarch64-linux-gnu; \
       fi \
    && rm -rf /var/lib/apt/lists/*

# Cross-compile env vars understood by the cc crate / rustc.
ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

# solvecraft-cli: invoked by the PDM worker for measurement/analysis and
# thumbnail rendering. Note: cache mounts are not part of the stage
# snapshot, so finished binaries are copied out of the cached target dir
# into /dist.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/solidcraft/target \
    case "$TARGETARCH" in \
      amd64) TRIPLET=x86_64-unknown-linux-gnu;; \
      arm64) TRIPLET=aarch64-unknown-linux-gnu;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH"; exit 1;; \
    esac \
    && rustup target add "$TRIPLET" \
    && cd /solidcraft \
    && cargo build --release --target "$TRIPLET" -p "$CAD_PKG" \
    && cp target/"$TRIPLET"/release/"$CAD_PKG" /dist-solidcraft-cli \
    && cp target/"$TRIPLET"/release/solvecraft-cli /dist-solvecraft-cli

# PDM server + ops CLI.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/solidcraft-pdm/target \
    case "$TARGETARCH" in \
      amd64) TRIPLET=x86_64-unknown-linux-gnu;; \
      arm64) TRIPLET=aarch64-unknown-linux-gnu;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH"; exit 1;; \
    esac \
    && rustup target add "$TRIPLET" \
    && cd /solidcraft-pdm \
    && cargo build --release --target "$TRIPLET" -p pdm-server -p pdm-cli \
    && cp target/"$TRIPLET"/release/pdm-server /dist-pdm-server \
    && cp target/"$TRIPLET"/release/pdm-cli /dist-pdm-cli

# Sparrow nesting engine (pinned commit; MIT, with MPL-2.0 jagua-rs). The worker
# shells out to this for laser.nest jobs. Built from the pinned revision so the
# input/output schema the PDM targets stays stable.
ARG SPARROW_REV=b8ef03312b7a195895f6c5332c6674dd78420ede
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    apt-get update && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && git clone https://github.com/JeroenGar/sparrow /sparrow \
    && cd /sparrow && git checkout --quiet "$SPARROW_REV" \
    && case "$TARGETARCH" in \
         amd64) T=x86_64-unknown-linux-gnu;; \
         arm64) T=aarch64-unknown-linux-gnu;; \
         *) echo "unsupported TARGETARCH: $TARGETARCH"; exit 1;; \
       esac \
    && cargo build --release --target "$T" -p sparrow-cli --bin sparrow --features only_final_svg \
    && cp /sparrow/target/"$T"/release/sparrow /dist-sparrow

# rust:1.99-slim is Debian 13 (glibc 2.41); the final stage must provide
# at least that glibc, so bookworm (glibc 2.36) is too old.
FROM debian:trixie-slim
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
# Dutchman Convert (native CAD → SolidCraft with feature history): the newest
# release at build time. The server keeps it current at runtime (boot check +
# interval, and on-demand per conversion job); PDM_DUTCHMAN_UPDATE=0 opts out.
# Best effort: a build without GitHub reachability still works — the first
# conversion pulls the binary itself.
COPY solidcraft-pdm/docker/install-dutchman.sh /install-dutchman.sh
RUN sh /install-dutchman.sh "$TARGETARCH" \
    || { echo "WARNING: dutchman not installed at build time; the server pulls it on demand"; rm -f /usr/local/bin/dutchman; }
COPY --from=builder /dist-solvecraft-cli /usr/local/bin/solvecraft-cli
COPY --from=builder /dist-solidcraft-cli /usr/local/bin/solidcraft-cli
COPY --from=builder /dist-pdm-server /usr/local/bin/pdm-server
COPY --from=builder /dist-pdm-cli /usr/local/bin/pdm-cli
COPY --from=builder /dist-sparrow /usr/local/bin/sparrow
# Web-workspace portal (index.html + app.js + style.css). lib.rs serves / from
# PDM_PORTAL_DIR when set, falling back to the embedded portal page.
COPY --from=builder /solidcraft-pdm/crates/pdm-api/static /opt/pdm/portal
COPY --from=web-editors /portal/editors /opt/pdm/portal/editors
ENV PDM_PORTAL_DIR=/opt/pdm/portal
ENV PDM_SOLVECRAFT_CLI=/usr/local/bin/solidcraft-cli
ENV PDM_DUTCHMAN_CLI=/usr/local/bin/dutchman
ENV PDM_SPARROW_CLI=/usr/local/bin/sparrow
EXPOSE 8080
# Server runs migrations on boot, then serves the API + in-process worker.
CMD ["pdm-server"]
