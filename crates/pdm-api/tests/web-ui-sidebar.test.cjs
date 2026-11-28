// Sidebar checks for the web portal: folding groups, the icon rail, the phone drawer and the current page.
// Run with: node --test crates/pdm-api/tests/web-ui-sidebar.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const STATIC = path.join(__dirname, "../static");
const app = fs.readFileSync(path.join(STATIC, "app.js"), "utf8");
const indexHtml = fs.readFileSync(path.join(STATIC, "index.html"), "utf8");
const css = fs.readFileSync(path.join(STATIC, "style.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

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
// Every rule as { selector, decls }, @media bodies included.
const RULES = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({
  selector: m[1].trim().replace(/\s+/g, " "),
  decls: new Map(m[2].split(";").map(d => d.match(/^\s*([-\w]+)\s*:\s*([\s\S]+?)\s*$/)).filter(Boolean).map(d => [d[1], d[2]])),
}));
const rule = sel => { const r = RULES.filter(x => x.selector === sel); assert(r.length, "a rule for " + sel); return new Map(r.flatMap(x => [...x.decls])); };
const NAV = indexHtml.slice(indexHtml.indexOf("<nav id=\"sidebar\""), indexHtml.indexOf("</nav>", indexHtml.indexOf("<nav id=\"sidebar\"")));

// Small stand-ins for the DOM and localStorage.
function classes(...init){
  const s = new Set(init);
  return { toggle(c, on){ if (on === undefined) on = !s.has(c); if (on) s.add(c); else s.delete(c); return on; },
    contains: c => s.has(c), add: c => s.add(c), remove: c => s.delete(c) };
}
function node(extra){
  const attrs = {};
  return Object.assign({ classList: classes(), dataset: {}, title: "", hidden: false,
    setAttribute(k, v){ attrs[k] = String(v); }, getAttribute(k){ return k in attrs ? attrs[k] : null; },
    removeAttribute(k){ delete attrs[k]; } }, extra);
}
function storage(init){
  const m = new Map(Object.entries(init || {}));
  return { getItem: k => m.has(k) ? m.get(k) : null, setItem: (k, v) => m.set(k, String(v)), removeItem: k => m.delete(k) };
}
function sidebarDom(drawer){
  const focused = [];
  const focusable = name => node({ name, focus(){ focused.push(name); } });
  const sections = ["workspace", "data", "mfg", "actions", "admin"].map(g => {
    const head = focusable("head:" + g);
    return node({ dataset: { group: g }, head, querySelector: sel => sel === ".sb-head" ? head : null });
  });
  const links = ["Dashboard", "Projects", "Test Admin"].map(t => node({ querySelector: () => ({ textContent: " " + t + " " }) }));
  const sidebar = node({ querySelectorAll: () => links, querySelector: () => sections[0].head });
  const ids = { sidebar, menubtn: focusable("menubtn"), "sb-backdrop": node({ hidden: true }) };
  const root = { getElementById: id => ids[id], querySelectorAll: () => sections, addEventListener(){} };
  const globals = { window: { matchMedia: () => ({ matches: drawer }), addEventListener(){} } };
  return { root, sections, links, ids, focused, globals };
}
const NAV_FNS = ["navFoldedGroups", "setNavGroupFolded", "wireNavGroups", "navIsDrawer", "syncMenuBtn", "setNavRail", "setNavDrawer", "toggleNav"];

test("the sidebar has five folding groups in today's order, each a header button that controls its links", () => {
  const groups = [...NAV.matchAll(/<div class="sb-section" data-group="([^"]+)">\s*<button type="button" class="sb-head" aria-expanded="true" aria-controls="([^"]+)">[\s\S]*?<span class="sb-tx">([^<]+)<\/span>[\s\S]*?<\/button>\s*<div class="sb-items" id="([^"]+)">/g)]
    .map(m => { assert.equal(m[2], m[4], "aria-controls names the group's links"); return [m[1], m[3]]; });
  assert.deepEqual(groups, [["workspace", "Workspace"], ["data", "Data"], ["mfg", "Manufacturing"], ["actions", "Actions"], ["admin", "Admin"]]);
  const navs = [...NAV.matchAll(/<a class="sb-link[^"]*" href="#\/([^"]+)" data-nav="([^"]+)"/g)].map(m => { assert.equal(m[1], m[2]); return m[2]; });
  assert.deepEqual(navs, ["dashboard", "projects", "items", "search", "mfg", "machines", "tools", "post-processors",
    "sheet-metal", "laser-nests", "approvals", "activity", "jobs", "system", "admin", "settings"]);
  assert.match(NAV, /<a class="sb-link sb-admin" href="#\/admin" data-nav="admin" hidden>/, "Administration stays for admins only");
  assert.match(NAV, /id="nb-approvals"/, "the approvals count stays");
  assert.deepEqual(rule(".sb-link[hidden]").get("display"), "none", "the hidden attribute wins over the link's flex layout");
});

test("group labels are sentence case: no uppercase or letter-spaced text in the sidebar", () => {
  assert.doesNotMatch(NAV, /sb-label/);
  const sidebarRules = RULES.filter(r => /\.sb-|#sidebar|\.menubtn/.test(r.selector));
  assert.ok(sidebarRules.length > 10);
  for (const r of sidebarRules){
    assert.notEqual(r.decls.get("text-transform"), "uppercase", r.selector);
    if (r.selector !== ".sb-brand") assert.equal(r.decls.get("letter-spacing"), undefined, r.selector);   // the product name is a wordmark, not a label
  }
});

test("a group's fold state is kept in this browser and comes back with aria-expanded", () => {
  const store = storage();
  const a = sidebarDom(false), ctx = loadFunctions(NAV_FNS, a.globals);
  ctx.wireNavGroups(a.root, store);
  assert.ok(a.sections.every(s => s.head.getAttribute("aria-expanded") === "true" && !s.classList.contains("folded")));
  a.sections[2].head.onclick();
  a.sections[4].head.onclick();
  assert.equal(a.sections[2].head.getAttribute("aria-expanded"), "false");
  assert.ok(a.sections[2].classList.contains("folded"));
  assert.deepEqual(JSON.parse(store.getItem("pdm_nav_folded")), ["mfg", "admin"]);
  a.sections[4].head.onclick();
  assert.deepEqual(JSON.parse(store.getItem("pdm_nav_folded")), ["mfg"]);
  // A new page load reads the same storage.
  const b = sidebarDom(false);
  ctx.wireNavGroups(b.root, store);
  assert.deepEqual(b.sections.map(s => s.head.getAttribute("aria-expanded")), ["true", "true", "false", "true", "true"]);
  // Damaged or missing storage folds nothing and does not throw.
  const c = sidebarDom(false);
  ctx.wireNavGroups(c.root, storage({ pdm_nav_folded: "{not json" }));
  assert.ok(c.sections.every(s => s.head.getAttribute("aria-expanded") === "true"));
  const blocked = { getItem(){ throw new Error("blocked"); }, setItem(){ throw new Error("blocked"); } };
  ctx.wireNavGroups(c.root, blocked);
  c.sections[0].head.onclick();
  assert.equal(c.sections[0].head.getAttribute("aria-expanded"), "false");
});

test("on wide screens the menu button folds the sidebar to an icon rail, with tooltips, kept in this browser", () => {
  const store = storage(), d = sidebarDom(false), ctx = loadFunctions(NAV_FNS, d.globals);
  ctx.toggleNav(d.root, store);
  assert.ok(d.ids.sidebar.classList.contains("rail"));
  assert.equal(store.getItem("pdm_nav_rail"), "1");
  assert.deepEqual(d.links.map(a => a.title), ["Dashboard", "Projects", "Test Admin"], "each label becomes its tooltip");
  assert.equal(d.ids.menubtn.getAttribute("aria-expanded"), "false");
  assert.equal(d.ids.menubtn.title, "Show the full sidebar");
  ctx.toggleNav(d.root, store);
  assert.ok(!d.ids.sidebar.classList.contains("rail"));
  assert.equal(store.getItem("pdm_nav_rail"), null);
  assert.deepEqual(d.links.map(a => a.title), ["", "", ""]);
  assert.equal(d.ids.menubtn.getAttribute("aria-expanded"), "true");
  assert.equal(d.ids.menubtn.title, "Show only icons in the sidebar");
  // The rail hides labels from sight only, and a folded group opens in the rail.
  assert.equal(rule("#sidebar.rail .sb-tx").get("clip-path"), "inset(50%)");
  assert.equal(rule("#sidebar.rail .sb-section .sb-link:not([hidden])").get("display"), "flex");
  assert.match(app, /wireSidebar\(document, localStorage\);/);
});

test("on narrow screens the same button opens a drawer; Escape closes it and focus returns to the button", () => {
  const store = storage(), d = sidebarDom(true), ctx = loadFunctions(NAV_FNS, d.globals);
  ctx.toggleNav(d.root, store);
  assert.ok(d.ids.sidebar.classList.contains("open"));
  assert.ok(!d.ids.sidebar.classList.contains("rail"), "the drawer never turns on the rail");
  assert.equal(store.getItem("pdm_nav_rail"), null);
  assert.equal(d.ids["sb-backdrop"].hidden, false);
  assert.equal(d.ids.menubtn.getAttribute("aria-expanded"), "true");
  assert.equal(d.ids.menubtn.title, "Close the menu");
  assert.deepEqual(d.focused, ["head:workspace"], "focus moves into the drawer");
  ctx.setNavDrawer(false, d.root, true);
  assert.ok(!d.ids.sidebar.classList.contains("open"));
  assert.equal(d.ids["sb-backdrop"].hidden, true);
  assert.deepEqual(d.focused, ["head:workspace", "menubtn"]);
  // The Escape handler leaves an open dialog or menu to close first.
  const wire = app.slice(app.indexOf("function wireSidebar("), app.indexOf("const DETAIL ="));
  assert.match(wire, /e\.key !== "Escape"/);
  assert.match(wire, /\.modal-backdrop, dialog\[open\], \.ctxmenu:not\(\.hidden\), \.kebabmenu:not\(\.hidden\)/);
  // A closed drawer is out of the Tab order.
  const narrow = css.slice(css.indexOf("@media (max-width: 900px)"));
  assert.match(narrow, /#sidebar \{[^}]*visibility:hidden/);
  assert.match(narrow, /#sidebar\.open \{[^}]*visibility:visible/);
});

test("the current page link gets aria-current, and it is accent text, bold and underlined with no fill", () => {
  const links = ["dashboard", "approvals", "settings"].map(nav => node({ dataset: { nav } }));
  const crumb = { textContent: "", title: "" };
  const ctx = loadFunctions(["navTarget", "syncNav"], {
    ROUTE: { name: "approvals" }, PMAP: {}, DETAIL: {}, TOOL: {}, itemName: () => "",
    $: () => crumb, document: { querySelectorAll: () => links },
  });
  ctx.syncNav();
  assert.deepEqual(links.map(a => a.getAttribute("aria-current")), [null, "page", null]);
  assert.deepEqual(links.map(a => a.classList.contains("on")), [false, true, false]);
  ctx.ROUTE = { name: "settings" };
  ctx.syncNav();
  assert.deepEqual(links.map(a => a.getAttribute("aria-current")), [null, null, "page"]);

  const on = rule(".sb-link.on");
  assert.equal(on.get("color"), "var(--acc)");
  assert.equal(on.get("font-weight"), "700");
  assert.equal(on.get("background"), undefined, "no filled block");
  assert.match([...RULES].find(r => r.selector.includes(".sb-link.on .sb-tx")).decls.get("text-decoration"), /underline/);
  assert.equal(rule(".sb-link").get("background"), undefined);
  // Hover and keyboard focus underline the label; neither paints a fill.
  const under = RULES.find(r => r.selector.includes(".sb-link:hover .sb-tx")).selector;
  assert.match(under, /\.sb-link:focus-visible \.sb-tx/);
  assert.ok(!RULES.some(r => /\.sb-link:hover$/.test(r.selector) && r.decls.has("background")));
});

test("accent text sits on the sidebar's own surface, a pair the theme and branding checks cover", () => {
  assert.equal(rule("#sidebar").get("background"), "var(--panel)");
  assert.equal(rule(".sb-head").get("color"), "var(--acc)");
  // Each theme's --acc on --panel is held to 4.5:1 by web-ui-tokens.test.cjs; an organization's
  // colors are checked against the same pair as they are chosen.
  const pairs = loadFunctions([], {});
  vm.runInContext(app.slice(app.indexOf("const BRAND_PAIRS ="), app.indexOf("];", app.indexOf("const BRAND_PAIRS =")) + 2).replace("const BRAND_PAIRS", "var BRAND_PAIRS"), pairs);
  assert.ok(pairs.BRAND_PAIRS.some(([fg, bg, min]) => fg === "acc" && bg === "panel" && min === 4.5));
  const tokens = fs.readFileSync(path.join(__dirname, "web-ui-tokens.test.cjs"), "utf8");
  assert.match(tokens, /TEXT_ON_SURFACES = \[[^\]]*"--acc"/);
  assert.match(tokens, /SURFACES = \["--bg", "--panel"/);
});

test("rows are about 32px (44px for touch), spacing comes from the scale, and the fold motion is transform only", () => {
  const row = rule(".sb-head, .sb-link");
  assert.equal(row.get("min-height"), "var(--sp-6)");
  assert.equal(rule(".sb-head, .sb-link, .sb-userbtn").get("min-height"), "44px");
  assert.match(css, /@media \(pointer: coarse\) \{ \.sb-head, \.sb-link, \.sb-userbtn/);
  assert.equal(rule(".sb-chev").get("transition"), "transform .15s ease");
  assert.equal(rule(".sb-section.folded .sb-head .sb-chev").get("transform"), "rotate(90deg)");
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\s*\*, \*::before, \*::after \{ animation:none !important; transition:none !important; \}/);
  // No colour literals in the sidebar rules.
  for (const r of RULES.filter(x => /\.sb-|#sidebar|\.menubtn/.test(x.selector)))
    for (const v of r.decls.values()) assert.doesNotMatch(v, /#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i, r.selector);
});

test("the account button opens the account menu; a token session gets Sign out", () => {
  const shown = [];
  const menu = node({ _trigger: null });
  menu.classList.add("hidden");
  const ctx = loadFunctions(["showUserMenu"], {
    $: id => (id === "kebabmenu" ? menu : node()), hideCtx(){}, hideKebab(){}, go(){}, signout(){}, changePassword(){}, setNavDrawer(){},
    showMenu: (m, entries, trigger) => shown.push({ m, labels: entries.map(e => e === "sep" ? "-" : e.label), trigger }),
    ME: { user: { id: "u1" } },
  });
  const trigger = node();
  ctx.showUserMenu({ stopPropagation(){}, currentTarget: trigger });
  assert.deepEqual([...shown[0].labels], ["Change password", "Change appearance", "-", "Sign out"]);
  assert.equal(shown[0].trigger, trigger, "Escape returns focus to the account button");
  ctx.ME = { user: null };
  ctx.showUserMenu({ stopPropagation(){}, currentTarget: trigger });
  assert.deepEqual([...shown[1].labels], ["Sign out"]);
  assert.match(NAV, /<button type="button" id="sbuserbtn" class="sb-userbtn" aria-haspopup="menu"><span id="sb-user" class="sb-user"><\/span>/);
  assert.match(app, /\$\("sbuserbtn"\)\.onclick = showUserMenu;/);
  // The top bar's account chip and button show only where the sidebar is a drawer.
  const wide = css.slice(css.indexOf("@media (min-width: 901px)"));
  assert.match(wide, /^@media \(min-width: 901px\) \{[\s\S]*?\.userchip, \.userchip-kebab \{ display:none; \}/);
});
