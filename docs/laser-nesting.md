# Laser nesting (Sparrow)

The PDM nests sheet-metal flat patterns onto rectangular stock with the
[Sparrow](https://github.com/JeroenGar/sparrow) engine and exports a
correctly-scaled DXF per sheet for import into the laser's CAM software.

## What this is (and is not)

- **Is:** a nesting *layout* — where each part copy goes on each sheet,
  exported as nominal (un-kerfed) DXF geometry the CAM software can import.
- **Is not:** a generated NC program. Kerf compensation, lead-ins, cut
  sequence, power/speed, and machine-specific output are supplied by the
  laser's CAM software. Do not treat a nest DXF as G-code.

The engine is a pinned upstream commit (see `docs/sparrow-compatibility.md`);
the PDM builds its solver JSON, runs the binary as a bounded subprocess inside
the worker, independently validates the layout, and renders the sheet DXF from
the *original nominal curves* (not the solver's collision polygons).

## Workflow

1. **Prepare flat patterns.** Each part needs a flat-pattern DXF: generated
   from a SolidCraft sheet-metal design (`sheet.flat-pattern` job) or uploaded
   directly (`POST /items/{id}/flat-pattern`). DXF items whose head version is
   itself a `.dxf` can nest without an explicit flat pattern.
2. **Create the nest.** In the portal, open **Laser Nests → New nest** (or select
   items under **Items** and choose **Create laser nest**). The create panel has:
   - **Sheet information** — length, width, grain direction (along length /
     along width / any), edge margin, and separate horizontal / vertical part
     spacing.
   - **Parts** — one line per part, chosen from the list of sheet-metal parts.
     Each line has a quantity and per-part options: *mirror original* (nest a
     mirror of the geometry, e.g. cutting the opposite face), *flipping
     allowed* (let the solver place mirrored copies), and *grain* (align this
     part's grain with the sheet's). The same part can be added on multiple
     lines with different options.

   The parts are pinned to exact CAD versions and their flat-pattern blobs at
   queue time (immutability).
3. **Run.** A `laser.nest` background job nests the parts across one or more
   identical sheets, validates each sheet, and stores per-sheet DXF + SVG
   preview + a manifest.
4. **Review & export.** Open **Laser Nests** to see runs, previews, and sheet
   counts. Download a single sheet DXF, a sheet preview, or the whole package
   (ZIP of every sheet DXF + preview + a `MANIFEST.txt`).

Mixed materials or thicknesses within one run are rejected; group them into
separate runs. A source CAD version being superseded flags the nest **stale**
without changing its stored outputs.

## API

| Method & path | Purpose |
| --- | --- |
| `POST /laser-nests/validate` | Preflight: resolve sources, report structured problems (mixed material, missing flat patterns, …) |
| `POST /laser-nests` | Snapshot inputs, create + queue; returns `202` with `nest` + `job` |
| `GET /laser-nests` | List org nests (`?project_id=&limit=`) |
| `GET /laser-nests/{id}` | Nest + parts + sheets + placements (detail) |
| `POST /laser-nests/{id}/cancel` | Cancel a queued/running nest |
| `POST /laser-nests/{id}/rerun` | New linked run from the same source parts |
| `GET /laser-nests/{id}/sheets/{i}/dxf` | Download one sheet's DXF |
| `GET /laser-nests/{id}/sheets/{i}/preview` | One sheet's SVG preview |
| `GET /laser-nests/{id}/package` | ZIP of all sheets (DXF + preview) + `MANIFEST.txt` |

Creation body:

```json
{
  "name": "Bracket batch",
  "project_id": "…", "machine_id": "…",
  "material": "Aluminum 5052-H32", "thickness_mm": 1.5,
  "units": "in", "default_part_unit": "in",
  "stock_width_mm": 48, "stock_height_mm": 96,
  "sheets": [
    { "width_mm": 48, "length_mm": 96, "quantity": 2 },
    { "width_mm": 24, "length_mm": 48, "quantity": null }
  ],
  "edge_margin_mm": 0.4,
  "spacing_x_mm": 0.2, "spacing_y_mm": 0.2,
  "grain_direction": "along_length",
  "seed": 42,
  "parts": [
    { "item_id": "…", "quantity": 8, "grain": "along_length" },
    { "item_id": "…", "quantity": 4, "mirror": true, "allow_mirror": false }
  ]
}
```

(The field names keep the historical `_mm` suffix; with `"units": "in"` the
values are inches and are converted. For a metric shop use `"units": "mm"` and
millimetre values.)

`stock_width_mm`, `stock_height_mm`, and `parts[].quantity` are required; the
rest default sensibly. Per-part overrides: `file_version_id`, `layer_cut`,
`layers_mark`, `unit`, `mirror`, `allow_mirror`, `grain`, `step_angle`,
`priority`, `label`, `use_bounding_box`, `common_edge`, `min_qty`,
`filler_qty`. The same `item_id` may appear on multiple part lines with
different options.

**Multiple sheet sizes.** `sheets` (all dimensions in `units`) lists
sheet-size definitions the worker works through *in order*: it fills each size
up to its `quantity` (`null` = as many as needed) and moves on to the next
until every part line is placed. A part that does not fit one size is carried
to the later, larger sizes. `stock_width_mm`/`stock_height_mm` record the
primary (first) sheet and are still required; omitting `sheets` keeps the
legacy single-size run.

**Defaults.** `GET /settings/nest-defaults` returns the org's saved
create-panel defaults (sheet size, units, margins, spacing, grain, corner,
guillotine, labels, part source unit) and `PUT` (admin) saves them; a blank
field keeps the panel's built-in default. `GET
/items/{id}/flat-pattern/preview` returns an SVG of a part's flat pattern for
the picker thumbnails.

### Spacing, grain, and mirrors

- **Spacing** is directional: `spacing_x_mm` (horizontal) and `spacing_y_mm`
  (vertical). The solver enforces them exactly by dilating each part's outline
  by the spacing box before packing, so parts sit at least `spacing_x` apart in
  x when their y-ranges overlap, and at least `spacing_y` apart in y when their
  x-ranges overlap. Omitting both falls back to `part_gap_mm` (legacy uniform),
  then to a 5 mm default.
- **Grain.** `grain_direction` sets the sheet's grain (along the stock *length*
  — the vertical axis — or the *width*). Each part's `grain` says whether that
  part's grain should run along or across its own long axis. When both are set,
  the allowed rotations are restricted so the part's long axis stays parallel to
  the sheet grain (e.g. sheet grain along length + part grain along length ⇒ the
  part may only rotate 90°/270° so its long axis runs down the sheet). `"any"`
  on either side lifts the restriction. An explicit `allowed_rotations` array
  always wins.
- **Mirrors.** The portal presents a three-way **Mirror** control per part:
  *Original Only* (the default), *Both*, *Mirror Only*. Under the hood *Mirror
  Only* sets `mirror: true` (nest a mirror image of the original geometry —
  useful when cutting the opposite face of a part) and *Both* sets
  `allow_mirror: true` (let the solver place flipped copies). `allow_mirror`
  defaults to false in the API too: a flipped copy is cut with the sheet's
  bottom face up, which is wrong for sheet with one good face (brushed,
  coated, film-protected). The same part can be cut in its
  original and mirrored orientations on the same sheet.

### Sheet & part options

The create panel mirrors a professional nesting tool (Sheets | Parts tabs).
Beyond stock, spacing, grain, and mirrors:

- **Sheet**: *nesting corner* (which corner the fill starts from — the layout is
  moved to that corner without mirroring any part: see below), *origin X/Y*
  (offset of the nesting origin),
  *sheet-sheet distance* (reference gap between sheets), *guillotine cut*
  (request a guillotine-cuttable layout), *automatic sheet origins*, *fill
  direction*, and *create as many sheets as needed*.
- **Per part**: *step angle* (rotation increment: 90°/45°/30°/180° — combined
  with grain), *priority* (nesting order, lower first), *label* (drawn when
  *add part labels* is on), *use bounding box* (nest by the part's bounding box
  instead of its outline), *common edge*, *minimum qty*, and *filler qty*.
- **All parts**: *add part labels* (draw P# / the part label on the sheet DXF +
  preview), *optimized pairs*, and *fill all sheets*.

Grain, spacing, mirrors, step angle, priority, bounding-box, origin, and
nesting corner are enforced by the engine. Guillotine, common edge, fill
direction, fill-all-sheets, optimized pairs, minimum/filler qty, and
sheet-sheet distance are stored on the run and surfaced in the manifest and
preview (the layout preferences are recorded even where the engine treats them
as advisory).

**Nesting corner and mirroring.** The solver always fills from the lower-left.
Upper-right is reached by turning the whole layout 180°. Lower-right and
upper-left need a reflection of the layout, and a reflection on its own would
mirror every part, whatever its line's mirror setting. For those two corners
the solver is given each part's mirror image instead, so the layout reflection
turns every copy back to how it was drawn: a line set to *Original Only* stays
unmirrored at every corner. A copy the solver flipped (a line set to *Both*) is
still flipped. The solver is also given the rotations that the corner turns
back into the line's allowed ones, so a corner never turns a part to an angle
its line (or `allowed_rotations`) doesn't allow: an upper-right fill with
`allowed_rotations: [0]` still cuts every part at 0°, not 180°.

### Units (inches → millimetres)

Everything is stored and nested internally in millimetres; the exported sheet
DXF is always millimetres (`$INSUNITS = 4`), which is what the laser expects.
Two independent unit concerns are handled:

- **Stock / margin / gap input unit.** Set `"units": "in"` (or `"mm"`, the
  default) and the `stock_width_mm`, `stock_height_mm`, `edge_margin_mm`, and
  `part_gap_mm` values are interpreted in that unit and converted to mm.
  Example: `"units": "in", "stock_width_mm": 48, "stock_height_mm": 96` nests
  on a 48×96 in (1219×2438 mm) sheet. Omitted margin/gap fall back to physical
  10 mm / 5 mm defaults.
- **Part source unit.** Each part's DXF is converted to mm using this
  precedence: a per-part `unit` override → the file's `$INSUNITS` → the nest's
  `default_part_unit` → millimetres. So an inch drawing that declares
  `$INSUNITS = 10` is converted automatically with no configuration; an
  inch drawing that declares no units should set `"default_part_unit": "in"`
  (or a per-part `"unit": "in"`). Generated flat patterns are recorded as mm.

The portal's **New nest** panel exposes the stock unit, part source unit,
grain directions, and directional spacing directly.

## Stock mapping

Given stock `W × H`, edge margin `M`, and directional part spacing
`gx` (x) / `gy` (y) (all nominal mm):

```
S        = 2·chord_tol + ε     (isotropic epsilon covering arc chord error)
outline  = part tessellation ⊕ box(gx, gy)   (each part's collision outline)
offset   = (M - S - gx/2, M - S - gy/2)
strip_height = H - 2·offset.y
```

Each part's collision outline is its nominal tessellation dilated by the
spacing box, so the solver enforces `gx` of horizontal clearance and `gy` of
vertical clearance directly (a part may sit diagonally closer, which is normal
for x/y spacing). The solver (a strip-packing engine) is constrained to a
fixed-height strip and minimizes its width; the PDM independently validates that
the solved width and every nominal part lie within `[M, W-M] × [M, H-M]` and
that directional spacing is respected (with a hard ring-distance check for parts
nested inside each other's concavities). Multiple identical sheets are produced
by splitting demand into per-sheet batches under one wall-clock budget; a run
that cannot place all demand reports the unplaced copies rather than silently
dropping them.

## Configuration

| Env var | Default | Meaning |
| --- | --- | --- |
| `PDM_SPARROW_CLI` | `/usr/local/bin/sparrow` | Path to the pinned `sparrow` binary. If absent, `laser.nest` jobs fail explicitly (no fake layouts). |

Per-run limits come from the nest record: `time_budget_sec` (whole batch),
`attempt_limit_sec` (one solver attempt), `max_sheets`, `chord_tol_mm`,
`join_tol_mm`, and `allowed_rotations`.

## Pinned engine

Commit `b8ef03312b7a195895f6c5332c6674dd78420ede` (sparrow 0.3.0, jagua-rs
2.0.0). Built with:

```
cargo build --release -p sparrow-cli --bin sparrow --features only_final_svg
```

The Docker images build and bundle it in the `builder` stage; the runtime
invokes it offline. See `docs/sparrow-compatibility.md` for the verified I/O
contract and `docs/sparrow-laser-implementation-plan.md` for the full design.

## Tests

- `crates/pdm-nesting` — unit tests for DXF parse/write, contour assembly,
  transforms, solver I/O, sheet validation, and batching.
- `crates/pdm-nesting/tests/e2e_sparrow.rs` — runs the *real* pinned binary
  (set `SPARROW_BIN`) and asserts feasibility, quantity, and nominal dimension
  preservation.
- The live PDM API + worker exercise the full create → nest → export path.
