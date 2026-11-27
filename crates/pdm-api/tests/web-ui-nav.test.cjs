// Run with: node --test crates/pdm-api/tests/web-ui-nav.test.cjs
// Navigation, list state in the address, full item lists, keyboard markup.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8");
const html = fs.readFileSync(path.join(__dirname, "../static/index.html"), "utf8");

function loadFunctions(names, globals){
  const context = vm.createContext(globals);
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
// Top-level `const NAME = [...]` arrays from app.js, so tests use the real lists.
function constArray(name){
  const m = app.match(new RegExp("const " + name + " = (\\[[\\s\\S]*?\\]);"));
  assert(m, "missing const " + name);
  return vm.runInNewContext(m[1]);
}
const esc = s => String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));

function router(route, hash){
  const ctx = loadFunctions(["routeToHash", "routeFromHash", "hashQuery", "sameRoute", "detailTabs"], {
    ROUTE: route, DETAIL: { tab:"overview", revs:[], comments:[], data:null }, TOOL: { tab:"general" },
    location: { hash: hash || "" },
    NAV_NAMES: constArray("NAV_NAMES"), ITEMS_SORTS: constArray("ITEMS_SORTS"),
    EXPLORER_SORTS: constArray("EXPLORER_SORTS"),
  });
  return ctx;
}

test("Items filters, sort and page survive a round trip through the address", () => {
  const r = { name:"items", origin:"items", _kind:"cad", _cls:"ASM", _sm:"on", _q:"bracket & plate",
    _sort:"part_number", _dir:"asc", _page:3 };
  const ctx = router(r);
  const h = ctx.routeToHash(r);
  assert.equal(h, "#/items?kind=cad&cls=ASM&sm=on&q=bracket%20%26%20plate&sort=part_number&dir=asc&page=3");
  const back = ctx.routeFromHash(h);
  for (const k of ["_kind", "_cls", "_sm", "_q", "_sort", "_dir", "_page"]) assert.equal(back[k], r[k], k);
  assert.equal(ctx.sameRoute(back), true);
  // Back between two filter states of the same view must count as a change.
  assert.equal(ctx.sameRoute(ctx.routeFromHash("#/items?kind=cad&cls=ASM&sm=on&q=bracket%20%26%20plate&sort=part_number&dir=asc&page=2")), false);
  assert.equal(ctx.sameRoute(ctx.routeFromHash("#/items?kind=cad&cls=ASM&sm=on&sort=part_number&dir=asc&page=3")), false);
});

test("Defaults stay out of the address and unknown values fall back", () => {
  const ctx = router({ name:"items", origin:"items" });
  assert.equal(ctx.routeToHash({ name:"items", _kind:"all", _cls:"all", _sm:"all", _q:"",
    _sort:"modified_at", _dir:"desc", _page:1 }), "#/items");
  const r = ctx.routeFromHash("#/items?sort=drop_table&dir=sideways&page=-4&sm=maybe");
  assert.equal(r._sort, "modified_at");
  assert.equal(r._dir, "desc");
  assert.equal(r._page, 1);
  assert.equal(r._sm, "all");
  assert.equal(ctx.sameRoute(r), true, "plain #/items equals the defaults");
});

test("Project explorer keeps tab, folder, filter and sort in the address", () => {
  const r = { name:"explorer", origin:"projects", pid:"p1", _ptab:"items", _folder:"f9",
    _q:"hinge", _sort:"_name", _dir:"asc" };
  const ctx = router(r);
  const h = ctx.routeToHash(r);
  assert.equal(h, "#/projects/p1?folder=f9&q=hinge&sort=_name&dir=asc");
  const back = ctx.routeFromHash(h);
  assert.equal(back.pid, "p1");
  assert.equal(back._folder, "f9");
  assert.equal(back._q, "hinge");
  assert.equal(back._sort, "_name");
  assert.equal(ctx.sameRoute(back), true);
  assert.equal(ctx.sameRoute(ctx.routeFromHash("#/projects/p1?q=hinge&sort=_name&dir=asc")), false);
  // The removed placeholder tab opens the items tab instead.
  assert.equal(ctx.routeFromHash("#/projects/p1?tab=bom")._ptab, undefined);
  assert.equal(ctx.routeFromHash("#/projects/p1?tab=settings")._ptab, "settings");
});

test("Laser nests and jobs are routes; a nest can be linked", () => {
  const ctx = router({ name:"dashboard" });
  assert.equal(ctx.routeFromHash("#/laser-nests").name, "laser-nests");
  assert.equal(ctx.routeFromHash("#/jobs").name, "jobs");
  const n = ctx.routeFromHash("#/laser-nests/abc-123");
  assert.equal(n.name, "laser-nests");
  assert.equal(n.nest, "abc-123");
  assert.equal(ctx.routeToHash(n), "#/laser-nests/abc-123");
});

test("Item tabs: placeholders are gone, Permissions is Access, old links open Overview", () => {
  const ctx = router({ name:"dashboard" });
  const keys = ctx.detailTabs().map(t => t[0]);
  assert.ok(!keys.includes("deps") && !keys.includes("bom"));
  assert.equal(ctx.detailTabs().find(t => t[0] === "perm")[1], "Access");
  assert.equal(ctx.routeFromHash("#/items/i1?tab=deps")._tab, "overview");
  // Back from ?tab=versions to the plain item address must select Overview again.
  assert.equal(ctx.routeFromHash("#/items/i1")._tab, "overview");
  assert.equal(ctx.routeFromHash("#/tools/t1")._tab, "general");
  assert.equal(ctx.routeFromHash("#/items/i1?tab=versions")._tab, "versions");
});

test("fetchAllItems follows has_more, dedupes and reports the total", async () => {
  const calls = [];
  const pages = { 1:["a","b"], 2:["b","c"], 3:["d"] };
  const ctx = loadFunctions(["fetchAllItems"], {
    api: async (m, url) => {
      calls.push(url);
      const p = Number(/page=(\d+)/.exec(url)[1]);
      return { items: pages[p].map(id => ({ id })), total: 4, has_more: p < 3 };
    },
  });
  const d = await ctx.fetchAllItems("/projects/p/items");
  assert.deepEqual(Array.from(d.items, i => i.id), ["a", "b", "c", "d"]);
  assert.equal(d.total, 4);
  assert.equal(d.complete, true);
  assert.deepEqual(calls, ["/projects/p/items?limit=200&page=1", "/projects/p/items?limit=200&page=2",
    "/projects/p/items?limit=200&page=3"]);
  // Stops when the user has moved on, or at the page bound, and says so.
  calls.length = 0;
  const gone = await ctx.fetchAllItems("/items?q=x", () => false);
  assert.equal(calls.length, 1);
  assert.equal(calls[0], "/items?q=x&limit=200&page=1");
  assert.equal(gone.complete, false);
  calls.length = 0;
  const capped = await ctx.fetchAllItems("/items", null, 2);
  assert.equal(calls.length, 2);
  assert.equal(capped.complete, false);
});

test("Project item counts come from the server total, not a capped list", async () => {
  const ctx = loadFunctions(["projectItemCounts"], {
    api: async (m, url) => {
      if (url.startsWith("/projects/bad/")) throw new Error("boom");
      assert.match(url, /limit=1&include_archived=true$/);
      return { items:[{ id:"x" }], total: url.includes("/big/") ? 312 : 2 };
    },
  });
  const c = await ctx.projectItemCounts([{ id:"big" }, { id:"small" }, { id:"bad" }]);
  assert.equal(c.big, 312);
  assert.equal(c.small, 2);
  assert.equal(c.bad, undefined);
});

test("Item rows: the name is a link with its full text; sort state is announced", () => {
  const rows = new Map();
  const ctx = loadFunctions(["itemsTable"], {
    esc, ITEM_ROWS: rows, PMAP:{},
    kindInfo: () => ({ icon:"", label:"CAD", cls:"cad" }), kindBadge: () => "<span class='badge'>CAD</span>",
    itemType: () => "Solid", smBadge: () => "", archBadge: () => "", stateBadge: s => s,
    revBadge: l => "<span class='rev'>" + l + "</span>", itemName: it => it.description,
    fmtTime: () => "now",
  });
  const long = "Bracket with a very long descriptive name that will not fit";
  const out = ctx.itemsTable([{ id:"i1", description:long, part_number:"BE-000123", head_version_seq:3,
    latest_revision:"B", lifecycle_state:"wip" }], false, new Set(), { key:"part_number", dir:"asc" });
  assert.match(out, /<a class='nm' href='#\/items\/i1' data-fk='item:i1' title='Bracket with a very long descriptive name that will not fit'>/);
  assert.match(out, /<td class='mono pn'>BE-000123<\/td>/);
  assert.match(out, /<th class='sortable' data-s='part_number' aria-sort='ascending'><button type='button' class='thsort'/);
  assert.match(out, /v3 <span class='rev'>B<\/span>/);
  assert.match(out, /aria-label='Select Bracket/);
  assert.equal(rows.get("i1").part_number, "BE-000123", "row kept for its menu");
  // Tables whose caller does not sort get plain headings, not dead buttons.
  const plain = ctx.itemsTable([{ id:"i2", description:"x", part_number:"P" }], false);
  assert.doesNotMatch(plain, /thsort|sortable/);
});

test("Tab strips are buttons in a tablist; only the selected tab is a tab stop", () => {
  const ctx = loadFunctions(["tabStripHtml"], {});
  const out = ctx.tabStripHtml([["a","Alpha"],["b","Beta",2]], "b", "t");
  assert.match(out, /^<div class='tabs' role='tablist'>/);
  assert.match(out, /<button type='button' role='tab' class='tab' aria-selected='false' tabindex='-1' data-fk='tab:a' data-t='a'>Alpha<\/button>/);
  assert.match(out, /<button type='button' role='tab' class='tab on' aria-selected='true' tabindex='0' data-fk='tab:b' data-t='b'>Beta <span class='count'>2<\/span><\/button>/);
});

test("Arrow keys, Home and End move focus along a tab strip; Enter selects", () => {
  const clicked = [];
  const mk = (i) => ({ i, focus(){ ctx.focused = this; }, click(){ clicked.push(this.i); },
    matches: s => s === "[role=tab]", tagName:"BUTTON" });
  const tabs = [mk(0), mk(1), mk(2)];
  const list = { querySelectorAll: () => tabs };
  for (const t of tabs) t.closest = () => list;
  const ctx = loadFunctions(["kbdKeys"], { KBD_CLICK:".x" });
  const press = (t, key) => { let prevented = false;
    ctx.kbdKeys({ target:t, key, preventDefault(){ prevented = true; } }); return prevented; };
  assert.equal(press(tabs[0], "ArrowRight"), true);
  assert.equal(ctx.focused.i, 1);
  press(tabs[0], "ArrowLeft");
  assert.equal(ctx.focused.i, 2, "wraps to the end");
  press(tabs[1], "Home");
  assert.equal(ctx.focused.i, 0);
  press(tabs[0], "End");
  assert.equal(ctx.focused.i, 2);
  assert.deepEqual(clicked, [], "arrows only move focus (panels load from the server)");
  assert.equal(press(tabs[0], "a"), false);
});

test("Sort direction words follow the key", () => {
  const ctx = loadFunctions(["dirOptions", "defaultDir"], {});
  assert.match(ctx.dirOptions("modified_at"), /Newest first[\s\S]*Oldest first/);
  assert.match(ctx.dirOptions("_name"), /A to Z[\s\S]*Z to A/);
  assert.equal(ctx.defaultDir("part_number"), "asc");
  assert.equal(ctx.defaultDir("created_at"), "desc");
});

test("Sidebar and bottom bar links are real links to their routes", () => {
  const links = [...html.matchAll(/<a class="(?:sb-link|bb-link)[^"]*"([^>]*)>/g)].map(m => m[1]);
  assert.ok(links.length >= 20);
  for (const attrs of links){
    const nav = /data-nav="([^"]+)"/.exec(attrs)[1];
    assert.match(attrs, new RegExp("href=\"#/" + nav + "\""), nav);
  }
  assert.match(html, /href="#\/jobs" data-nav="jobs"/);
});

test("Item page Back returns to the list it was opened from", () => {
  const seen = [];
  const route = { name:"detail", id:"i1", origin:"items", _back:"#/items?q=plate&page=2" };
  const ctx = loadFunctions(["leaveDetail"], {
    ROUTE: route,
    routeFromHash: h => ({ name:"items", origin:"items", _q:"plate", _page:2, from:h }),
    syncNav(){ seen.push("sync"); }, render(full){ seen.push("render:" + full); },
    pushHash(){ seen.push("push"); }, go(){ seen.push("go"); },
  });
  ctx.leaveDetail();
  assert.equal(ctx.ROUTE.name, "items");
  assert.equal(ctx.ROUTE._page, 2);
  assert.deepEqual(seen, ["sync", "render:true", "push"]);
  // Opened from a link with no list behind it: fall back to the origin section.
  seen.length = 0;
  ctx.ROUTE = { name:"detail", id:"i1", origin:"items" };
  ctx.leaveDetail();
  assert.deepEqual(seen, ["go"]);
});
