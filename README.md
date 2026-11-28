# Foundry PDM

Self-hosted vault for engineering data: versioned CAD files, check-in/out, BOM
metadata, a web portal, and CAD connectors (including Onshape → manufacturing).

**Docker setup:** this file. Advanced options: [`docs/docker-server.md`](docs/docker-server.md).
**What changed:** [`CHANGELOG.md`](CHANGELOG.md).

---

## Set up a Docker server

Install Docker Engine or Docker Desktop with Docker Compose. The default stack
builds the web portal, API and background worker from this checkout and starts
PostgreSQL for metadata. Uploaded files are stored in a Docker volume. No
separate database, CAD checkout, GPU, or object store is required.

### 1. Get the code

```sh
git clone https://github.com/ocpistol-bec/foundrypdm.git
cd foundrypdm
```

### 2. Start everything

From the repository root:

```sh
docker compose up -d --build
```

The first build can take a while. PostgreSQL creates the `pdm` database on
first start; the PDM server applies its schema migrations automatically. Wait
for the server to start, then check:

```sh
curl http://localhost:8080/healthz
# → ok
```

The portal is at http://localhost:8080/. To use a different host port, start
with `PDM_PORT=8081 docker compose up -d --build` (or put `PDM_PORT=8081`
in a `.env` file in the repository root). PostgreSQL is only accessible on the
internal Docker network.

### 3. Create the first admin

```sh
docker compose exec pdm pdm-cli create-org --name "Acme"
docker compose exec pdm pdm-cli create-user \
  --org Acme --email you@example.com --display-name "You" --role admin \
  --password "change-me"
```

Open http://localhost:8080/ and sign in. This first admin is also the server's host admin: the
one who changes what every organization on the server shares and can make other admins host
admins (see [docs/security.md](docs/security.md#authorization); recovery:
`pdm-cli host-admin grant --email … [--org …]`).

### 4. Manage your installation

```sh
docker compose logs -f pdm       # startup and application logs
docker compose down              # stop; keep the database and uploaded files
docker compose up -d --build     # rebuild and restart after updating the code
```

The `postgres-data` and `blob-data` Docker volumes hold your database and
uploaded files. Back up **both** regularly; `docker compose down -v` deletes
both volumes and all their data. The default database credentials are intended
for this private Docker network; change both values in `compose.yaml` together
before deploying to a shared environment. See [`docs/docker-server.md`](docs/docker-server.md)
for advanced CAD, GPU and S3 configuration.

### 5. Portal settings (after login)

Admin → **Settings** (not env vars):

- **Branding** — display name, logo and theme colors for the sidebar, the sign-in page and
  the browser tab; on a server with several organizations, `PDM_SIGNIN_ORG` picks the one the
  sign-in page shows. `?nobrand` in the address turns the colors off for one visit
- **Mail server** — SMTP so **Invite user** works
- **Onshape** — API keys + default project for Send to manufacturing
- **Preview renderer** — GPU vs CPU for thumbnails
- **Part-number prefixes** — auto part numbers

In **Administration → Users**, admins can disable or enable accounts. Disabled
accounts retain their files and history, but cannot sign in or use existing API
tokens until enabled again. At least one active administrator must remain.

Do not commit SMTP or Onshape secrets.

The standalone image does not bundle the CAD CLI or web editors. CAD CLI-dependent
preview, analysis and conversion jobs need the advanced image described in the
Docker server guide.

---

## What this repo contains

- `apps/pdm-server` — HTTP server
- `crates/pdm-api` — REST + web portal (`static/`)
- `crates/pdm-db` — Postgres
- `crates/pdm-worker` — preview / analyze jobs
- `docker/` — Compose + Dockerfiles

## Development checks

Use Rust 1.90 or newer and Node.js 18 or newer:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node --test crates/pdm-api/tests/web-ui.test.cjs
```

Portal changes (`crates/pdm-api/static/`) follow [`docs/ui-guidelines.md`](docs/ui-guidelines.md);
its checklist says what to check before merging.

The last-administrator concurrency test uses an isolated Postgres database:

```sh
PDM_TEST_DATABASE_URL=postgresql://USER:PASSWORD@localhost/TEST_DATABASE \
  cargo test -p pdm-db account_updates_preserve_an_active_admin -- --ignored
```

See [`e2e.sh`](e2e.sh) for the vault/manufacturing acceptance-test environment.
