// Arrange my menu checks for the web portal: moves, group rules, the menu the sidebar shows, the
// icon search and the bundled icon files.
// Run with: node --test crates/pdm-api/tests/web-ui-arrange-menu.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const STATIC = path.join(__dirname, "../static");
const app = fs.readFileSync(path.join(STATIC, "app.js"), "utf8");
const indexHtml = fs.readFileSync(path.join(STATIC, "index.html"), "utf8");
const css = fs.readFileSync(path.join(STATIC, "style.css"), "utf8");
const menuRs = fs.readFileSync(path.join(__dirname, "../src/menu.rs"), "utf8");

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
const plain = v => JSON.parse(JSON.stringify(v));
const AM = ["amLevels", "amList", "amCan", "amMove", "amPathOf", "amMoveBefore", "amMoveIntoEmpty", "amRemoveGroup", "amGroups", "amNameProblem"];
const menu = () => [
  { group: "Workspace", key: "workspace", items: [{ nav: "dashboard" }, { nav: "projects" }] },
  { group: "Mine", items: [{ group: "Two", items: [{ group: "Three", items: [{ nav: "jobs" }] }] }] },
  { nav: "items" },
];

test("Up, Down, Into group above and Out of group move an entry and say where it went", () => {
  const c = loadFunctions(AM, {}), m = menu();
  assert.deepEqual(plain(c.amMove(m, [0, 1], "up")), [0, 0]);
  assert.deepEqual(m[0].items.map(n => n.nav), ["projects", "dashboard"]);
  assert.equal(c.amMove(m, [0, 0], "up"), null, "the first entry cannot go up");
  assert.equal(c.amMove(m, [2], "down"), null, "the last entry cannot go down");
  assert.deepEqual(plain(c.amMove(m, [2], "into")), [1, 1], "Items goes to the end of the group above");
  assert.deepEqual(m[1].items.map(n => n.nav || n.group), ["Two", "items"]);
  assert.deepEqual(plain(c.amMove(m, [1, 1], "out")), [2], "out of a group lands just after it");
  assert.equal(c.amCan(m, [0], "out"), false, "a top entry is in no group");
  assert.equal(c.amCan(m, [0], "into"), false, "no group above the first entry");
});

test("groups go three levels deep at most, and a group cannot go into itself", () => {
  const c = loadFunctions(AM, {}), m = menu();
  m.push({ group: "Last", items: [] });
  // Order: Workspace, Mine (3 levels), items, Last.
  assert.equal(c.amLevels(m[1]), 3);
  assert.equal(c.amCan(m, [1], "into"), false, "Mine into Workspace would make a fourth level");
  assert.equal(c.amCan(m, [2], "into"), true, "a link can go into a group");
  assert.equal(c.amCan(m, [3], "into"), false, "Last is below a link, not a group");
  assert.equal(c.amCan([{ group: "A", items: [] }, { group: "C", items: [{ group: "D" }] }], [1], "into"), true,
    "two levels into a top group makes three");
  assert.equal(c.amMoveBefore(m, [1], [1, 0]), null, "not into itself");
  assert.equal(c.amMoveBefore(m, [1], [0, 0]), null, "three levels cannot go one level down");
  assert.deepEqual(plain(c.amMoveBefore(m, [2], [1])), [1], "Items dragged before Mine");
  assert.equal(m[1].nav, "items");
});

test("a row dropped on an empty group goes into it, within three levels", () => {
  const c = loadFunctions(AM, {}), m = menu();
  m.push({ group: "Empty", items: [] });
  assert.deepEqual(plain(c.amMoveIntoEmpty(m, [2], [3])), [2, 0], "Items into Empty");
  assert.deepEqual(m[2].items.map(n => n.nav), ["items"]);
  m.push({ group: "Empty 2", items: [] });
  assert.equal(c.amMoveIntoEmpty(m, [1], [3]), null, "three levels cannot go one level down");
  assert.equal(c.amMoveIntoEmpty(m, [2], [2, 0]), null, "a link is not a group");
  assert.equal(c.amMoveIntoEmpty(m, [1], [1, 0]), null, "not into itself");
});

test("Remove group moves its entries up a level, where the group was", () => {
  const c = loadFunctions(AM, {}), m = menu();
  assert.equal(c.amRemoveGroup(m, [0]), true);
  assert.deepEqual(m.map(n => n.nav || n.group), ["dashboard", "projects", "Mine", "items"]);
  assert.equal(c.amRemoveGroup(m, [0]), false, "a link is not a group");
});

test("a group name needs text, 60 characters at most, no markup and no twin", () => {
  const c = loadFunctions(AM, {}), m = menu();
  assert.equal(c.amNameProblem("Shop floor", m), "");
  assert.match(c.amNameProblem("   ", m), /Type a name/);
  assert.match(c.amNameProblem("x".repeat(61), m), /60 characters/);
  assert.equal(c.amNameProblem("x".repeat(60), m), "");
  assert.match(c.amNameProblem("<b>", m), /Leave out < and >/);
  assert.match(c.amNameProblem("three", m), /You have a group with this name/, "names are compared without case, at every level");
  assert.equal(c.amNameProblem("Three", m, m[1].items[0].items[0]), "", "a group keeps its own name on rename");
  // The server checks the same limits.
  assert.match(menuRs, /const MAX_NAME_CHARS: usize = 60;/);
  assert.match(menuRs, /const MAX_DEPTH: usize = 3;/);
});

test("the sidebar shows the stored menu with every link once, and new links join their default group", () => {
  const base = { groups: [
    { key: "workspace", label: "Workspace", icon: "<svg class='icon'><use href='#ic-grid'/></svg>", items: ["dashboard", "projects"] },
    { key: "data", label: "Data", icon: "", items: ["items", "search"] },
  ], links: { dashboard: {}, projects: {}, items: {}, search: {} }, icons: {} };
  const c = loadFunctions(["navDefaultItems", "navFindGroup", "navEffectiveItems"], {});
  assert.deepEqual(plain(c.navEffectiveItems(null, base)), [
    { group: "Workspace", key: "workspace", items: [{ nav: "dashboard" }, { nav: "projects" }] },
    { group: "Data", key: "data", items: [{ nav: "items" }, { nav: "search" }] },
  ], "no stored menu: the default");
  const stored = [
    { group: "Mine", items: [{ nav: "search", hidden: true }, { nav: "search" }, { nav: "gone-page" }] },
    { group: "Workspace", key: "workspace", items: [] },
  ];
  assert.deepEqual(plain(c.navEffectiveItems(stored, base)), [
    { group: "Mine", items: [{ nav: "search", hidden: true }] },
    { group: "Workspace", key: "workspace", items: [{ nav: "dashboard" }, { nav: "projects" }] },
    { nav: "items" },
  ], "a link twice or unknown drops out; missing links join their group or the end");
});

test("custom icons come only from the bundled sprites, and the search finds by name and word", () => {
  const c = loadFunctions(["faIconSvg", "searchIcons"], {});
  assert.equal(c.faIconSvg("solid/house"), "<svg class='icon fa' aria-hidden='true'><use href='/static/icons/solid.svg#house'/></svg>");
  assert.match(c.faIconSvg("regular/bell"), /regular\.svg#bell/);
  for (const bad of ["house", "solid/../x", "solid/a'b", "brands/github", "", null]) assert.equal(c.faIconSvg(bad), "", String(bad));
  const index = JSON.parse(fs.readFileSync(path.join(STATIC, "icons/index.json"), "utf8"));
  const r = c.searchIcons(index, "house", 10);
  assert.equal(r.icons[0], "solid/house", "the exact name comes first");
  assert.ok(r.total > 5);
  assert.ok(c.searchIcons(index, "home", 50).icons.includes("solid/house"), "aliases and search words count");
  assert.ok(c.searchIcons(index, "bell", 50).icons.includes("regular/bell"), "regular icons are listed too");
  assert.deepEqual(plain(c.searchIcons(index, "  ", 10)), { total: 0, icons: [] });
  assert.equal(c.searchIcons(index, "zzqqxx", 10).total, 0);
  const many = c.searchIcons(index, "a", 60);
  assert.equal(many.icons.length, 60, "the list stops at the limit");
  assert.ok(many.total > 60);
});

test("the bundled icon files carry the credit and the server knows the same names", () => {
  const index = JSON.parse(fs.readFileSync(path.join(STATIC, "icons/index.json"), "utf8"));
  assert.equal(index.version, "7.3.1");
  const solid = fs.readFileSync(path.join(STATIC, "icons/solid.svg"), "utf8");
  const regular = fs.readFileSync(path.join(STATIC, "icons/regular.svg"), "utf8");
  for (const svg of [solid, regular]) assert.match(svg, /Font Awesome Free 7\.3\.1[\s\S]*CC BY 4\.0/);
  assert.match(fs.readFileSync(path.join(STATIC, "icons/LICENSE.txt"), "utf8"), /Font Awesome Free License/);
  const ids = svg => new Set([...svg.matchAll(/<symbol id="([a-z0-9-]+)"/g)].map(m => m[1]));
  const s = ids(solid), rg = ids(regular);
  for (const [name, styles] of index.icons){
    assert.equal(styles.includes("s"), s.has(name), name);
    assert.equal(styles.includes("r"), rg.has(name), name);
  }
  assert.match(menuRs, /include_str!\("\.\.\/static\/icons\/index\.json"\)/);
  // The app update image copies the icons folder.
  assert.match(fs.readFileSync(path.join(__dirname, "../../../docker/Dockerfile.app-update"), "utf8"), /COPY --from=portal \/icons \/opt\/pdm\/portal\/icons\//);
  // The sprites load only when needed: no page links them up front.
  assert.doesNotMatch(indexHtml, /static\/icons\//);
});

test("Arrange my menu is a route, linked from the account menu, and its buttons follow the rules", () => {
  assert.match(app, /const NAV_NAMES = \[[^\]]*"menu"\]/);
  assert.match(app, /case "menu": await viewArrangeMenu\(v\); break;/);
  assert.match(app, /\{ label: "Arrange my menu", act: \(\) => \{/);
  const view = app.slice(app.indexOf("async function viewArrangeMenu("), app.indexOf("function loadIconIndex("));
  for (const words of ["Put your screens in the order you want. Make your own groups. Groups can go three levels deep.",
    "This is your own menu.", "Close my other groups when I open a group", "Name of the new group", "Add the group",
    "Save my menu", "Reset my menu", "Cancel"]) assert.ok(view.includes(words), words);
  const rows = app.slice(app.indexOf("function amRowsHtml("), app.indexOf("async function viewArrangeMenu("));
  for (const label of ["Up", "Down", "Into group above", "Out of group", "Show", "Hide", "Rename", "Remove group", "Change icon"])
    assert.ok(rows.includes("\"" + label + "\""), label);
  assert.match(rows, /aria-pressed/, "Show and Hide say which is on");
  assert.match(rows, /aria-describedby/, "each button names its row");
  const picker = app.slice(app.indexOf("function pickIcon("), app.indexOf("const NAV_MENU ="));
  for (const words of ["Find an icon for ", "Type part of an icon's name, such as user or folder. Then click the icon you want.",
    "Use the default", "Close", "Clear the search", ":scope > .am-row\").after(panel)", "opener.focus()", "Escape"]) assert.ok(picker.includes(words), words);
  assert.ok(!picker.includes("showModal"), "the picker is a panel under the row, not a dialog");
  assert.ok(picker.includes("(current ? \"\" : \" disabled\")"), "Use the default is off when the entry has its default icon");
  for (const words of ["This group is empty.", "Drag a row by its handle, or use the buttons next to it.",
    "The groups that hold the open group stay open. The portal saves this choice at once."]) assert.ok(app.includes(words), words);
  assert.ok(rows.includes("(p.length > 1 ? btn(\"out\""), "Out of group shows only inside a group");
  assert.match(css, /\.icon\.fa \{ stroke:none; \}/);
  // BX10: text fields on this page are 16px at least under a touch pointer (--fs-lg is 1rem).
  assert.ok(css.includes("@media (pointer: coarse) { .am-add input, .ip-panel input { font-size:var(--fs-lg); }"));
  assert.ok(css.includes(".am-sub { margin:0 0 0 .85em; padding-left:.85em; border-left:1px solid var(--line); }"), "indent with a guide line");
  assert.ok(css.includes(".am-tag { font-size:var(--fs-sm); color:var(--dim); }"), "state words are plain dim text");
  assert.match(css, /--fs-lg:1rem;/);
  // BX19: a failed save keeps the draft and says so beside the buttons.
  assert.ok(view.includes('fail("Your menu was not saved: " + e.message + ". Your changes are still here. Try again.")'));
  assert.doesNotMatch(app.slice(app.indexOf("function navBase("), app.indexOf("const NAV_MENU =")), /;\s*\/\/[^\n]*;/);
});

test("opening a group closes the others when the option is on, and keeps the groups that hold it", () => {
  const sections = [];
  const sec = (id, parent) => {
    const classes = new Set(), attrs = {};
    const s = { dataset: { group: id }, parent,
      classList: { toggle(c, on){ if (on) classes.add(c); else classes.delete(c); }, contains: c => classes.has(c) },
      contains(o){ for (let x = o; x; x = x.parent) if (x === s) return true; return false; } };
    s.head = { setAttribute(k, v){ attrs[k] = v; }, getAttribute: k => attrs[k] };
    s.querySelector = () => s.head;
    sections.push(s);
    return s;
  };
  const a = sec("a"), a1 = sec("g:a/1", a), b = sec("b"), c = sec("c");
  const store = new Map();
  const ctx = loadFunctions(["navFoldedGroups", "setNavGroupFolded", "wireNavGroups"], {
    NAV_MENU: { close_others: true },
  });
  const storage = { getItem: k => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) };
  ctx.wireNavGroups({ querySelectorAll: () => sections }, storage);
  for (const s of [a, a1, b, c]) s.head.onclick();                 // fold all four
  assert.ok([a, a1, b, c].every(s => s.classList.contains("folded")));
  b.head.onclick();                                                 // open b: nothing else opens
  a.head.onclick();                                                 // open a: b closes
  assert.ok(!a.classList.contains("folded") && b.classList.contains("folded"));
  a1.head.onclick();                                                // open a1: a holds it and stays open
  assert.ok(!a.classList.contains("folded") && !a1.classList.contains("folded"));
  ctx.NAV_MENU.close_others = false;
  c.head.onclick();
  assert.ok(!a.classList.contains("folded") && !c.classList.contains("folded"), "option off: others stay open");
});
