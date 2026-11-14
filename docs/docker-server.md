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
