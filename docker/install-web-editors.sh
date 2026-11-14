#!/bin/sh
# Install pinned, unmodified upstream web releases, including their licence notices.
# Usage: sh docker/install-web-editors.sh [portal-static-directory]
set -eu
destination=${1:-crates/pdm-api/static}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
# install_editor APP VERSION CHECKSUM [GITHUB-OWNER]
# Fetches $OWNER/$APP release v$VERSION, asset $APP-web-$VERSION.zip, and installs
# it under editors/$APP/. Owner defaults to "storytold" (the Craft editor line);
# SolidCraft web ships from "tangency-labs".
install_editor() {
    app=$1 version=$2 checksum=$3
    owner=${4:-storytold}
    archive="$work/$app.zip"
    curl --fail --location --retry 3 --output "$archive" \
        "https://github.com/$owner/$app/releases/download/v$version/$app-web-$version.zip"
    printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --status
    unzip -q "$archive" -d "$work/$app"
    mkdir -p "$destination/editors/$app"
    cp -R "$work/$app/$app-web-$version/." "$destination/editors/$app/"
}
install_editor pdfcraft 0.15.0 fdb2508030d7668cb5164db3b7d05baa1011a87497c95ef043844d90cbbc4da2
install_editor cadcraft 0.14.0 f2fbfb80388dc0315a469d65cf7c966b742f86b6b4d3d405752bdee7428d94e8
# SolidCraft web: the upstream release zip is the same verified build the GPU web
# lab (solidcraft-web-lab) mirrors and smoke-tests; it carries its own build.json
# manifest, service worker, fonts, licences and attribution. The portal runs it in
# a same-origin iframe under /static/editors/solidcraft/ ("Edit in SolidCraft web").
# This pin is the image's starting point: at runtime the server tracks the newest
# release on its own (System › SolidCraft web editor; PDM_WEB_UPDATE=0 to opt out).
install_editor solidcraft 0.40.1 08a1537c4e2cbfd129f2cabc4f209e2e25da4529075d268b639a75a0a72fad6e tangency-labs
