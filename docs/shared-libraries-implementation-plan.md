# Shared Libraries — Implementation Plan and AI Task Handoff

Status: proposed implementation plan; no runtime functionality implemented by this document.
Date: 2026-11-18.

## 1. Goal

Allow one Foundry PDM to publish a folder/catalog of parts or items and another PDM
to subscribe to it. Example: a bearing manufacturer publishes a catalog; customers
browse it through their own PDM and insert bearings into SolidCraft assemblies.

An existing assembly must remain reproducible when the supplier is offline,
publishes an update, withdraws a part, or revokes future access.

## 2. Product requirements

The following requirements come from the planning discussion:

- Libraries have a separate top-level area from Projects.
- A supplier can select folders, items, and exact file versions for publication.
- A checkbox beside an item/file/version is a convenient publication control.
- Access is configurable: **Anyone** or **Credential required**.
- Suppliers choose whether to distribute geometry-only models, actual native
  SolidCraft files, or both.
- Supplier components can be read-only references, with an optional policy allowing
  customers to make independent editable local copies.
- Another PDM can subscribe, browse, download, and use these parts in assemblies.
- Publishers can allow customers to download and store a complete published library
  release on their own PDM. Customers can choose this explicitly rather than only
  caching parts when they are used.

Implementation defaults below are recommendations, not additional user decisions:

- Explicit immutable library releases rather than automatically publishing saves.
- Library-wide access grants initially; separate libraries for different audiences.
- Metadata sync and CAD downloads on demand by default, with an explicit
  publisher-controlled full-library storage option (see section 2.2).
- Manual refresh and polling initially; webhooks and public directories later.
- STEP is the initial geometry-only representation where conversion is available.
- Support both public and credential-required access in the first complete release.
- Public discoverability is separate from public access. A public library may be
  unlisted; an unlisted URL is not an access-control mechanism.

### 2.1 Separate format from editing policy

**Published representations:** geometry-only, native SolidCraft, or both.

**Customer usage policy:** reference-only, or allow an editable local copy.

These are independent. Native files can be read-only library references. STEP files
can still be modified after import. Enforce policy inside supported PDM/CAD workflows;
do not claim that downloaded files are universally un-editable. Geometry-only
publication must not expose the source feature history or a hidden native download.

### 2.2 Download and store the library on the customer's PDM

Provide a publisher setting: **Allow full-library download**. This controls bulk
replication of the published catalog through supported APIs/UI. It is separate from
access mode and editing policy; authenticated access is still required for private
libraries. Disabling bulk replication does not prevent retention of individual
components required by saved assemblies and cannot prevent a recipient from
retaining files already delivered.

When allowed, the customer chooses a storage mode:

| Mode | Behavior |
|---|---|
| On demand | Store catalog metadata locally; download CAD files when used. |
| Store full library on this PDM | Download a selected published release, including its catalog, metadata, previews, permitted representations, and required dependencies, into the customer's server-side storage. |

Full-library storage means durable installation, not a temporary browser download
or CAD workstation cache. Every item in the installed release must be browsable and
usable through the customer's PDM without contacting the supplier. Unsupported
representations remain files available for download; do not claim unsupported CAD
formats can be inserted.

Requirements:

- Show the selected release, representation choices, estimated download size,
  download progress, failures, retry controls, and verified offline readiness.
- Permit a fixed installed release or optional automatic download of future
  releases. Automatic catalog updates never update saved assembly components.
- Keep the previous complete installation active until a replacement is verified.
  Interrupted downloads resume; insufficient storage must not corrupt it.
- Full installations are explicitly retained and excluded from automatic cache
  eviction until an administrator changes storage mode or removes the installation.
- Retain supplier provenance, license/attribution, and reference-only/local-copy
  policy. Installing a library does not make its items customer-owned or editable.
- Revocation, supplier outages, withdrawal, or later disabling full downloads stop
  future remote retrieval as applicable; already installed content remains usable.
- Removing a subscription offers **Keep stored library offline** or **Remove stored
  library**. Removal preserves versions required by existing assemblies. An offline
  retained library keeps its identity and can be reconnected without duplicating it.

This is a required product feature, not the deferred portable release-bundle export.

## 3. Repository boundaries and starting points

Inspect current code before implementing; this checkout contains other ongoing work.
Documentation can lag implementation.

| Area | Starting points |
|---|---|
| Domain types and validation | `crates/pdm-core/src/lib.rs` |
| Queries and migrations | `crates/pdm-db/src/lib.rs`, `crates/pdm-db/src/migrate.rs`, `crates/pdm-db/migrations/` |
| HTTP, authentication, authorization | `crates/pdm-api/src/routes.rs`, `auth.rs`, `lib.rs`, `state.rs` |
| Portal | `crates/pdm-api/static/app.js`, `index.html`, `style.css`; inspect `portal.rs` for legacy overlap |
| Storage | `crates/pdm-storage/` |
| Jobs and conversions | `crates/pdm-worker/` |
| API and data documentation | `docs/api-specification.md`, `docs/data-model.md`, `docs/security.md` |
| SolidCraft/plugin handoff | `docs/qwen-pdm-addon-handoff.md`, `docs/qwen-pdm-addon-tasks.md` |

Current findings to verify in Task L00:

- Items currently require `project_id`; simply adding a Libraries navigation tab
  does not establish independent library ownership.
- Part numbers are unique per organization in the initial schema.
- Files/versions, hashes, org-scoped storage, jobs, and audit records are reusable.
- `file_references` and `bom_snapshots` exist, but general assembly-reference handling
  needs investigation rather than an assumption that it already works.
- Token scopes exist as data; enforce the new scopes at every relevant boundary.
- Migrations are explicitly registered in `migrate.rs`. Use the next available
  migration number when implementing; do not reserve a number in this plan.

The standalone plugin checkout is identified by the current handoff as
`/workspace/FoundryPDM-SolidCraft-Plugin`. Read its `AGENTS.md` and canonical handoff
before CAD integration work. Plugin implementation belongs there; minimal CAD host
hooks belong in SolidCraft; server/API changes belong here. If those repositories
are unavailable, complete server tasks and record the CAD work as blocked.

## 4. Architecture and invariants

### 4.1 Organization and identity

- Projects contain working product designs.
- Locally owned libraries can contain their own items and catalog folders.
- A library can also publish references to selected project-owned items without
  duplicating the working files. Organizing a catalog must not move source items.
- Subscribed libraries are remote read-only catalogs with local caches/mappings.
- One source item can appear in multiple published libraries.
- Publisher, library, entry, release, item, version, and representation have explicit
  stable identities. URLs are changeable connection locations, not identities.
- Do not identify parts by names, paths, or manufacturer part numbers.
- Keep internal/customer part numbers separate from manufacturer part numbers.
  Two suppliers may both publish `6205` without collision in the customer's PDM.
- Remote IDs are provenance, not foreign keys to another organization's local rows.

### 4.2 Publication

- A checkbox stages an exact version into a draft publication selection.
- An item-level checkbox resolves to a visible exact version at selection time.
- `Publish release` snapshots selected versions, catalog paths, allowlisted metadata,
  usage policy, representations, and dependency relationships.
- Unchecking stages removal from a future release. Existing releases are unchanged.
- A public projection must exclude credentials, internal comments, costs, owners,
  unselected formats, and private manufacturing data by default.
- Required dependencies must be explicitly included or publication fails with a
  useful report. Never silently broaden the published/private content boundary.
- Missing conversions block the requested representation or produce a clear draft
  error; never substitute a native source for geometry-only publication.
- Freeze the actual representation hashes/bytes in the release. A later conversion
  or thumbnail regeneration must not silently change an existing release manifest.
- Published and locally referenced blobs need retention protection against source
  deletion and garbage collection.

### 4.3 Consumption and retention

- Metadata can synchronize independently from large CAD files.
- Verify download size and SHA-256 before activation.
- Stage synchronization and activate atomically; failed sync retains the previous
  usable release. Retrying must not duplicate imported items or versions.
- Before saving a usable assembly reference, retain its exact component version and
  required dependency closure locally. Offline use cannot depend on supplier URLs.
- Never resolve an existing assembly to `latest` implicitly.
- Revocation stops future remote access; it cannot erase bytes already delivered.
- Withdrawal discourages new use but preserves existing assembly references.
- Unsubscribe/delete-source operations must not cascade-delete referenced content.
- Supplier-managed content is read-only through all supported mutation paths, not
  only through disabled UI controls. Local annotations are stored separately.
- Editable copies receive new local identity and retain source provenance. They do
  not become an editable branch of the supplier item or update it remotely.

### 4.4 Authorization and connection handling

- Anonymous/library credentials can read only entries and representations in
  authorized published releases. A known item ID or hash grants no extra access.
- Public library access must not turn generic item/blob APIs into public APIs.
- Credential-required access must be enforceable on metadata, previews, and bytes.
- Customer-specific credentials support expiration, rotation, revocation, and audit.
- Customer PDM connection secrets stay server-side and out of logs, exports, browser
  payloads, and CAD metadata. Choose at-rest protection using existing conventions.
- Validate subscription endpoints and redirects, timeouts, sizes, and pagination.
  Define administrator-configured handling for private/self-hosted network addresses;
  do not allow arbitrary remote URLs to become an internal-network request proxy.
- Do not forward supplier credentials to a different redirect origin.

## 5. Conceptual data model and protocol

Finalize names and constraints in L01; these are responsibilities, not mandatory DDL.

| Entity | Responsibility |
|---|---|
| Library | Organization-owned catalog identity, defaults, access policy, full-download permission, source selections |
| Library folder/entry | Catalog hierarchy and source item/version selection, per-entry overrides |
| Release | Immutable manifest, release sequence, note, digest, publication timestamp |
| Release entry/representation | Frozen metadata, source version, policies, dependency edges, hashes/sizes |
| Access grant | Customer identity/label, library scope, credential hash, expiry/revocation |
| Subscription/installation | Local org, remote identity and endpoint, protected credential, storage mode, selected/active release, representation selection, update preference, sync/progress and retention state |
| Import mapping | Remote identities to local item/version/artifact, provenance, cached state |
| Retained component reference | Assembly version to exact local component version and provenance |

Reuse existing blobs, jobs, attempts, audit, and file-version infrastructure where
appropriate. Avoid a second storage system or a parallel queue.

Separate publisher management, consumer subscription management, and remote read
APIs. Draft contracts should cover:

- Protocol capabilities/version negotiation and durable publisher identity.
- Library settings, access grants, draft selections, publication, and release status.
- Authorized library details, release lists, bounded paginated manifests, and exact
  representation downloads. Pagination must stay within one immutable release.
- Subscription creation, credential replacement, sync/refresh, offline caching,
  update preview, unsubscribe, and local-copy requests.
- Stable errors for unauthorized, revoked, withdrawn, missing representation,
  incompatible protocol, incomplete dependencies, and corrupted downloads.

Do not invent a login session protocol unnecessarily. Customer-scoped bearer
credentials can implement credential-required subscription initially. Record the
supported flow clearly; interactive supplier-account login can be added separately.

## 6. Execution rules for the local AI

1. Execute one numbered task at a time, starting with L00. Read dependencies first.
2. Inspect `git status` and applicable repository instructions. Preserve unrelated
   work, including uncommitted migrations and changes in shared source files.
3. Update the task ledger below after verified work. Do not mark a task done based
   on placeholders, mocked success paths, or unavailable cross-repository work.
4. Add meaningful tests for authorization, transactions, retention, protocol, and
   assembly behavior. Avoid tests that only mirror trivial UI implementation.
5. Run focused checks while implementing and the repository's required checks at
   milestone completion. Report failures and distinguish existing failures using
   evidence; do not claim unrun checks passed.
6. Update API/data/security documentation with the code that establishes behavior.
7. Do not commit, push, deploy, or restart a shared environment unless requested.
8. Finish each task with: files changed, behavior delivered, commands/results,
   remaining blockers, decisions made, and the next eligible task.

## 7. Task ledger

Use `pending`, `in_progress`, `blocked`, or `completed`; include evidence/links when
updating. All tasks begin pending.

| Task | Status | Dependencies | Main owner |
|---|---|---|---|
| L00 — Current-state inventory | pending | None | PDM + plugin research |
| L01 — Contracts and ownership design | pending | L00 | PDM |
| L02 — Library storage and item ownership | pending | L01 | PDM |
| L03 — Scoped access and connection secrets | pending | L02 | PDM |
| L04 — Draft selection and release publishing | pending | L02, L03 | PDM |
| L05 — Remote catalog and download API | pending | L03, L04 | PDM |
| L06 — Subscriptions and synchronization | pending | L05 | PDM |
| L07 — Local cache, mappings, and read-only enforcement | pending | L06 | PDM |
| L08 — Publisher portal | pending | L04, L05 | PDM portal |
| L09 — Consumer portal | pending | L06, L07 | PDM portal |
| L10 — Version-pinned assembly references | pending | L07 | PDM + plugin contract |
| L11 — SolidCraft library insertion | pending | L09, L10 | Plugin + minimal CAD hooks |
| L12 — Updates, withdrawal, and editable copies | pending | L08, L09, L10, L11 | PDM + plugin |
| L13 — End-to-end acceptance and operator docs | pending | L12 | PDM + plugin |

### L00 — Inventory the actual integration

**Work:** Trace item ownership, folder queries, version/artifact downloads, token
authorization, conversion capabilities, deletion/retention, assembly references,
and the active plugin/host integration. Identify all mutation and download paths
that library restrictions must cover, including public design links. Determine
which geometry-only and native formats can actually be generated and inserted.

**Deliverable:** A current-state findings section appended to this document with
file/symbol references, integration boundaries, gaps, and unresolved decisions.

**Acceptance:** Evidence-backed list; no assumption that schema-only BOM tables are
implemented. Missing plugin checkout recorded as a specific blocker for its work.

### L01 — Specify contracts and resolve ownership

**Work:** Define the library-owned item model and how project items become catalog
entries without moving/duplicating them. Specify folder ownership, namespace rules,
remote identity, manifest schema, dependency graphs, authorization roles, access
credential flow, endpoint contracts, and the read-only/local-copy matrix. Specify
publisher identity continuity through URL changes and restore/clone operations.

Choose an incremental migration compatible with existing project-owned items.
Prefer explicit library ownership over user-visible fake projects. If a generic
container abstraction is justified, bound its scope and document affected queries.

**Deliverable:** Contract/design document linked here, including example manifest
and request/response payloads; settled defaults and explicit unresolved blockers.

**Acceptance:** All later tasks can implement against concrete contracts; version
pinning and cross-org isolation are unambiguous. No silent global part-number
constraint change without an upgrade and API compatibility plan.

### L02 — Implement library persistence and ownership

**Work:** Add migrations, register them, add domain types and org-scoped queries.
Implement library-owned items/folders, catalog references to project items, access
configuration, draft selections, release records, subscriptions, and mapping keys
as needed by the agreed design. Establish explicit deletion/retention constraints.

**Acceptance:** Existing project data upgrades intact. Cross-org item/folder links
are rejected. Library CRUD and independent item creation work without a fake
project. Unique constraints prevent duplicate mappings; manufacturer numbers can
collide safely. Test fresh schema and upgrade from a pre-library database.

### L03 — Implement access grants and protected connections

**Work:** Add owner/admin management permissions and library-scoped read grants.
Support Anyone and Credential required modes. Implement credential generation,
hashed publisher-side storage, expiry, rotation, revocation, and customer-side
secret protection. Implement endpoint/redirect validation for subscription fetches.

**Acceptance:** Anonymous clients cannot read private metadata or bytes. A credential
for library A cannot access library B or generic org APIs. Revoked/expired tokens
fail immediately. Secrets are redacted. Test wrong-org management, redirect-origin
credential leakage, and configured self-hosted address behavior.

### L04 — Implement draft selection and immutable publication

**Work:** Stage exact versions and per-entry format/usage overrides. Resolve selected
folder trees and dependency closure. Freeze allowlisted metadata, representations,
and hashes in a release. Use conversion jobs where supported and expose failures.
Publish transactionally; detect stale draft/concurrent publication conflicts.

**Acceptance:** A source save does not publish itself. Private dependencies or
missing requested formats produce actionable errors. Geometry-only releases expose
no native download. Published manifests survive source edits/moves/deletion under
the retention policy. Concurrent publishing cannot create partial/duplicate releases.

### L05 — Implement remote read protocol

**Work:** Add capabilities, library/release reads, paginated immutable manifests,
and streaming representation/preview downloads. Authorize by published membership,
not hash knowledge. Include size/hash metadata and versioned protocol semantics.
Expose the full-download permission and representation sizes in the protocol, and
enforce it on bulk installation endpoints without blocking individual assembly use.

**Acceptance:** A separate HTTP client can consume public and private catalogs.
Unpublished versions and excluded formats remain inaccessible through these APIs.
Test direct ID/hash requests, revoked credentials, stable pagination, and unsupported
protocol behavior. Existing generic/private endpoints retain their access boundaries.

### L06 — Implement subscriptions and resumable sync

**Work:** Add org-scoped subscription management, capabilities checks, identity
verification, manual refresh/poll jobs, retry state, and metadata staging/activation.
Support credential replacement without losing cached provenance or active release.
Add full-release installation jobs, publisher permission checks, fixed-release or
automatic-download update preferences, and resumable size/progress accounting.

**Acceptance:** Two PDM instances can synchronize a catalog. Repeating or resuming
a job does not duplicate entries. Failure/cancellation retains the previous usable
release. A changed endpoint cannot silently substitute a different publisher.
Test paging failures, invalid manifests, incompatible servers, and interrupted jobs.
Full-library jobs obey publication permissions and cannot activate an incomplete
installation. Test permission changes during download and insufficient storage.

### L07 — Implement local cache and immutable supplier mappings

**Work:** Download on demand with verification, map remote versions into local
version/artifact storage, expose offline availability, and enforce read-only behavior
through check-in, checkout, properties, moves, deletion, conversion, and other
relevant paths found in L00. Store customer annotations separately. Protect retained
blobs during cache cleanup and unsubscribe.

**Acceptance:** Corrupt/truncated bytes never activate. Same hashes cannot bypass
org isolation. Concurrent requests converge on one mapping. Cached versions work
offline. Mutation APIs reject supplier edits independently of UI restrictions.
Referenced versions survive cache cleanup; unreferenced cache is reclaimable.
Explicitly installed full releases are durable and exempt from cache eviction even
when no assembly uses their parts. Verify complete offline browsing/downloads and
dependency availability. Removing the subscription can retain an offline library;
reconnecting it reuses its identity and mappings.

### L08 — Build publisher library UI

**Work:** Add Libraries > Published by us, library/folder/item management, access
settings and grant management, an Allow full-library download setting,
format/editing defaults, per-entry overrides, and
publication checkboxes on appropriate item/version surfaces. Provide draft review,
dependency/conversion errors, release publication, and Published badges.

**Acceptance:** A supplier can publish a selected project version or directly owned
library item through the UI. Exact selected versions and released vs draft state
are visible. Unchecking stages future removal. Existing releases remain unchanged.
Credential values are shown only when appropriate and not returned by list APIs.

### L09 — Build consumer library UI

**Work:** Add Libraries > Subscribed, URL/credential setup, search/browse, hierarchy,
preview, manufacturer identifiers, provenance, format/usage labels, sync errors,
offline download controls, and subscription removal with retention feedback.
Offer On demand and Store full library on this PDM modes, release/representation
selection, size/progress/retry status, verified offline readiness, update preferences,
and Keep stored library offline / Remove stored library choices when unsubscribing.

**Acceptance:** A customer can configure either access mode, browse a library,
download an available representation, and see its exact version/cache status.
Two manufacturers' `6205` entries remain distinguishable. Never display a working
Insert action before the L11 integration is available; capability-gate it.

### L10 — Implement pinned assembly reference contract and retention

**Work:** Implement or extend assembly reference storage/APIs to bind an assembly
version to exact local component versions, representation hashes, occurrences, and
source provenance. Resolve recursive dependencies, reject unsupported/missing
references and cycles appropriately, and retain the required closure. Specify the
plugin's save/open contract and opaque-CAD reference resolution responsibilities.

**Acceptance:** Saving cannot claim offline readiness with missing dependencies.
Reopening a historical assembly resolves the original component bytes. New supplier
versions do not change it. Reference tracking and save are transactionally consistent
or use a documented recoverable protocol; failed saves leave no misleading pin state.

### L11 — Implement SolidCraft browse/insert/open workflow

**Work:** In the canonical plugin repository, add Libraries browsing and insertion
through the customer's PDM connection. Resolve local pinned representations and
embedded document references using the supported CAD interface. Preserve stable
occurrence/subcomponent identity and read-only supplier behavior. Add only necessary
host hooks in SolidCraft; keep PDM networking out of geometry/document core layers.

**Acceptance:** Insert a bearing, save an assembly, restart CAD, disconnect the
supplier, and reopen successfully. Test geometry-only and native cases supported
by capabilities; no fake native conversion or placeholder successful insertion.
Record exact repository revisions and plugin/host test commands in the handoff.

### L12 — Implement updates, withdrawal, and local editable copies

**Work:** Detect new releases and preview version/metadata changes. Add explicit
assembly component update and resulting assembly save. Define tombstones/withdrawal
without rewriting release content. Permit local copies only under the selected
usage policy, with fresh local item/document identity and retained provenance.

**Acceptance:** Updating creates a new assembly version; the old version still
opens with old components. Withdrawal/revocation/unsubscribe preserve retained
assemblies. Local copies are editable, cannot overwrite supplier items, and do not
automatically receive supplier updates. Test reference-only policy rejection.

### L13 — Verify the complete workflow and document operation

**Work:** Provide a reproducible isolated two-PDM fixture and acceptance procedure,
plus automated integration coverage for critical server boundaries. Document
publisher setup, credential rotation, subscriptions, formats, offline behavior,
updates, backup/restore identity, retention, and protocol compatibility. Update
API/data/security documentation and supported-capabilities reporting.

**Acceptance scenarios:**

1. Public bearing library: anonymous browse/download of selected content only.
2. Private library: customer login/credential succeeds; wrong, expired, and revoked
   credentials fail for metadata, previews, and bytes.
3. Geometry-only release: native source/feature history is not obtainable via any
   library credential or publication link produced by the feature.
4. Native release: supported native insertion works with read-only policy.
5. Assembly survives supplier outage, newer release, source deletion, withdrawal,
   and subscription removal once its dependencies are locally retained.
6. Explicit update preserves the old assembly; permitted local copy is independent.
7. Two suppliers with identical part numbers coexist without identity collisions.
8. Interrupted sync, corrupted bytes, and repeated jobs preserve the last complete
   state and do not duplicate catalog items.
9. Fresh-install and existing-data migration paths pass; other orgs remain isolated.
10. Full-library storage: download a complete release, restart the customer PDM,
    disconnect the supplier, and browse/use a previously unused part successfully.
    Installed content survives cache cleanup. Verify denied bulk downloads when
    disabled, resumable failures, fixed vs automatic release download, and both
    unsubscribe choices without losing assembly dependencies.

## 8. Verification commands and completion reporting

Use commands appropriate to the changed crate/task and the repository's current
instructions. The documented PDM completion checks are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node --test crates/pdm-api/tests/web-ui.test.cjs
```

Database integration tests require an isolated test database. Follow current test
conventions; never run destructive fixtures against a working vault. Read plugin
and CAD checkout instructions for their actual checks rather than copying old
monorepo commands. A documentation-only task does not require Rust compilation.

Suggested completion report:

```text
Task: Lxx — title
Status: completed / blocked / in_progress
Behavior delivered:
Files changed:
Verification commands and results:
API/schema/compatibility decisions:
Remaining blockers:
Next eligible task:
```

## 9. Copyable prompt for a fresh local AI session

```text
Implement the next eligible task from
docs/shared-libraries-implementation-plan.md, starting with L00 if none is complete.
Read applicable repository instructions and inspect git status first. Preserve
unrelated work. Follow the task dependencies, architecture invariants, and acceptance
criteria. Execute one task, verify it, update the ledger and relevant documentation,
and report evidence and blockers. Do not commit, push, deploy, or restart shared
services. For CAD/plugin work, first follow docs/qwen-pdm-addon-handoff.md and the
canonical plugin repository instructions. Do not mark unavailable cross-repository
work complete.
```

## 10. Deferred extensions

After the complete bearing workflow is proven: public directory/discovery,
interactive supplier-account federation, multi-folder selection convenience,
webhooks/delta sync, signed/offline release bundles, richer faceted catalog search,
additional CAD connectors, and usage analytics. These do not replace any of the
access modes, publication controls, or native/geometry policies required above.
