# Sparrow compatibility notes (pinned upstream)

This file is the result of the Task 0 compatibility spike. The `pdm-nesting`
solver adapter targets exactly this revision and these observations. Re-verify
before bumping the pinned commit.

## Pinned revision

- Repository: `https://github.com/JeroenGar/sparrow`
- Pinned commit: `b8ef03312b7a195895f6c5332c6674dd78420ede`
  (`v0.3.0-10-gb8ef033`).
- `sparrow` crate version: `0.3.0`, edition 2024, `rust-version = 1.90`.
- Collision engine: `jagua-rs = 2.0.0` (from crates.io, features `spp`).
- The CLI is a separate workspace crate `sparrow-cli`; the binary is `sparrow`.
  `jagua-rs` is MPL-2.0, `sparrow` is MIT. Both notices ship with the bundled
  binary.

## Build recipe (portable, stable, no SIMD/nightly, no host CPU flag)

```sh
git clone https://github.com/JeroenGar/sparrow
cd sparrow && git checkout b8ef03312b7a195895f6c5332c6674dd78420ede
cargo build --release -p sparrow-cli --bin sparrow --features only_final_svg
```

`only_final_svg` emits a single final SVG (no intermediate solution dump). The
JSON final result is always written regardless.

## CLI flags (verified with `--help` on the pinned build)

| Flag | Meaning |
| --- | --- |
| `-i, --input <PATH>` | instance JSON (or a solution JSON for warm start) |
| `-t, --global-time <SEC>` | total wall-clock budget (split 80/20 explore/compress) |
| `-e, --exploration <SEC>` | exploration budget (use with `-c`) |
| `-c, --compression <SEC>` | compression budget (use with `-e`) |
| `-x, --early-termination` | stop when no improvement is found |
| `-s, --rng-seed <N>` | fixed RNG seed (deterministic for a given seed) |
| `--workers <N>` | separator worker threads |

Omitting `-t` defaults to a 600 s global budget.

## Input schema (jagua-rs 2.0.0 SPP external repr)

Top level is an `ExtSPInstance`:

```json
{
  "name": "my-instance",
  "min_item_separation": 0,
  "strip_height": 1000.0,
  "items": [
    {
      "id": 10,
      "demand": 2,
      "orientation": { "rotation": { "mode": "discrete", "angles": [0, 90] } },
      "shape": { "type": "simple_polygon", "data": [[0,0],[100,0],[100,100],[0,100]] }
    }
  ]
}
```

Verified rules:

- `name` is a string; it names the output files (`output/final_{name}.json`).
- `min_item_separation` is a finite `>= 0` number in input units; omitted
  means zero. It is the *nominal* minimum distance between part boundaries and
  between a part and the strip's left/bottom/right edges.
- `strip_height` is the fixed strip height; the strip is `[0, width] x
  [0, strip_height]` and the solver minimizes `width`.
- `items[].id` is a `u64`, unique across items.
- `items[].demand` is a `u64 > 0`. All demand copies are placed in the single
  strip (SPP never splits into bins).
- `items[].orientation.rotation` is required:
  - `{"mode":"discrete","angles":[...]}` — non-empty list of finite degrees;
  - `{"mode":"stepped","step":...}` — step in `(0,360]` dividing 360;
  - `{"mode":"continuous"}` — any angle.
  - `angles == [0]` is treated as "no rotation".
- `items[].orientation.reflection_axes` is optional (`[]` disables mirroring).
- `items[].shape` for a **strip item** supports only `simple_polygon` and
  `rectangle`. Polygons with holes and multipolygons are rejected. Holes must
  therefore be kept in the PDM's nominal geometry and *not* sent to Sparrow;
  the solver sees the outer boundary only.
- A shape whose exact height equals the usable strip height can fail with
  "could not construct an initial placement" (boundary equality is not
  accepted). Leave a small headroom or the solver's separation.

## Output

The CLI writes, relative to its **working directory**:

- `output/final_{name}.json`
- `output/final_{name}.svg` (visualization only — do not parse geometry from it)
- `output/log.txt`

The JSON is `{ instance fields..., "solution": ExtSPSolution }`:

```json
{
  "strip_width": 200.048,
  "layout": {
    "container_id": 0,
    "placed_items": [
      { "item_id": 10,
        "transformation": { "reflected": false, "rotation": 90.0,
                             "translation": [40.0, 0.03] } }
    ],
    "density": 0.16
  },
  "density": 0.16,
  "run_time_sec": 0
}
```

Verified rules:

- `solution.strip_width` is the achieved strip width.
- `layout.placed_items` has exactly `sum(items[].demand)` entries.
- Each `transformation` is `{ reflected? (default false), rotation (deg, CCW,
  y-up), translation: [x, y] }`.
- **The transformation applies to the original input shape coordinates.** The
  importer centers each item's shape to its centroid internally, but the
  exported transform is re-composed so that applying `translate(rotation
  applied to (reflected original point))` to the original polygon yields the
  placed polygon. Point pipeline, innermost to outermost:
  1. if `reflected`, map `(x, y) -> (x, -y)`;
  2. rotate CCW by `rotation` degrees;
  3. add `translation`.
- `placed_items` may reference an `item_id` more than `demand` times only if
  the input demand allows it; the solver enforces exactly demand copies.

## Mapping a fixed stock sheet onto the strip

To nest on a stock rectangle `W x H` with edge margin `M` and inter-part gap
`G` (all nominal, in mm):

```
min_item_separation = G
strip_height        = H - 2*M + 2*G
```

After the solve, add `(M - G, M - G)` to every `translation` to move the strip
into stock coordinates `[0, W] x [0, H]`. This makes the guaranteed distance
from any part to a stock edge exactly `M` and the guaranteed distance between
any two parts exactly `G` (both measured on nominal outer boundaries).

A single sheet is feasible iff, after the offset, every placed part's nominal
outer boundary lies within `[M, W - M] x [M, H - M]`. This must be validated
independently from the solver output (the strip solver only guarantees the
width/height budget and pairwise separation in strip space, not the stock
right/top margins, since it minimizes width and leaves the top open).

Because SPP places all demand in one strip and minimizes width, a run either
fits one stock sheet or it does not. Multiple identical sheets are produced by
splitting demand into per-sheet batches and solving each batch separately (see
`pdm-nesting` `batch.rs`), keeping a global wall-clock budget.

## Exit codes and failure surface

- `0` on success (final JSON + SVG written).
- `1` on failure; the first line of stderr is `Error: <reason>` (e.g.
  "could not construct an initial placement for item N", "No such file or
  directory"). No partial `final_{name}.json` is published on failure.
- The process respects the `-t` budget; a time-limited run still exports its
  best feasible layout (it does not fail merely because the budget ran out).

## Determinism

For a fixed `-s` seed, the same input and the same binary build produce the
same layout. Parallelism (`--workers`) can change the numeric path, so the seed
and worker count are recorded in the nest manifest for provenance. Bitwise
repeatability across different builds is not promised.
