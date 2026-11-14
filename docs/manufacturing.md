# Manufacturing & CAM Management

Target subsystem (from `ToDoMFGCAM.txt`). **Status: core implemented, deployed and e2e-tested
(V.01-07 → V.01-11; migrations 0005–0007).** Done: machine registry (CNC/laser/plasma/waterjet/printer/
manual), tool library (9 kinds), post-processors, the setup→CAM→NC→job→inspection chain (NC programs pin
the source revision + machine + post-processor version), per-item status (`GET /items/{id}/manufacturing`),
the org-wide robotics dashboard (`GET /manufacturing/dashboard`), and the data-integrity invariant that a
new design revision flags dependent NC programs as `outdated` (verified in `e2e_mfg.rs`), plus 3D-printing
jobs and sheet-metal specs (migration 0008, V.01-12), and the read-only G-code parser
(`POST /gcode/parse` → toolpath + bbox + tool/spindle/rapid-feed-arc counts, V.01-13).
**Remaining:** G-code read-only visualization (web-side, consumes the parser output),
released-package immutability hardening.

## The data chain
```
Part → Revision → CAD Model → Manufacturing Setup → CAM Project → NC Program
      → Machine → Production Job → Inspection
```
Every manufactured part supports the relationships above (a part links to revisions, which link to the
CAD model, which links to one or more mfg setups, CAM projects, NC programs, target machines, production
jobs, and inspection plans/results).

## Machine Registry
Records for: CNC mills, lathes, routers, laser cutters, plasma cutters, waterjets, 3D printers, manual
machines. Track: identity, manufacturer, model, controller, capabilities, work envelope, axis count,
supported materials, supported post-processors, maintenance status/calendar.

## Tool Library
Reusable tooling: end mills, drills, taps, reamers, face mills, chamfer tools, turning tools, tool
holders, inserts. Track: dimensions, cutting parameters (SFM / feeds), coatings, stock/lead time, and
linkage to machines (which machines hold which tools).

## CAM / NC
- **CAM project** → toolpaths; **NC program** (G-code) per machine/post-processor, versioned like other
  PDM files (immutable blobs + revisions).
- **Post-processors:** machine- or controller-specific; a setup declares which post-processors apply.
- **Manufacturing setup:** the bridge between a CAD model and CAM (stock, work coordinate, fixture).

## Production & Inspection
- **Production job:** a run of a part revision on a machine with a tool list + program + operator.
- **Inspection:** plan + results (against the part's `properties`/measurements), traceable to a job.

## Proposed schema (new tables, additive migrations)
`machines`, `machine_capabilities`, `tools`, `tool_uses` (machine↔tool), `post_processors`,
`mfg_setups`, `cam_projects`, `nc_programs` (as PDM items/files), `production_jobs`, `inspection_plans`,
`inspection_results`. Reuse the existing item/version/blob model for CAD models, CAM/NC files, and
inspection artifacts (they are all versioned documents); the new tables add the relational domain.

## Phasing
1. Machines + Tool Library (registry data, CRUD + web UI).
2. Mfg setup + CAM project + NC program (versioned files).
3. Production jobs + inspection (link part → job → results).
See `docs/development-roadmap.md` for the ordered backlog.
