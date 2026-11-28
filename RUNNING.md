# Running Foundry PDM

Foundry PDM is the engineering-data vault (server + web portal + worker). SolveCraft
consumes it through the in-app **PDM panel** (see §4).

Layout assumption: the PDM repo and the SolveCraft repo are siblings
(`solidcraft-pdm/` + `solidcraft/`). The Docker build needs both, so run it from
the directory that contains them.

## 1. Start the stack (Postgres + MinIO + server)

```sh
cd <dir containing both repos>
docker compose -f solidcraft-pdm/docker/pdm-dev.yml up -d
```

| Service  | Host port | Purpose                          |
|----------|-----------|----------------------------------|
| pdm      | 8080      | API + web portal + worker        |
| postgres | 15432     | metadata (user/pass/db: pdm)     |
| minio    | 19000/9001→19001 | blob storage + console    |

Check: `curl http://localhost:8080/healthz` → `ok`

### First-time bootstrap

```sh
docker exec -it docker-pdm-1 pdm-cli create-org --name "Acme"
docker exec -it docker-pdm-1 pdm-cli create-user \
  --org Acme --email you@example.com --display-name "You" --role admin \
  --password "<password>"
```

Then open **http://localhost:8080/** and sign in with the email + password.
Signing in gives the portal a bearer token that expires (see `docs/docker-server.md`,
"Sign-in sessions"). For the REST API, scripts and CAD clients use an API token, which
an administrator creates in the portal or with `pdm-cli create-token`.

## 2. Multi-arch images (arm64 + amd64)

```sh
# both platforms -> solvecraft-pdm:latest (manifest list)
bash solidcraft-pdm/docker/build.sh

# single platform
bash solidcraft-pdm/docker/build.sh arm64
bash solidcraft-pdm/docker/build.sh amd64     # cross-compiled via buildx
```

The image bundles `pdm-server`, `pdm-cli`, and a matching-arch
`solvecraft-cli` (used by the worker for measurements + PNG thumbnails).
Push to your registry with `docker push solvecraft-pdm:<tag>` after building.

## 3. Web portal

- **Refresh behavior (web UI V.01-29):** pages use manual refresh by default so
  background updates cannot reset drafts, previews, menus or scroll position.
  Use the Refresh button to reload the current view. **Auto-refresh: Off/On**
  beside it is remembered in this browser. Opting in refreshes list views every
  six seconds; item details, Settings, Administration, and open dialogs/editors
  remain protected from timer-driven refreshes. Completed actions still update
  their view normally.

- Sign in (email/password) or connect with a `pdm_…` token. Sessions persist
  across refreshes; Sign out revokes the token.
- Onshape-style explorer: recently-modified cards, folders, items with model
  thumbnails, revision badges (A/B/… or WIP), part no / state / version /
  modified / modified-by / owner / lock columns, list ↔ grid switch,
  All/Mine/Recent filters, search, item detail with versions + audit.
- New Project / New Item open modal windows (add fields easily in
  `crates/pdm-api/src/portal.rs` → `openModal(...)` calls).
- Thumbnails: on check-in the worker runs `solvecraft-cli snapshot` and stores
  a 512px PNG (visible per version and per item).
- **Edit in SolidCraft web** (Preview tab, under the 3D model) opens the
  selected version in the SolidCraft CAD app bundled into the image and runs it
  in a same-origin iframe; **Save to PDM** checks the edited design in as a new
  immutable version. Convertible CAD versions are restated to a native design
  first. See `docs/web-editors.md`.

## 4. SolveCraft with the PDM panel

### Desktop

```sh
cd <...>/solidcraft
cargo run -p solvecraft          # add --release for a faster UI
```

1. Click **PDM** in the top app bar (right of Home) — the panel opens on the left.
2. Enter the server (default `http://127.0.0.1:8080`; use
   `http://<host>:8080` if the PDM stack runs elsewhere) and an API token
   (an administrator creates one for you in the portal's Administration, or with
   `pdm-cli create-token`; a portal sign-in expires, so don't paste that), or prefill both via env:

   ```sh
   export SOLVECRAFT_PDM_SERVER=http://127.0.0.1:8080
   export SOLVECRAFT_PDM_TOKEN=pdm_...
   ```

3. **Connect** — pick a project; items appear with model thumbnails, name,
   part number, revision badge, last-modified-by, and lock state.
4. **Open** — downloads the head version into the local PDM workspace
   (`$TMPDIR/solvecraft-pdm/<org>/<item>/design.solvecraft`) and opens it in the
   editor. The panel shows a pinned "open item" card with **Check in** /
   **Undo check out** and a local-modification hint.
5. **Check out / Check in** — the commit-message box in the panel is used for
   check-in; the server validates the base version and blob SHA-256, bumps the
   version, and re-renders the thumbnail.

### Web (wasm)

```sh
cd <...>/solidcraft/apps/solvecraft-web
trunk serve    # dev server on port 8781 (Trunk.toml)
```

The panel renders and Connect works in the browser; heavy flows are best on
the desktop build.

## 5. Ops

- Logs: `docker logs -f docker-pdm-1`
- Tear down (keep data): `docker compose -f solidcraft-pdm/docker/pdm-dev.yml down`
- Tear down (drop data too): add `-v`
- Data: Postgres in the `pgdata` volume; blobs in MinIO's `pdm` bucket
  (`miniodata` volume). Back up both volumes for a consistent restore.
- E2E acceptance test (PDM side): `bash solidcraft-pdm/e2e.sh` — boots a clean
  Postgres, runs the full vault flow incl. summary fields; leaves no data.

## 6. Updating PDM

Releases live on a configured Gitea repository, tagged like
the build tags (`V.01-37`). Everything below uses a Gitea bearer token in
`PDM_UPDATE_TOKEN` (the API is 403 without one); without it the commands print
a clear "token missing or wrong" line and `update check` exits `3`.

```sh
export PDM_UPDATE_BASE_URL=https://gitea.example.com
export PDM_UPDATE_REPO=owner/solidcraft-pdm
export PDM_UPDATE_TOKEN=<gitea token>
pdm-cli update check            # "update available: V.01-37 (local V.01-36)" or
                                # "up to date: V.01-37 (local …)"; exit 0 either way
pdm-cli update check --json     # {"local","latest","available","url","server"}
pdm-cli update apply --yes      # download the matching asset and replace this pdm-cli
```

- `local` is `PDM_BUILD_TAG` when set (deployment builds), else the crate
  version (from-source builds, `0.1.0`). With `--database-url` /
  `PDM_DATABASE_URL` present, `check` also reports the running server's build
  tag from `GET /version` (`PDM_API_URL`, default `http://127.0.0.1:8080`) —
  best effort, never fatal.
- Exit codes: `0` up to date / update available / no releases yet, `1`
  network, parse or asset error, `3` token.
- `apply` picks the asset whose name matches this OS/arch
  (`linux|windows|macos` + `x86_64|aarch64|arm64`), only accepts URLs under
  the repo's `releases/` (or `releases_download/`) paths, caps the download
  at 200 MB, and for a `.tar.gz` extracts `pdm-cli` and replaces the current
  executable with a same-directory rename; the old binary is kept beside it
  as `pdm-cli.old-<timestamp>`. It asks `replace …? [y/N]` unless `--yes`.
- A copy managed by a launcher (`SOLIDCRAFT_MANAGED_BY=<name>`, or an
  `installed.json` with a `manager`/`managed_by` entry next to the executable)
  never replaces itself: `apply` prints "update this copy in \<manager\>
  (the launcher manages its CLI)".
- The server checks once a day in the background at boot (marker file
  `.pdm-update-check` next to the local data dir when `PDM_STORAGE_LOCAL` is
  set; in-memory otherwise) and is silent on failure. `PDM_UPDATE_CHECK=0`
  disables it; `PDM_UPDATE_BASE_URL` and `PDM_UPDATE_REPO` select the Gitea
  server and repository. Set both to your own release source.
  When a newer release exists the server logs
  `PDM update available: <tag> (running <build>); pdm-cli update check` and
  answers `GET /version` with the additive fields `latest` (tag or null) and
  `update_available` (bool); the portal's System panel shows an Update row
  that links to the release page.

### Updating the bundled CAD CLI (image level)

`bash docker/build-dev.sh` ships the PDM code *and* rebuilds the bundled
SolidCraft CLI from the sibling CAD checkout. To ship a new CAD CLI without
rebuilding the PDM code (the PDM repo and the CLI moved independently — the
Inventor `.ipt` import, for instance, came from the CAD line), layer it on the
last PDM image:

```sh
# 1. Build the CLI in the CAD checkout (both names: the fork ships
#    `solidcraft-cli` plus a `solvecraft-cli` alias that execs it).
cd <cad-checkout> && cargo build --release -p solidcraft-cli
# 2. One image per new CLI: keep the PDM API/portal, replace /usr/local/bin.
docker build -f solidcraft-pdm/docker/Dockerfile.cli-update \
  --build-arg PDM_BASE_IMAGE=docker-pdm:<last tag> \
  --build-arg PDM_BUILD_TAG=<next tag> \
  --build-arg SOLIDCRAFT_VERSION=<cli version> \
  --build-arg SOLIDCRAFT_FORK_REV=<cad commit> \
  -t docker-pdm:<next tag> <cad-checkout>/target/release
# 3. Point the stack at it and verify the CLI inside the worker.
echo "<next tag>" > solidcraft-pdm/docker/.build-tag
cd solidcraft-pdm && PDM_BUILD_TAG=<next tag> \
  docker compose -f docker/pdm-dev.yml up -d --force-recreate pdm pdm-worker
docker exec docker-pdm-worker-1 /usr/local/bin/solidcraft-cli --version
```

The image bakes the CLI's provenance into labels (`io.solidcraft.cli.version`,
`io.solidcraft.cli.fork-revision`, `io.solidcraft.cli.upstream-revision`);
`GET /system-info` reports the running CLI's `--version` under
`solidcraft_cli`. Note the version line: the CAD fork reports `0.27.1` while
carrying features (Inventor editable-history import) that postdate the
upstream `0.29.x` releases it syncs from — compare behaviour, not the number.

### Keeping the bundled tools current (runtime)

The CAD CLI, Dutchman (the native-CAD → SolidCraft feature-tree converter) and
the SolidCraft web editor ("Edit in SolidCraft web") all auto-update at
runtime from their public GitHub repos, in the background, on boot and on an
interval — no reboot, no image rebuild:

- **CAD CLI** — `PDM_CLI_UPDATE=0` opts out; `PDM_CLI_UPDATE_HOURS` (default 6).
- **Dutchman** — the PDM converts native CAD (Fusion, Inventor, SolidWorks,
  FreeCAD, CATIA, Rhino, Creo View, ACIS, STEP, Onshape bundles) with a
  [dutchman-cad/dutchman](https://github.com/dutchman-cad/dutchman) release
  picked from a **channel** (System › Dutchman, one per server, set by a host admin): the newest stable
  (`latest`), `nightly`, the `onshape-preview` builds (Onshape import), or a
  pinned tag. The image bakes the newest stable in at build
  (`docker/install-dutchman.sh`); the server re-checks on boot and hourly
  (`PDM_DUTCHMAN_UPDATE_HOURS`) — chasing the referenced release, recorded in
  a `.info` file beside the binary; and a conversion job pulls it on demand
  when the binary is missing or from another channel. `PDM_DUTCHMAN_UPDATE=0`
  opts out; `PDM_DUTCHMAN_CLI` names the binary (default
  `/usr/local/bin/dutchman`); `PDM_DUTCHMAN_CHANNEL` overrides the server's choice
  for the host's update loop; `PDM_DUTCHMAN_TIMEOUT_SECS` (default 1800)
  bounds one conversion (the org's settings field overrides it).
- **Onshape import** — project Settings → "Import from Onshape (Dutchman)"
  (`POST /projects/{id}/onshape-import`): the worker's `onshape.import` job
  fetches a Part Studio with the org's Onshape API keys
  (`dutchman onshape fetch`, keys passed as `ONSHAPE_ACCESS_KEY` /
  `ONSHAPE_SECRET_KEY`), checks the `.onshape-bundle` in as a version, and
  enqueues the `cad.solidcraft` conversion. The referenced Dutchman build must
  carry the Onshape reader (Onshape-preview or nightly channel); large Part
  Studios (over the reader's 256 MiB expanded-bundle limit) fail with a clear
  report.
- **SolidCraft web** — the server downloads the newest
  `solidcraft-web-<version>.zip` from
  [tangency-labs/solidcraft](https://github.com/tangency-labs/solidcraft)
  releases, checks its SHA-256 against the release's `SHA256SUMS.txt`, and
  swaps it into the portal's `editors/solidcraft/`. `PDM_WEB_UPDATE=0` opts
  out; `PDM_WEB_UPDATE_HOURS` (default 6). It only ever moves *up* (a
  same-or-older release is a no-op), so a bad release is retried, not
  downgraded. Open editors pick up a new build on the next open.

The System panel shows installed vs latest for each, with a check/update
button. `GET /settings/solidcraft-web` (and `POST /settings/solidcraft-web/update`)
expose the same; `GET /settings/cad-cli` (and `.../update`) for the CLI, and
`GET/PUT /settings/dutchman` (and `POST .../update`) for Dutchman's channel
and conversion settings. The image still
pins known-good starting builds (`docker/install-web-editors.sh`,
`docker/install-dutchman.sh`), so a fresh container is functional even with
GitHub unreachable.

## 7. API quick reference

`GET /me`, `POST /auth/login` `{username,password,org?}`, `POST /auth/logout`,
`GET/POST /projects`, `GET/POST /projects/{id}/folders`,
`GET/POST /projects/{id}/items`, `GET /items/{id}`,
`POST/DELETE /items/{id}/checkout`, `POST /items/{id}/checkin`,
`PUT /blobs`, `GET /versions/{id}/blob`, `GET /versions/{id}/thumbnail`,
`GET /items/{id}/thumbnail`, `GET /items/{id}/versions`,
`GET /search?q=…`, `GET /audit`, `POST /items/{id}/release`.
Auth: `Authorization: Bearer pdm_…`.
