// Laser nest panel: picked parts only, units that convert, blank fields that
// say their default, re-run of failed runs, and lengths shown in full.
// Run with: node --test crates/pdm-api/tests/web-ui-nest.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8");

function loadFunctions(names, context){
  for (const name of names){
    const start = app.indexOf("function " + name + "(");
    assert(start >= 0, "missing function " + name);
    const asyncStart = app.slice(start - 6, start) === "async " ? start - 6 : start;
    const rest = app.slice(start);
    const end = rest.search(/\n(?:async )?function /);
    vm.runInContext(app.slice(asyncStart, end < 0 ? app.length : start + end), context);
  }
  return context;
}

// A stand-in for the few DOM calls the panel makes: every id resolves to a
// plain object that records what the code sets on it.
function fakeDom(){
  const els = new Map();
  const mk = id => ({ id, value: "", checked: false, disabled: false, textContent: "", innerHTML: "",
    style: {}, dataset: {}, attrs: {}, _q: {}, classList: { contains: () => false },
    setAttribute(k, v){ this.attrs[k] = v; }, focus(){},
    querySelector(){ return null; }, querySelectorAll(sel){ return this._q[sel] || []; } });
  const $ = id => { if (!els.has(id)) els.set(id, mk(id)); return els.get(id); };
  return { $, els, document: { getElementById: $, querySelector: () => null, querySelectorAll: () => [] } };
}

// The nest block of app.js (create panel, list, detail, downloads) in a
// sandbox. `NC` is a top-level `let`, so it is read through the context.
function loadNest(globals = {}){
  const dom = fakeDom();
  const ctx = vm.createContext({
    $: dom.$, document: dom.document, ROUTE: {}, TOKEN: "t",
    toast(){}, go(){}, render(){}, attachThumbs: () => Promise.resolve(), thumbUrl: () => Promise.resolve(""),
    fmtTime: () => "today", emptyState: (ic, t, x, cta) => t + " " + x + (cta || ""),
    smDisp: () => "mm", setTimeout: fn => fn(), localStorage: { setItem(){}, getItem(){ return null; } },
    api: async () => ({}), fetch: () => Promise.reject(new Error("no previews")),
    ...globals,
  });
  loadFunctions(["esc"], ctx);
  const start = app.indexOf("function ncDefaultState(){");
  const end = app.indexOf("async function downloadLatest(");
  assert(start >= 0 && end > start, "nest block found");
  vm.runInContext(app.slice(start, end), ctx);
  ctx.dom = dom;
  ctx.nc = () => vm.runInContext("NC", ctx);
  ctx.setNC = obj => { ctx.__nc = obj; vm.runInContext("NC = __nc", ctx); };
  return ctx;
}
const plain = v => JSON.parse(JSON.stringify(v));
const cands = [{ id: "a", name: "BR-100", hasFlat: true }, { id: "b", name: "BR-200", hasFlat: true }];
const panelApi = (defaults = {}) => async (method, url) => {
  if (url === "/sheet-metal") return { sheet_metal: [{ item_id: "a", flat_pattern_sha: "x" }, { item_id: "b", flat_pattern_sha: "y" }] };
  if (url.startsWith("/items?")) return { items: [{ id: "a", part_number: "BR-100" }, { id: "b", part_number: "BR-200" }], has_more: false };
  if (url === "/settings/nest-defaults") return { defaults };
  throw new Error("unexpected " + method + " " + url);
};
const divBalance = html => (html.match(/<div\b/g) || []).length - (html.match(/<\/div>/g) || []).length;

test("a new nest starts with no parts, and Queue nest says why it is disabled", async () => {
  const ctx = loadNest({ api: panelApi() });
  ctx.nc().active = true;
  const v = ctx.dom.$("view");
  await ctx.renderNestCreate(v);
  assert.equal(ctx.nc().lines.length, 0, "no part is added that nobody picked");
  // The Parts tab closes before the footer opens, so re-rendering the tab
  // cannot remove Queue nest.
  const html = v.innerHTML;
  const between = html.slice(html.indexOf("<div id='ncPartsTab'"), html.indexOf("id='ncQueue'"));
  assert.equal(divBalance(between), 1, "only the footer is open where Queue nest sits");
  assert.equal(ctx.dom.$("ncQueue").disabled, true, "Queue nest is disabled with no parts");
  assert.match(ctx.dom.$("ncQueueWhy").textContent, /Add at least one part/);
  assert.match(ctx.ncPartsHtml(), /No parts yet\. Tick parts above and choose Add selected\./);
});

test("removing the last part leaves the list empty instead of adding one back", () => {
  const ctx = loadNest();
  const NC = ctx.nc(); NC.cands = cands; NC.lines = [ctx.ncDefaultLine("b")];
  const btn = { dataset: { i: "0" } };
  ctx.dom.$("ncPartList")._q[".nc-rmline"] = [btn];
  ctx.ncRebindParts();
  ctx.dom.$("ncPartList")._q[".nc-rmline"] = [];
  btn.onclick({ stopPropagation(){} });
  assert.equal(ctx.nc().lines.length, 0);
  assert.equal(ctx.dom.$("ncQueue").disabled, true);
});

test("the parts tab markup is balanced, so Queue nest stays outside the tabs", () => {
  const ctx = loadNest();
  const NC = ctx.nc(); NC.cands = cands;
  assert.equal(divBalance(ctx.ncPartsHtml()), 0, "no lines");
  NC.lines = [ctx.ncDefaultLine("a")];
  assert.equal(divBalance(ctx.ncPartParamsHtml()), 0, "one line: parameters column");
  assert.equal(divBalance(ctx.ncPartsHtml()), 0, "one line: whole tab");
  assert.equal(divBalance(ctx.ncSheetsHtml()), 0, "sheets tab");
  NC.lines = [];
  assert.match(ctx.ncPartParamsHtml(), /id='nc_spx'/, "spacing is shown with no part selected");
});

test("switching units converts every length and switching back restores it", () => {
  const ctx = loadNest();
  const NC = ctx.nc();
  NC.sheets.push({ width: 24, length: 48, asNeeded: false, quantity: 2 });
  NC.originX = ""; NC.ssd = "abc";
  const el = ctx.dom.$("nc_units");
  ctx.ncBindSheets();
  el.value = "mm";
  el.oninput({ target: el }); el.onchange({ target: el });   // a select fires both
  const mm = ctx.nc();
  assert.equal(mm.units, "mm");
  assert.deepEqual(plain(mm.sheets.map(s => [s.width, s.length])), [[1219.2, 2438.4], [609.6, 1219.2]]);
  assert.equal(mm.margin, 9.906);
  assert.equal(mm.spacingX, 5.08);
  assert.equal(mm.originX, "", "blank stays blank");
  assert.equal(mm.ssd, "abc", "text that is not a number is left for the field check");
  assert.equal(ctx.ncSheetRow(mm.sheets[0], 0).includes("1219.2 × 2438.4 mm"), true);
  assert.match(ctx.ncSheetsHtml(), /Width <span class='dim'>\(mm\)/, "labels follow the unit");
  ctx.ncConvertUnits("in");
  const back = ctx.nc();
  assert.deepEqual(plain(back.sheets.map(s => [s.width, s.length])), [[48, 96], [24, 48]]);
  assert.equal(back.margin, 0.39);
  assert.equal(back.spacingX, 0.2);
});

test("a blank length shows the default the server will use, in the field's unit", () => {
  const ctx = loadNest();
  assert.match(ctx.ncLenInput("nc_margin", "Sheet margin", "", "margin"), /placeholder='0\.394 \(default\)'/);
  assert.match(ctx.ncLenInput("nc_spx", "Part–part", "", "spacingX"), /placeholder='0\.197 \(default\)'/);
  ctx.ncConvertUnits("mm");
  assert.match(ctx.ncLenInput("nc_margin", "Sheet margin", "", "margin"), /placeholder='10 \(default\)'/);
  assert.match(ctx.ncLenInput("nc_spx", "Part–part", "", "spacingX"), /placeholder='5 \(default\)'/);
});

test("lengths are checked strictly and block Queue nest with a message that names the unit", () => {
  const ctx = loadNest();
  const NC = ctx.nc(); NC.lines = [ctx.ncDefaultLine("a")];
  assert.deepEqual(plain(ctx.ncProblems()), []);
  NC.margin = "";  // blank = default
  assert.deepEqual(plain(ctx.ncProblems()), []);
  NC.margin = "1,5";
  NC.spacingY = "-1";
  NC.sheets[0].width = "";
  const msgs = plain(ctx.ncProblems().map(p => p.msg));
  assert.deepEqual(msgs, [
    "Sheet #1 width must be a number greater than 0 (in)",
    "Sheet margin must be a number of 0 or more (in)",
    "Vertical part spacing must be a number of 0 or more (in)",
  ]);
  ctx.nc().tab = "sheets";
  ctx.ncUpdateQueue();
  assert.equal(ctx.dom.$("ncQueue").disabled, true);
  assert.equal(ctx.dom.$("ncQueueWhy").textContent, "Sheet #1 width must be a number greater than 0 (in) (and 2 more)");
});

test("a field's message shows under it once the field has been left", () => {
  const ctx = loadNest();
  const NC = ctx.nc(); NC.lines = [ctx.ncDefaultLine("a")];
  const margin = ctx.dom.$("nc_margin");
  ctx.dom.document.querySelectorAll = sel => sel === ".nc input[data-len]" ? [margin] : [];
  ctx.ncBindSheets();
  margin.value = "1e999";
  margin.oninput();
  assert.equal(margin.attrs["aria-invalid"], "false", "not flagged while first typing");
  assert.equal(ctx.dom.$("nc_margin_err").textContent, "");
  assert.equal(ctx.dom.$("ncQueue").disabled, true, "but Queue nest is already held back");
  margin.onchange();
  assert.equal(margin.attrs["aria-invalid"], "true");
  assert.equal(ctx.dom.$("nc_margin_err").textContent, "Sheet margin must be a number of 0 or more (in)");
  margin.value = "0.5";
  margin.oninput();
  assert.equal(margin.attrs["aria-invalid"], "false", "cleared as soon as it is fixed");
  assert.equal(ctx.dom.$("ncQueue").disabled, false);
  assert.equal(ctx.nc().margin, "0.5");
});

test("a 0 × 0 part spacing is sent so the server does not fall back to 5 mm", async () => {
  const bodies = [];
  const ctx = loadNest({ api: async (m, url, body) => { bodies.push(body);
    return url === "/laser-nests/validate" ? { ok: true } : { nest: { status: "queued" } }; } });
  const NC = ctx.nc(); NC.lines = [ctx.ncDefaultLine("a")]; NC.spacingX = "0"; NC.spacingY = 0;
  ctx.dom.$("nc_name").value = "N";
  await ctx.queueLaserNest();
  assert.equal(bodies[1].part_gap_mm, 0);
  const ctx2 = loadNest({ api: async (m, url, body) => { bodies.push(body);
    return url === "/laser-nests/validate" ? { ok: true } : { nest: { status: "queued" } }; } });
  ctx2.nc().lines = [ctx2.ncDefaultLine("a")];
  ctx2.dom.$("nc_name").value = "N";
  await ctx2.queueLaserNest();
  assert.equal(bodies[3].part_gap_mm, undefined, "only when both spacings are 0");
});

test("after a failed create, Queue nest is re-checked instead of simply re-enabled", async () => {
  const ctx = loadNest({ api: async (m, url) => {
    if (url === "/laser-nests/validate"){ ctx.nc().margin = "3/8"; return { ok: true }; }   // typed meanwhile
    throw new Error("server down");
  } });
  ctx.nc().lines = [ctx.ncDefaultLine("a")];
  ctx.dom.$("nc_name").value = "N";
  await ctx.queueLaserNest();
  assert.equal(ctx.dom.$("ncQueue").disabled, true);
  assert.equal(ctx.dom.$("ncQueue").textContent, "Queue nest");
  assert.equal(ctx.nc().busy, false);
});

test("org defaults are applied once per new panel, in their own unit", async () => {
  const ctx = loadNest({ api: panelApi({ units: "mm", margin: 3 }) });
  ctx.nc().active = true;
  await ctx.renderNestCreate(ctx.dom.$("view"));
  let NC = ctx.nc();
  assert.equal(NC.units, "mm");
  assert.equal(NC.sheets[0].width, 1219.2, "a size the defaults leave out keeps its physical length");
  assert.equal(NC.margin, 3);
  NC.margin = "7";
  await ctx.renderNestCreate(ctx.dom.$("view"));   // back to an open panel
  assert.equal(ctx.nc().margin, "7", "coming back keeps the user's edit");
});

test("the part picker pages through every sheet-metal part and reports a failed load", async () => {
  const calls = [];
  const ctx = loadNest({ api: async (m, url) => {
    calls.push(url);
    if (url === "/sheet-metal") return { sheet_metal: [{ item_id: "p1" }, { item_id: "p250" }] };
    const page = +url.match(/page=(\d+)/)[1];
    return page === 1 ? { items: [{ id: "p1", part_number: "A-1" }], has_more: true }
      : { items: [{ id: "p250", part_number: "A-250" }], has_more: false };
  } });
  const got = await ctx.loadNestableParts();
  assert.deepEqual(plain(got.map(c => c.name)), ["A-1", "A-250"]);
  assert.deepEqual(calls.slice(1), ["/items?sheet_metal=true&limit=200&page=1", "/items?sheet_metal=true&limit=200&page=2"]);
  const failing = loadNest({ api: async () => { throw new Error("boom"); } });
  await assert.rejects(failing.loadNestableParts(), /boom/);
  // The panel says so, with Retry, instead of "no sheet-metal parts".
  const v = failing.dom.$("view");
  v.querySelector = () => null;
  failing.nc().active = true;
  await failing.viewLaserNests(v);
  assert.match(v.innerHTML, /The sheet-metal parts did not load boom/);
  assert.match(v.innerHTML, /id='ncRetry'>Retry</);
  assert.equal(typeof failing.dom.$("ncRetry").onclick, "function");
});

test("nest sizes and spacing keep their decimals and follow the display unit", () => {
  const ctx = loadNest();
  assert.equal(ctx.fmtSize(48 * 25.4, 96 * 25.4, "mm"), "1219.2 × 2438.4 mm");
  assert.equal(ctx.fmtSize(48 * 25.4, 96 * 25.4, "in"), "48 × 96 in");
  assert.equal(ctx.fmtSize(0.4, 0.4, "mm"), "0.4 × 0.4 mm");
  assert.equal(ctx.fmtLen(10, "in"), "0.394 in");
  const row = ctx.nestRow({ id: "n", name: "N", status: "completed", stock_width_mm: 1219.2, stock_height_mm: 2438.4,
    grain_direction: "any", created_at: "", stale: true, stale_reason: "BR-100 has a newer version" });
  assert.match(row, /1219\.2 × 2438\.4 mm/);
  assert.match(row, /Stale: BR-100 has a newer version/, "the stale reason is visible text");
});

for (const status of ["failed", "cancelled"]) test("a " + status + " nest offers Re-run and Edit and re-run", async () => {
  const buttons = {};
  const box = { dataset: {}, style: {}, innerHTML: "", querySelectorAll: () => [],
    querySelector(sel){ const m = /data-act=(\w+)/.exec(sel);
      return m && this.innerHTML.includes("data-act='" + m[1] + "'") ? (buttons[m[1]] = buttons[m[1]] || {}) : null; } };
  const nest = { id: "n1", name: "Nest C", status, stock_width_mm: 100, stock_height_mm: 100,
    spacing_x_mm: 0.4, spacing_y_mm: 0.4, edge_margin_mm: 10, grain_direction: "along_length", sheet_defs: [],
    error: status === "failed" ? "internal error: partial demand: 1 copies could not be placed within 0 sheets" : null };
  const calls = [], routes = [];
  const ctx = loadNest({ go: r => routes.push(r), api: async (m, url) => {
    calls.push(m + " " + url);
    return url.endsWith("/rerun") ? { nest: { status: "queued" } }
      : { detail: { nest, parts: [{ item_id: "x", part_number: "PL-300", quantity: 1 }], sheets: [] } };
  } });
  ctx.dom.els.set("ln-n1", box);
  await ctx.toggleNestDetail({ dataset: { id: "n1" } });
  assert.match(box.innerHTML, status === "failed" ? /No sheets: the run failed\./ : /No sheets: the run was cancelled\./);
  assert.match(box.innerHTML, /spacing 0\.4 × 0\.4 mm · margin 10 mm/);
  assert.equal(buttons.pkg, undefined, "no package without sheets");
  await buttons.rerun.onclick({ stopPropagation(){} });
  assert.equal(calls.at(-1), "POST /laser-nests/n1/rerun");
  buttons.edit.onclick({ stopPropagation(){} });
  assert.equal(ctx.nc().active, true, "Edit and re-run opens the panel");
  assert.equal(ctx.ncPartName("x"), "PL-300", "a part no longer in the picker keeps its part number");
  if (status === "failed") assert.match(ctx.nestErrorText(nest.error), /^1 copy did not fit\. Add a sheet size/);
});

test("Edit and re-run fills the panel from the run, in the display unit", () => {
  const nest = { name: "Nest A", stock_width_mm: 1219.2, stock_height_mm: 2438.4, edge_margin_mm: 10,
    spacing_x_mm: 5.08, spacing_y_mm: 5.08, sheet_defs: [{ widthMm: 1219.2, lengthMm: 2438.4, quantity: null },
      { widthMm: 609.6, lengthMm: 1219.2, quantity: 2 }], grain_direction: "any", nesting_corner: "upper_left",
    guillotine: true, auto_sheet_origins: false, default_part_unit: "mm" };
  const parts = [{ item_id: "b", quantity: 2, ordinal: 1, allow_mirror: true, part_grain: "any", step_angle: 45 },
    { item_id: "a", quantity: 3, ordinal: 0, mirror: true, part_grain: "along_width", common_edge: true }];
  const routes = [];
  const ctx = loadNest({ smDisp: () => "in", go: r => routes.push(r) });
  ctx.editNestAsNew(nest, parts);
  const NC = ctx.nc();
  assert.equal(NC.units, "in");
  assert.deepEqual(plain(NC.sheets), [{ width: 48, length: 96, asNeeded: true, quantity: 1 },
    { width: 24, length: 48, asNeeded: false, quantity: 2 }]);
  assert.equal(NC.margin, 0.393700787);
  assert.equal(NC.spacingX, 0.2);
  assert.deepEqual(plain(NC.lines.map(l => [l.item_id, l.quantity, l.mirror_mode, l.grain, l.step_angle, l.common_edge])),
    [["a", 3, "mirror", "along_width", 90, true], ["b", 2, "both", "any", 45, false]]);
  assert.equal(NC.corner, "upper_left");
  assert.equal(NC.grain, "any");
  assert.equal(NC.guillotine, true);
  assert.equal(NC.autoOrigins, false);
  assert.equal(NC.defaultsApplied, true, "org defaults do not overwrite the run's values");
  assert.equal(NC.partUnit, "auto");
  assert.equal(NC.active, true);
  assert.deepEqual(routes, ["laser-nests"]);
});

test("exactly the nine recorded-only options sit under 'Saved with the run, not applied yet'", () => {
  const ctx = loadNest();
  const NC = ctx.nc(); NC.cands = cands; NC.lines = [ctx.ncDefaultLine("a")];
  const html = ctx.ncSheetsHtml() + ctx.ncPartParamsHtml();
  assert.equal((html.match(/Saved with the run, not applied yet/g) || []).length, 3);
  const advisory = [...html.matchAll(/id='(nc_\w+)'[^>]*aria-describedby='(?:nc_\w+_err )?ncAdv/g)].map(m => m[1]).sort();
  assert.deepEqual(advisory, ["nc_autoorig", "nc_commedge", "nc_fill", "nc_fillall", "nc_fillqty",
    "nc_gull", "nc_minqty", "nc_optpairs", "nc_ssd"]);
});

test("preflight problems show on their part line, not in a passing toast", async () => {
  const toasts = [];
  const ctx = loadNest({ toast: (m) => toasts.push(m), api: async (m, url) => {
    if (url === "/laser-nests/validate") return { ok: false, problems: [{ item_id: "a", problem: "validation error: no flat pattern attached" }],
      mixed_thickness: true };
    throw new Error("should not create");
  } });
  const NC = ctx.nc(); NC.cands = cands; NC.lines = [ctx.ncDefaultLine("a"), ctx.ncDefaultLine("b")];
  ctx.dom.$("nc_name").value = "N";
  await ctx.queueLaserNest();
  assert.deepEqual(toasts, []);
  assert.equal(ctx.nc().tab, "parts");
  assert.match(ctx.nc().preflightMsg, /One part line needs attention.*different thicknesses/);
  const list = ctx.ncPartListHtml();
  assert.match(list, /'>no flat pattern attached — open the part's Sheet metal tab\.</);
  assert.equal((list.match(/nc-lineerr/g) || []).length, 1, "only the line with the problem is marked");
  assert.equal(ctx.dom.$("ncQueue").textContent, "Queue nest");
});

test("nest previews take drawing colors from theme tokens", () => {
  const ctx = loadNest();
  const NC = ctx.nc(); NC.cands = cands;
  const html = ctx.ncSheetPreview(0) + ctx.ncPartPreview();
  const bare = html.replace(/var\(--(?:draw-[\w-]+|canvas),\s*#[0-9a-f]{3,6}\)/gi, "");
  assert.deepEqual(bare.match(/#[0-9a-f]{3,6}\b/gi), null, "every color is a token with today's value as fallback");
  assert.match(html, /stroke:var\(--draw-sheet, #3c3\)/);
  assert.match(html, /background:var\(--draw-hole, #c33\)/);
  assert.match(html, /48 × 96 in/);
});

test("downloads are named after the nest", async () => {
  const saved = [];
  const ctx = loadNest({ api: async () => new Uint8Array(1) });
  ctx.saveBuffer = (buf, name) => saved.push(name);
  await ctx.downloadNestSheet("n", 2, "Brackets 3/4\" run");
  await ctx.downloadNestPackage("n", "Brackets 3/4\" run");
  await ctx.downloadNestPackage("n", "  ");
  assert.deepEqual(saved, ["Brackets 3-4- run - sheet 2.dxf", "Brackets 3-4- run.zip", "laser nest.zip"]);
});
