# Foundry PDM — Docker server

For the self-contained default setup (PostgreSQL and local persistent file
storage), start with the [README](../README.md#set-up-a-docker-server). The
sections below describe the separate advanced development stack with MinIO and
the CAD CLI; its build requires a sibling CAD checkout and a GPU device mapping.

## Layout

Build context is the **parent** of this repo and the CAD checkout (sibling dirs):

```
<workspace>/
  solidcraft-pdm/     # this repo
  solidCraft-WPDM/    # required by this advanced image build
```

## Quick start

```sh
cd <workspace>
docker compose -f solidcraft-pdm/docker/pdm-dev.yml up -d --build
curl http://localhost:8080/healthz   # → ok
```

| Service     | Host port        | Role                         |
|-------------|------------------|------------------------------|
| pdm         | 8080             | API + web portal             |
| pdm-worker  | (internal 8081)  | jobs: preview, analyze       |
| postgres    | 15432            | metadata                     |
| minio       | 19000 / 19001    | blobs + console              |

### First org and admin

```sh
docker compose -f solidcraft-pdm/docker/pdm-dev.yml exec pdm pdm-cli create-org --name "Acme"
docker compose -f solidcraft-pdm/docker/pdm-dev.yml exec pdm pdm-cli create-user \
  --org Acme --email you@example.com --display-name "You" --role admin \
  --password "<password>"
```

Open http://localhost:8080/ and sign in.

## Environment (DB and storage)

Set these on **pdm** and **pdm-worker**. Compose already sets them for the bundled Postgres/MinIO.

| Variable | Example | Meaning |
|----------|---------|---------|
| `PDM_DATABASE_URL` | `postgresql://pdm:pdm@postgres:5432/pdm` | Postgres connection string |
| `PDM_S3_ENDPOINT` | `http://minio:9000` | S3-compatible API |
| `PDM_S3_BUCKET` | `pdm` | Bucket name (create it first) |
| `PDM_S3_ACCESS_KEY` | `pdm` | S3 access key |
| `PDM_S3_SECRET_KEY` | `pdmsecret` | S3 secret |
| `PDM_BUILD_TAG` | `V.01-37` | Shown in the portal / `/version` |
| `PDM_SOLVECRAFT_CLI` | `/usr/local/bin/solidcraft-cli` | CLI for snapshots |
| `PDM_DUTCHMAN_CLI` | `/usr/local/bin/dutchman` | Dutchman binary (native CAD → SolidCraft with feature history) |
| `PDM_DUTCHMAN_UPDATE` | `1` | Pull the referenced Dutchman release (boot + hourly + on first conversion); `0` opts out |
| `PDM_DUTCHMAN_UPDATE_HOURS` | `1` | Hours between Dutchman release checks |
| `PDM_DUTCHMAN_CHANNEL` | — | Host-level channel override for the update loop (`latest`, `nightly`, or a tag); the org's System › Dutchman choice otherwise wins |
| `PDM_DUTCHMAN_TIMEOUT_SECS` | `1800` | Wall-clock limit for one Dutchman conversion (the org can override it) |
| `PDM_WORKER` | `0` on API, `1` on worker | Whether this process claims jobs |
| `PDM_WORKER_CONCURRENCY` | `4` | Parallel jobs on the worker |
| `PDM_LOCK_LEASE_HOURS` | `24` | Check-out lease |
| `PDM_MAX_UPLOAD_BYTES` | `2147483648` | Upload cap (2 GiB) |
| `SOLIDCRAFT_GPU` | `auto` | `auto` / `hardware` / `software` / `cpu` |
| `PDM_PORTAL_DIR` | `/opt/pdm/portal` | Static web UI (index.html, app.js) |

### External Postgres

Point `PDM_DATABASE_URL` at your server, e.g.

```
postgresql://pdm:<password>@db.example.com:5432/pdm
```

The process runs migrations on boot. The DB user needs permission to create tables.

### External S3

Set `PDM_S3_*` to AWS, MinIO, Garage, etc. Create the bucket before starting PDM.

## GPU (thumbnails)

The worker uses Vulkan when `/dev/dri` is visible. Compose passes `/dev/dri` and `SOLIDCRAFT_GPU=auto`. On hosts without a GPU, it falls back to CPU. Org setting **Settings → Preview renderer** can override (`auto` / `GPU` / `GPU software` / `CPU`).

If compose fails on a machine with no `/dev/dri`, remove the `devices:` block from `docker/pdm-dev.yml`.

## CAD → SolidCraft conversion (Dutchman)

**Convert to SolidCraft** (item versions tab, "Import to SolidCraft" in the
item right-click / ⋮ menus, and "Edit in SolidCraft web" on a native version)
runs the worker's `cad.solidcraft` job. **Import from Onshape**
(project Settings → "Import from Onshape (Dutchman)", `POST
/projects/{id}/onshape-import`) fetches a Part Studio with the org's Onshape
API keys through `dutchman onshape fetch` (the referenced build must carry the
Onshape reader — pick the *Onshape preview* or *Nightly* channel in System ›
Dutchman), checks the `.onshape-bundle` in as a version, and runs the same
conversion.

To import and convert a Part Studio:

1. Configure the organization's Onshape access key and secret in **Settings → Onshape**.
2. Select and update a Dutchman build with the Onshape reader in **System → Dutchman**.
3. Open the destination project's **Settings → Import from Onshape (Dutchman)**.
4. Paste the Part Studio's workspace or version URL and optionally enter a name / part number.
5. Leave **Check in the converted SolidCraft design** selected and click **Import**.
   PDM saves the source bundle, converts it, then checks in the `.sc` design as
   another version. If unchecked, the source bundle and derived conversion
   remain available without checking in the `.sc` design.

Folder sync does not need to be enabled. Re-importing the same Part Studio
updates its existing item; a link already associated with another project
returns a conflict. Import and conversion jobs are visible in **Tasks**.

### Manual Onshape STEP fallback

If direct import fails, export the desired Part Studio from Onshape as STEP.
In the same project Settings section, choose the `.step` or `.stp` file under
**Manual STEP fallback** and click **Upload STEP**. Keep **Check in the converted
SolidCraft design** selected to also convert and check in the `.sc` result.
This path does not require Onshape credentials or an Onshape-capable Dutchman
build; it uses the ordinary STEP converter and preserves geometry rather than
the original Onshape feature history.

The Part Studio URL is optional for manual uploads. Supply it to reuse the
item linked to that document element, including an empty item created by a
failed direct import. With no link, PDM creates a new item. The STEP remains
available if conversion fails; retry **Convert to SolidCraft** on its version.

If Tasks reports `unrecognized subcommand 'onshape'`, the installed Dutchman
does not have the Onshape reader. In particular, stable **0.2.10** does not
provide this command. Select **onshape-preview-2** in **System → Dutchman**, save
and update, then retry the URL import. A current stable release is not necessarily
an Onshape-capable release. The import page keeps the job ID and failure message
visible alongside the STEP fallback.

Dutchman runs on a **release channel** chosen per org in **System ›
Dutchman** (dropdown: latest stable, nightly, the Onshape-preview builds, or a
pinned tag). The PDM keeps the host on that channel: at boot, hourly, and on
the first conversion that references it. Conversion options (preset, part
positions, history verification, timeouts, Onshape call budget) are the same
panel's "Conversion settings" fields.

The `cad.solidcraft` job:

- Native CAD formats — Fusion (`.f3d`/`.f3z`), Inventor (`.ipt`/`.iam`),
  SolidWorks (`.sldprt`/`.sldasm`), FreeCAD (`.FCStd`), CATIA (`.CATPart`),
  Rhino (`.3dm`), Creo View (`.pvz`), ACIS (`.sat`/`.sab`/`.smt`/`.smb`) and
  STEP (`.step`/`.stp`) — convert through [Dutchman
  Convert](https://github.com/dutchman-cad/dutchman), which decodes the native
   file into a SolidCraft design **with editable feature history where it
   maps** (Onshape bundles `.onshape-bundle` the same way, through a build with
   the Onshape reader). The PDM keeps the host on the referenced release: the
   image bakes the newest stable in at build time
   (`docker/install-dutchman.sh`), the server re-checks GitHub on boot and
   hourly, and a conversion job pulls the referenced release on demand when the
   binary is missing or from another channel. `PDM_DUTCHMAN_UPDATE=0` opts out
   (offline hosts); `PDM_DUTCHMAN_TIMEOUT_SECS` (default 1800) bounds one
   conversion (the org's Settings field overrides it).
- Meshes (`.stl`/`.obj`/`.3mf`/`.glb`/`.gltf`) and IGES (`.igs`/`.iges`)
  restate through the SolidCraft CLI (`PDM_SOLVECRAFT_CLI`) as geometry only.
- System › **Dutchman (feature-tree converter)** shows the channel menu,
  installed build and conversion settings, with a check/update button
  (`GET/PUT /settings/dutchman`, `POST /settings/dutchman/update`).

The source version is never modified: the conversion is stored as a derived
artifact and can be checked in as a new version.

## Image build

```sh
# from <workspace>
bash solidcraft-pdm/docker/build.sh          # arm64 + amd64
PDM_BUILD_TAG=V.01-37 docker compose -f solidcraft-pdm/docker/pdm-dev.yml up -d
```

`docker/Dockerfile.dev` is the compose build. Runtime image needs `ca-certificates`, `curl`, and (for GPU) `mesa-vulkan-drivers` `libvulkan1`.

## After boot — Settings in the portal

These are **not** env vars; they live in the org (Admin → Settings):

- **Mail server** — SMTP host, port, user, password, from, STARTTLS/TLS. Required for **Invite user**.
- **Onshape** — API access key + secret, default PDM project. Required for **Send to manufacturing**.
- **Preview renderer** — GPU policy for the worker.
- **Part-number prefixes** — auto part numbers.

Do not put SMTP or Onshape secrets in git.

### Onshape extension Action URL

```
https://<your-host>/static/onshape.html?documentId={$documentId}&workspaceId={$workspaceOrVersionId}&elementId={$elementId}&server=https://cad.onshape.com
```

Register an OAuth app in Onshape → Element right panel → that URL. OAuth client secret is stored in org settings after you paste it once.

## Health

```
GET /healthz     → ok
GET /version     → build tag
```
