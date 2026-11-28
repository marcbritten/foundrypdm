// Lifecycle badges, statuses and the event feed in the web portal.
// Run with: node --test crates/pdm-api/tests/web-ui-status.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const read = f => fs.readFileSync(path.join(__dirname, f), "utf8");
const app = read("../static/app.js");
const css = read("../static/style.css");
const html = read("../static/index.html");
const srcDir = path.join(__dirname, "../src");
const routes = read("../src/routes.rs");
// Every server source file in this crate, so audit actions in new modules are covered too.
const sources = fs.readdirSync(srcDir).filter(f => f.endsWith(".rs"))
  .map(f => fs.readFileSync(path.join(srcDir, f), "utf8")).join("\n");

// Like loadFunctions in web-ui.test.cjs, but a function ends at the next top-level
// statement of any kind, and top-level `const` tables can be loaded by name.
function slice(start){
  const rest = app.slice(start + 1);
  const end = rest.search(/\n(?:async function |function |const |let |\/\/)/);
  return app.slice(start, end < 0 ? app.length : start + 1 + end);
}
function load(names, globals){
  const context = vm.createContext(Object.assign({ console }, globals));
  for (const name of names){
    let start = app.indexOf("\nconst " + name + " = ");
    if (start < 0) start = app.indexOf("\nfunction " + name + "(");
    if (start < 0) start = app.indexOf("\nasync function " + name + "(");
    assert(start >= 0, "missing " + name);
    // `var` so the table is also visible as a property of the context.
    vm.runInContext(slice(start + 1).replace(/^const /, "var "), context);
  }
  return context;
}
const BADGE = ["esc", "LIFECYCLE", "ownEntry", "humanize", "lifecycleLabel", "statusBadge", "stateBadge"];
const EVENTS = [...BADGE, "EVENT_LABELS", "DETAIL_LABELS", "UUID_RE", "bytes", "feedTarget", "eventSentence", "capFirst",
  "clipText", "formatDetail", "detailText"];
const users = { u1: { id: "u1", display_name: "Ana Example", email: "ana@example.test" } };
const globals = () => ({ UMAP: users, PMAP: { p1: { name: "Rig" } } });
const UUID = "7d1f0c2a-1b2c-4d5e-8f90-a1b2c3d4e5f6";
const text = h => h.replace(/<[^>]*>/g, "");

test("lifecycle states each get their own word and class; obsolete and released carry an icon", () => {
  const c = load(BADGE, {});
  const cases = { wip: ["WIP", "wip"], in_review: ["In review", "in-review"], released: ["Released", "released"],
    release: ["Released", "released"], obsolete: ["Obsolete", "obsolete"], draft: ["Draft", "draft"] };
  for (const [state, [word, cls]] of Object.entries(cases)){
    const h = c.stateBadge(state);
    assert.equal(text(h), word, state);
    assert.match(h, new RegExp("class='state " + cls + "'"), state);
  }
  assert.match(c.stateBadge("obsolete"), /#ic-ban/);
  assert.match(c.stateBadge("released"), /#ic-check/);
  assert.doesNotMatch(c.stateBadge("wip"), /<svg/);
  assert.equal(text(c.stateBadge(null)), "WIP");
  // Unknown and hostile states are humanised and escaped; the class never comes from the value.
  assert.equal(text(c.stateBadge("on_hold")), "On hold");
  assert.equal(text(c.stateBadge("constructor")), "Constructor");
  const evil = c.stateBadge("x' onmouseover='alert(1)");
  assert.match(evil, /class='state other'/);
  assert.doesNotMatch(evil, /' onmouseover/);
});

test("approval statuses read as words, in the color of what happened", () => {
  const start = app.indexOf("function apprFullTable(");
  const c = load([...BADGE, "APPROVAL_STATES", "apprFullTable"],
    { UMAP: {}, myId: () => "u1", isRole: () => true, fmtTime: () => "now" });
  // The server stores a decided request's status as the decision itself.
  const h = c.apprFullTable([{ id: "a", status: "reject" }, { id: "b", status: "approve" },
    { id: "c", status: "pending" }, { id: "d", status: "withdrawn" }]);
  assert(start > 0);
  assert.match(h, /class='state rejected'>Rejected</);
  assert.match(h, /class='state released'><svg[^]*?#ic-check[^]*?Approved</);
  assert.match(h, /class='state draft'>Pending</);
  assert.match(h, /class='state other'>Withdrawn</);
  assert.doesNotMatch(h, /REJECT|APPROVE|PENDING/);
});

test("every audit action the server records has a plain-language label", () => {
  const actions = new Set();
  for (const m of sources.matchAll(/record_audit\(([^]*?)\.await/g))
    for (const a of m[1].matchAll(/"([a-z_]+\.[a-z_]+)"/g)) actions.add(a[1]);
  // Module helpers that wrap record_audit, e.g. audit(&state, org, admin, "org.x", None).
  for (const m of sources.matchAll(/\baudit\(&state,[^;]*?"([a-z_]+\.[a-z_]+)"/g)) actions.add(m[1]);
  // Codes built at run time.
  for (const a of routes.matchAll(/"(project\.main_assembly_[a-z]+)"/g)) actions.add(a[1]);
  assert.match(routes, /format!\("approval\.\{decision\}"\)/);
  actions.add("approval.approve"); actions.add("approval.reject");
  assert(actions.size > 30, "parser found only " + actions.size + " actions");
  const c = load(EVENTS, globals());
  const missing = [...actions].filter(a => !Object.prototype.hasOwnProperty.call(c.EVENT_LABELS, a));
  assert.deepEqual(missing, []);
  // The feed icons and the labels cover the same codes.
  const icons = [...app.slice(app.indexOf("function feedIcon("), app.indexOf("function feedIcon(") + 2000)
    .matchAll(/"([a-z_]+\.[a-z_]+)":"ic-/g)].map(m => m[1]);
  assert.deepEqual(icons.filter(a => !(a in c.EVENT_LABELS)), []);
});

test("events read as sentences with their object; unknown codes are humanised, never blank", () => {
  const c = load(EVENTS, globals());
  const ev = (action, extra) => Object.assign({ action, object_type: "item", object_id: "i1" }, extra);
  assert.equal(c.eventSentence(ev("item.checkin", { detail: { part_number: "PN-1" } })),
    "checked in <a href='#/items/i1'>PN-1</a>");
  assert.equal(text(c.eventSentence(ev("item.checkout"))), "checked out an item");
  assert.equal(text(c.eventSentence({ action: "approval.requested", object_type: "approval_request",
    detail: { item_id: "i1", part_number: "PN-7" } })), "requested approval for PN-7");
  assert.equal(text(c.eventSentence({ action: "user.login", object_type: "user", object_id: "u1" })), "signed in");
  assert.equal(text(c.eventSentence({ action: "user.token_created", object_type: "user", object_id: "u1" })),
    "created an access token for Ana Example");
  assert.equal(text(c.eventSentence({ action: "approval.reject", object_type: "approval_request",
    detail: { item_id: "i1" } })), "rejected the release of an item");
  assert.equal(text(c.eventSentence({ action: "project.onshape_import", object_type: "item", object_id: "i1",
    detail: { url: "https://cad.example.test/d/1", project_id: "p1", job_id: UUID } })), "imported an item from Onshape");
  assert.equal(text(c.eventSentence({ action: "project.main_assembly_set", object_type: "project", object_id: "p1",
    detail: { project: "Rig", item_id: UUID } })), "set the main assembly of Rig");
  assert.equal(text(c.eventSentence({ action: "project.updated", object_type: "project", object_id: "p1" })),
    "updated project Rig");
  assert.equal(text(c.eventSentence({ action: "laser_nest.cancel", object_type: "laser_nest", detail: "n1" })),
    "cancelled a laser nest");
  assert.equal(text(c.eventSentence({ action: "widget.part_moved", object_type: "approval" })),
    "Widget part moved · an approval");
  for (const a of ["", null, undefined, "toString", "__proto__", "constructor"]){
    const s = text(c.eventSentence({ action: a }));
    assert(s.trim().length > 0, "blank sentence for " + a);
    assert.doesNotMatch(s, /function|\[object/);
  }
  assert.equal(c.capFirst(c.eventSentence(ev("item.comment", { detail: { part_number: "P" } }), "this item")),
    "Commented on this item");
});

test("event detail shows readable pairs: no ids, hashes or JSON; user ids become names", () => {
  const c = load(EVENTS, globals());
  const pairs = d => JSON.parse(JSON.stringify(c.formatDetail({ detail: d })));
  assert.deepEqual(pairs({ from: "in_review", to: "obsolete" }), [["State", "In review to Obsolete"]]);
  assert.deepEqual(pairs({ commit_message: "first cut", revision: 1, sha256: "ab12", version: 3, version_seq: 3 }),
    [["Note", "first cut"], ["Version", "3"]]);
  assert.deepEqual(pairs({ revision: "B", version_seq: 4 }), [["Revision", "B"], ["Version", "4"]]);
  assert.deepEqual(pairs({ decision: "reject", item_id: "i1", note: "gap too small" }), [["Note", "gap too small"]]);
  assert.deepEqual(pairs({ workstation: "bench", base_version_id: "v9" }), [["Workstation", "bench"]]);
  assert.deepEqual(pairs({ assignees: ["u1"] }), [["Assignees", "Ana Example"]]);
  assert.deepEqual(pairs({ assignees: ["u1", "zz"] }), [["Assignees", "2 people"]]);
  assert.deepEqual(pairs({ assignees: [] }), []);
  assert.deepEqual(pairs({ display_name: "Bo", role: "editor", active: false }),
    [["Name", "Bo"], ["Role", "editor"], ["Active", "no"]]);
  assert.deepEqual(pairs({ some_new_key: 5, nested: { a: 1 }, refs: [UUID], tags: ["a", "b"] }),
    [["Some new key", "5"], ["Tags", "a, b"]]);
  // No ids, even under keys this table does not know (an approved release stores its request id).
  assert.deepEqual(pairs({ revision: "A", version_seq: 1, approval_request: UUID }), [["Revision", "A"], ["Version", "1"]]);
  assert.deepEqual(pairs({ owner_id: "nobody-known" }), []);
  assert.deepEqual(pairs({ owner_id: "u1", description: "Lid" }), [["Owner", "Ana Example"], ["Description", "Lid"]]);
  assert.deepEqual(pairs({ filename: "a.step", size_bytes: 2048, item_id: UUID }), [["Size", "2.0 KB"]]);
  // What the sentence already names is not repeated as a pair.
  assert.deepEqual(pairs({ part_number: "PN-1" }), []);
  assert.deepEqual(pairs({ name: "Brackets" }), []);
  assert.deepEqual(JSON.parse(JSON.stringify(c.formatDetail({ object_type: "project", detail: { project: "Rig", item_id: UUID } }))), []);
  assert.deepEqual(JSON.parse(JSON.stringify(c.formatDetail({ object_type: "item",
    detail: { url: "https://cad.example.test/d/1", project_id: "p1", job_id: UUID } }))),
    [["Link", "https://cad.example.test/d/1"], ["Project", "Rig"]]);
  // A detail that is not an object (laser nests store a bare id) renders nothing, without throwing.
  for (const d of ["5f0c-uuid", 42, null, undefined, [1, 2], true]) assert.deepEqual(pairs(d), []);
});

test("event text is escaped, and long values are cut before escaping", () => {
  const c = load(EVENTS, globals());
  const hostile = "<img src=x onerror=alert(1)>";
  const out = c.detailText(c.formatDetail({ detail: { commit_message: hostile, [hostile]: "v" } }));
  assert.doesNotMatch(out, /<img/);
  assert.match(out, /&lt;img src=x onerror=alert\(1\)&gt;/);
  const s = c.eventSentence({ action: "item.created", object_type: "item", object_id: "i'1",
    detail: { part_number: hostile } });
  assert.doesNotMatch(s, /<img|'1'/);
  const long = c.detailText(c.formatDetail({ detail: { body: "&".repeat(300) } }));
  assert.match(long, /^Comment: (&amp;){119}…$/);
});

test("feed, item Activity tab and audit log show sentences, not codes or JSON", async () => {
  const ev = { action: "item.lifecycle_transitioned", object_type: "item", object_id: "i1", actor_id: "u1",
    at: new Date().toISOString(), detail: { from: "wip", to: "in_review", part_number: "PN-2" } };
  const base = [...EVENTS, "fmtTime", "fmtDate", "actorName", "kindSvg", "feedIcon", "feedDayLabel", "emptyState"];
  const c = load([...base, "feed", "auditTable"], globals());
  const f = c.feed([ev]);
  assert.match(text(f), /changed the state of PN-2 · State: WIP to In review/);
  assert.doesNotMatch(f, /lifecycle_transitioned|class='mono'/);
  const a = c.auditTable([ev]);
  assert.match(text(a), /Changed the state of PN-2/);
  assert.match(a, /<details class='rawev'><summary>Raw event<\/summary>/);
  assert.match(a, /&quot;action&quot;: &quot;item.lifecycle_transitioned&quot;/);
  const box = {};
  const t = load([...base, "tabActivity"], Object.assign(globals(), { DETAIL: { id: "i1" },
    api: async () => ({ events: [ev, { action: "item.checkin", object_type: "item", object_id: "i1",
      detail: { sha256: "ffff", version_seq: 2, commit_message: "thinner wall" } }] }) }));
  await t.tabActivity(box);
  assert.match(text(box.innerHTML), /Changed the state of this item/);
  assert.match(text(box.innerHTML), /Checked in this item(Version: 2 · Note: thinner wall|Note: thinner wall · Version: 2)/);
  assert.doesNotMatch(box.innerHTML, /\{|item\.checkin|ffff/);
});

test("version compare marks changed rows with a word and bold values", () => {
  const c = load(["esc", "bytes", "fmtDate", "kindInfo", "kindSvg", "updateCompare"],
    { CAD_EXTS: [], GCODE_EXTS: [], DETAIL: { cmp: new Set(["a", "b"]) } });
  const cbox = { innerHTML: "" };
  c.updateCompare({ querySelector: () => cbox }, [
    { id: "a", seq: 2, size_bytes: 10, author_name: "Ana", mime_type: "model/stl" },
    { id: "b", seq: 1, size_bytes: 20, author_name: "Ana", mime_type: "model/stl" }]);
  const rows = cbox.innerHTML.split("<tr").slice(2);
  const size = rows.find(r => r.includes(">size<")), author = rows.find(r => r.includes(">author<"));
  assert.match(size, /<b>10 B<\/b>.*<b>20 B<\/b>.*<b>Changed<\/b>/);
  assert.doesNotMatch(author, /Changed|<b>/);
});

test("manufacturing steps: done has a check, the current step is marked, nothing pulses", () => {
  const c = load(["esc", "MFG_STEPS", "MFG_STEP_LABEL", "mfgStepper"], {});
  const h = c.mfgStepper("being_machined");
  assert.match(h, /<span class='s done'><svg class='icon' aria-hidden='true'><use href='#ic-check'\/><\/svg>needs CAD/);
  assert.match(h, /<span class='s active' aria-current='step'>machining/);
  assert.match(h, /<span class='s queued'>inspection/);
  assert.equal((h.match(/aria-current/g) || []).length, 1);
  assert.doesNotMatch(css, /animation:\s*pulse|@keyframes pulse/);
  const rule = sel => (css.match(new RegExp("(^|\\n)" + sel.replace(/[.*]/g, "\\$&") + "[^{]*\\{([^}]*)\\}")) || [])[2] || "";
  assert.match(rule(".stepper .s.queued"), /color:var\(--dim\)/);
  assert.match(rule(".stepper .s.active"), /font-weight:600/);
});

test("status colors come from tokens; states differ by more than color", () => {
  const rule = sel => {
    const m = css.match(new RegExp("(^|\\n)" + sel.replace(/[.]/g, "\\.") + "\\s*\\{([^}]*)\\}"));
    assert(m, "missing rule " + sel);
    return m[2];
  };
  assert.match(rule(".state.in-review"), /color:var\(--life-in-review, var\(--acc\)\)/);
  assert.match(rule(".state.released"), /color:var\(--life-released, var\(--ok\)\)/);
  assert.match(rule(".state.wip"), /color:var\(--life-wip, var\(--dim\)\)/);
  assert.match(rule(".state.obsolete"), /dashed/);
  assert.match(rule(".state.obsolete"), /var\(--life-obsolete, var\(--dim\)\)/);
  assert.match(css, /\n\.state\.failed, \.state\.rejected \{ color:var\(--err\); border-color:var\(--err\); \}/);
  assert.match(rule(".state.outdated"), /color:var\(--warn\)/);
  assert.match(css, /\n\.errtext, \.dim\.errtext \{ color:var\(--err\); \}/);
  // The "Raw event" toggle shows the same focus ring as other controls.
  assert.match(css, /\n\.rawev summary:focus-visible \{ outline:2px solid var\(--acc\);/);
  // Failed inspections and outdated programs no longer use the neutral WIP style.
  assert.match(app, /<span class='state failed'>failed<\/span>/);
  assert.match(app, /<span class='state outdated' title=/);
  assert.doesNotMatch(app, /#d98/);
  assert.match(app, /<p class='dim errtext'>" \+ esc\(cli\.error\)/);
  for (const id of ["ic-check", "ic-ban"]) assert.match(html, new RegExp("<symbol id=\"" + id + "\""));
});
