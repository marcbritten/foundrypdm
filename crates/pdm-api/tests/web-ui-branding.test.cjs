// Organization branding in the web portal: contrast checks, the accent helper, the CSS mirror of
// the server's, the name/logo swap, and the allow-list kept in step with style.css.
// Run with: node --test crates/pdm-api/tests/web-ui-branding.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const STATIC = path.join(__dirname, "../static");
const app = fs.readFileSync(path.join(STATIC, "app.js"), "utf8");
const css = fs.readFileSync(path.join(STATIC, "style.css"), "utf8");
const indexHtml = fs.readFileSync(path.join(STATIC, "index.html"), "utf8");
const coreBranding = fs.readFileSync(path.join(__dirname, "../../pdm-core/src/branding.rs"), "utf8");

function loadFunctions(names, globals){
  const context = vm.createContext(globals || {});
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
// A top-level `const NAME = …;` from app.js, evaluated on its own.
function loadConst(name){
  const start = app.indexOf("const " + name + " = ");
  assert(start >= 0, "missing const " + name);
  const end = app.indexOf(";\n", start) >= 0 ? app.indexOf(";\n", start) : app.indexOf(";\r\n", start);
  return vm.runInNewContext("(" + app.slice(start + ("const " + name + " = ").length, end) + ")");
}

const F = loadFunctions(["brandHex", "contrastRatio", "brandingWarnings", "deriveAccent", "brandCss",
  "brandColorPatch", "applyBrandIdentity", "applyBrandColors", "brandOff", "logoCanvasSize", "brandPreviewCss"]);
const PAIRS = loadConst("BRAND_PAIRS");
const LABELS = loadConst("BRAND_TOKEN_LABELS");
const EDITOR_THEMES = loadConst("BRAND_THEMES");

// The server's allow-list, read from the Rust source so the two cannot drift.
const SERVER_TOKENS = (() => {
  const m = coreBranding.match(/pub const TOKENS: &\[&str\] = &\[([\s\S]*?)\];/);
  assert(m, "TOKENS in crates/pdm-core/src/branding.rs");
  return [...m[1].matchAll(/"([^"]+)"/g)].map(x => x[1]);
})();

// Each theme block's custom properties, with var() references to the same block resolved.
const THEME_SELECTORS = { dark: ':root, [data-theme="dark"]', light: '[data-theme="light"]',
  slate: '[data-theme="slate"]', contrast: '[data-theme="contrast"]' };
function themeBlock(theme, overrides){
  const sel = THEME_SELECTORS[theme];
  const start = css.indexOf(sel + " {");
  assert(start >= 0, "theme block " + theme);
  const body = css.slice(css.indexOf("{", start) + 1, css.indexOf("}", start));
  const raw = {};
  for (const part of body.split(";")){
    const m = part.match(/^\s*(--[\w-]+)\s*:\s*([\s\S]+?)\s*$/);
    if (m) raw[m[1].slice(2)] = m[2];
  }
  Object.assign(raw, overrides || {});
  const resolve = (v, depth) => depth > 5 ? v : v.replace(/var\(--([\w-]+)\)/g, (_, k) => resolve(raw[k] || "", depth + 1));
  const out = {};
  for (const [k, v] of Object.entries(raw)) out[k] = resolve(v, 0);
  return out;
}
const DEFAULTS = Object.fromEntries(Object.keys(THEME_SELECTORS).map(t => [t, themeBlock(t)]));
// Objects made in the vm context have that context's prototypes; compare them as plain data.
const plain = x => JSON.parse(JSON.stringify(x));
const same = (actual, expected, msg) => assert.deepEqual(plain(actual), expected, msg);

test("contrastRatio follows WCAG 2.x", () => {
  assert.equal(F.contrastRatio("#ffffff", "#000000"), 21);
  assert.equal(F.contrastRatio("#000", "#fff"), 21);
  assert.equal(Math.round(F.contrastRatio("#ffffff", "#5aa9ff") * 100) / 100, 2.46);
  assert.equal(F.contrastRatio("#5aa9ff", "#5aa9ff"), 1);
  assert.equal(F.contrastRatio("rgb(255, 255, 255)", "#000000"), 21);
  assert.equal(F.contrastRatio("rgba(5,8,12,.6)", "#000000"), null, "a color with alpha is not judged");
  assert.equal(F.contrastRatio("red", "#000000"), null);
});

test("brandingWarnings names a failing pair and its ratio, and only that", () => {
  const dark = { ...DEFAULTS.dark };
  same(F.brandingWarnings(dark, PAIRS), [], "the dark defaults pass every pair");
  const white = F.brandingWarnings({ ...dark, "on-acc": "#ffffff" }, PAIRS);
  assert.equal(white.length, 1);
  assert.equal(white[0].fg, "on-acc");
  assert.equal(white[0].bg, "acc");
  assert.equal(white[0].min, 4.5);
  assert(white[0].ratio < 4.5 && white[0].ratio > 2.4);
  same(F.brandingWarnings({ ...dark, "on-acc": "#000000" }, PAIRS), []);
  // The focus ring and field edges are held to 3:1, text to 4.5:1.
  const mins = Object.fromEntries(PAIRS.map(p => [p[0] + "/" + p[1], p[2]]));
  assert.equal(mins["acc/bg"], 3);
  assert.equal(mins["field-line/panel"], 3);
  assert.equal(mins["fg/bg"], 4.5);
});

test("every theme's defaults pass the checked pairs, so a warning always means an override", () => {
  for (const [theme, colors] of Object.entries(DEFAULTS))
    same(F.brandingWarnings(colors, PAIRS).map(w => w.label), [], theme);
});

test("deriveAccent reads at 4.5:1 on each theme's panel and leaves high contrast alone unless asked", () => {
  for (const brand of ["#3b82f6", "#ff6600", "#1a1a1a", "#fafafa", "#7f7f7f"]){
    const out = F.deriveAccent(brand, DEFAULTS, false);
    same(Object.keys(out).sort(), ["dark", "light", "slate"], brand);
    for (const [theme, set] of Object.entries(out)){
      assert(F.contrastRatio(set.acc, DEFAULTS[theme].panel) >= 4.5, brand + " acc on " + theme);
      assert(F.contrastRatio(set["on-acc"], set.acc) >= 4.5, brand + " on-acc on " + theme);
      assert(["#000000", "#ffffff"].includes(set["on-acc"]));
    }
    const all = F.deriveAccent(brand, DEFAULTS, true);
    assert(all.contrast && F.contrastRatio(all.contrast.acc, DEFAULTS.contrast.panel) >= 4.5, brand);
  }
  // Applied to each theme, it causes no contrast warning at all (tokens that refer to the
  // accent, such as the in-review lifecycle color, follow it).
  for (const brand of ["#3b82f6", "#ff6600", "#e55e36", "#7f7f7f"]){
    const out = F.deriveAccent(brand, DEFAULTS, true);
    for (const [theme, set] of Object.entries(out)){
      same(F.brandingWarnings(themeBlock(theme, set), PAIRS), [], brand + " on " + theme);
      for (const surface of ["panel", "seg-on-bg", "active"])
        assert(F.contrastRatio(set.acc, DEFAULTS[theme][surface]) >= 4.5, brand + " acc on " + theme + " " + surface);
    }
  }
  // A color that already passes is kept as it is.
  assert.equal(F.deriveAccent("#195ebd", DEFAULTS, false).light.acc, "#195ebd");
  same(F.deriveAccent("not a color", DEFAULTS, true), {});
});

test("brandCss writes what the server writes, and nothing outside the allow-list", () => {
  const tokens = ["acc", "on-acc", "fg"];
  assert.equal(
    F.brandCss({ light: { acc: "#1D4ED8" }, dark: { "on-acc": "#0b1220", acc: "#3b82f6" } }, tokens),
    "html[data-theme=\"dark\"]{--acc:#3b82f6;--on-acc:#0b1220}\nhtml[data-theme=\"light\"]{--acc:#1d4ed8}\n");
  const out = F.brandCss({
    dark: { acc: "#3b82f6", shadow: "#000000", fg: "red;}body{display:none", "on-acc": "url(x)" },
    "evil\"]{x:y}": { acc: "#000000" },
    light: { fg: "#fff" },
    contrast: { fg: "#ffffff" }
  }, tokens.concat(["x;}y{"]));
  assert.equal(out, "html[data-theme=\"contrast\"]{--fg:#ffffff}\nhtml[data-theme=\"dark\"]{--acc:#3b82f6}\n");
  assert.equal(F.brandCss({}, tokens), "");
  assert.equal(F.brandCss(null, tokens), "");
});

test("brandColorPatch sends changes, removals as null and emptied themes as null", () => {
  const saved = { dark: { acc: "#111111", fg: "#eeeeee" }, light: { acc: "#222222" } };
  const pending = { dark: { acc: "#333333" }, slate: { acc: "#444444" } };
  same(F.brandColorPatch(saved, pending), {
    dark: { acc: "#333333", fg: null }, light: null, slate: { acc: "#444444" } });
  same(F.brandColorPatch(saved, JSON.parse(JSON.stringify(saved))), {});
  same(F.brandColorPatch({}, {}), {});
});

// ── applyBrandIdentity against a small fake document ────────────────────
class El {
  constructor(tag){
    this.tagName = tag; this._text = ""; this.children = []; this.hidden = false;
    this.attrs = {}; this.dataset = {}; this.className = "";
    const set = new Set();
    this.classList = { toggle: (c, on) => { if (on) set.add(c); else set.delete(c); }, contains: c => set.has(c) };
  }
  get textContent(){ return this._text + this.children.map(c => c.textContent || "").join(""); }
  set textContent(v){ for (const c of this.children) c.parentNode = null; this._text = String(v); this.children = []; }
  appendChild(c){ c.parentNode = this; this.children.push(c); return c; }
  setAttribute(k, v){ this.attrs[k] = String(v); }
  getAttribute(k){ return k in this.attrs ? this.attrs[k] : null; }
  remove(){ this.removed = true; }
}
function fakeDoc(){
  const marks = [new El("span"), new El("span")], names = [new El("span"), new El("span")];
  for (const m of marks) m.textContent = "⬢";
  for (const n of names) n.textContent = "Foundry PDM";
  const fav = new El("link"); fav.setAttribute("href", "data:product-mark");
  const link = new El("link"), head = new El("head");
  const byId = { favicon: fav, "brand-css": link };
  return {
    marks, names, fav, link, head, title: "Foundry PDM — Workspace",
    querySelectorAll: sel => sel === "[data-brand-name]" ? names : sel === "[data-brand-mark]" ? marks : [],
    createElement: tag => new El(tag),
    getElementById: id => byId[id] || head.children.find(c => c.id === id) || null,
  };
}

test("applyBrandIdentity sets the name as text, swaps the mark for the logo and sets the title", () => {
  const doc = fakeDoc();
  F.applyBrandIdentity({ name: "<b>Example</b> Works", showName: true, logoSrc: null }, doc);
  for (const n of doc.names){ assert.equal(n.textContent, "<b>Example</b> Works"); assert.equal(n.children.length, 0); assert.equal(n.hidden, false); }
  for (const m of doc.marks) assert.equal(m.textContent, "⬢");
  assert.equal(doc.title, "<b>Example</b> Works — Workspace");

  F.applyBrandIdentity({ name: "Example Works", showName: true, logoSrc: "/logo?v=1" }, doc);
  for (const m of doc.marks){
    assert.equal(m.children.length, 1);
    const img = m.children[0];
    assert.equal(img.tagName, "img");
    assert.equal(img.src, "/logo?v=1");
    assert.equal(img.alt, "", "the name is shown next to it, so the logo is decorative");
    assert(m.classList.contains("has-logo"));
  }
  for (const n of doc.names) assert.equal(n.hidden, false);

  F.applyBrandIdentity({ name: "Example Works", showName: false, logoSrc: "/logo?v=1" }, doc);
  for (const n of doc.names) assert.equal(n.hidden, true);
  for (const m of doc.marks) assert.equal(m.children[0].alt, "Example Works", "the logo alone carries the name");

  // A logo that fails to load falls back to the mark and the name.
  doc.marks[0].children[0].onerror();
  assert.equal(doc.marks[0].textContent, "⬢");
  assert(!doc.marks[0].classList.contains("has-logo"));
  for (const n of doc.names) assert.equal(n.hidden, false);

  // Without a logo the name always shows, whatever the flag says.
  F.applyBrandIdentity({ name: "Example Works", showName: false, logoSrc: null }, doc);
  for (const n of doc.names) assert.equal(n.hidden, false);

  F.applyBrandIdentity(null, doc);
  assert.equal(doc.title, "Foundry PDM — Workspace");
  for (const n of doc.names) assert.equal(n.textContent, "Foundry PDM");
});

test("a replaced logo that fails late does not undo the new one", () => {
  const doc = fakeDoc();
  F.applyBrandIdentity({ name: "Example", showName: false, logoSrc: "/public/logo?v=1" }, doc);
  const old = doc.marks[0].children[0];
  F.applyBrandIdentity({ name: "Example", showName: false, logoSrc: "blob:org-logo" }, doc);
  old.onerror();
  assert.equal(doc.marks[0].children[0].src, "blob:org-logo");
  assert(doc.marks[0].classList.contains("has-logo"));
  for (const n of doc.names) assert.equal(n.hidden, true);
});

test("the color preview reaches the page only while the Branding card is on it", () => {
  const saved = { dark: { acc: "#111111", "primary-fg": "#222222" } };
  const pending = { dark: { acc: "#333333" } };
  // primary-fg falls back to var(--fg): what the probe resolves with the pending colors.
  const eff = { dark: { acc: "#333333", "primary-fg": "#d8e1ec" } };
  const tokens = ["acc", "primary-fg"];
  assert.equal(F.brandPreviewCss(saved, pending, eff, tokens, true),
    ".brand-sample[data-theme=\"dark\"], .brand-tile[data-theme=\"dark\"]{--acc:#333333;--primary-fg:#d8e1ec}\n" +
    "html[data-theme=\"dark\"]:has(#brand-card){--acc:#333333;--primary-fg:#d8e1ec}\n");
  assert.doesNotMatch(F.brandPreviewCss(saved, pending, eff, tokens, false), /html/, "?nobrand: tiles only");
  assert.equal(F.brandPreviewCss({}, {}, {}, tokens, true), "");
  // Every theme gets its page rule, so switching themes from the top bar keeps the preview.
  const all = F.brandPreviewCss({}, { light: { acc: "#444444" }, contrast: { acc: "#555555" } }, {}, tokens, true);
  assert.match(all, /html\[data-theme="light"\]:has\(#brand-card\)\{--acc:#444444\}/);
  assert.match(all, /html\[data-theme="contrast"\]:has\(#brand-card\)\{--acc:#555555\}/);
});

test("the tab icon changes only when the logo is chosen as the icon, and comes back", () => {
  const doc = fakeDoc();
  F.applyBrandIdentity({ name: "Example", showName: true, logoSrc: "/logo?v=1", faviconSrc: null }, doc);
  assert.equal(doc.fav.getAttribute("href"), "data:product-mark");
  F.applyBrandIdentity({ name: "Example", showName: true, logoSrc: "/logo?v=1", faviconSrc: "/logo?v=1" }, doc);
  assert.equal(doc.fav.getAttribute("href"), "/logo?v=1");
  F.applyBrandIdentity({ name: "Example", showName: true, logoSrc: "/logo?v=1", faviconSrc: null }, doc);
  assert.equal(doc.fav.getAttribute("href"), "data:product-mark");
});

test("applyBrandColors replaces the sign-in organization's CSS, and ?nobrand turns colors off", () => {
  const doc = fakeDoc();
  F.applyBrandColors({ dark: { acc: "#3b82f6" } }, ["acc"], doc, "");
  assert.equal(doc.link.disabled, true);
  const style = doc.head.children.find(c => c.id === "org-brand");
  assert(style, "a <style id=org-brand> is added");
  assert.equal(style.textContent, "html[data-theme=\"dark\"]{--acc:#3b82f6}\n");
  F.applyBrandColors({ dark: { acc: "#3b82f6" } }, ["acc"], doc, "?nobrand");
  assert.equal(style.removed, true);
  assert.equal(F.brandOff("?nobrand"), true);
  assert.equal(F.brandOff("?x=1&nobrand=1"), true);
  assert.equal(F.brandOff("?nobranding"), false);
  assert.equal(F.brandOff(""), false);
});

test("logoCanvasSize draws about 160 px tall within the server's limits", () => {
  same(F.logoCanvasSize(480, 120), { w: 640, h: 160 });
  same(F.logoCanvasSize(0, 0), { w: 160, h: 160 });
  same(F.logoCanvasSize(10000, 100), { w: 2048, h: 20 });
  same(F.logoCanvasSize(10, 1000), { w: 16, h: 160 });
});

test("the editor's token list is the server's, with a label for each", () => {
  // The server's allow-list is style.css's per-theme colors, minus what is not a plain brand
  // color (shadow, backdrop), preview and drawing surfaces (paper, canvas, draw-*) and tokens
  // derived from others (text-1, text-3, surface*, focus-ring).
  const excluded = k => /^(shadow|backdrop|paper|canvas|draw-.*|text-1|text-3|surface.*|focus-ring)$/.test(k);
  const declared = Object.keys(DEFAULTS.dark).filter(k => !excluded(k)).sort();
  assert.deepEqual([...SERVER_TOKENS].sort(), declared,
    "update TOKENS in crates/pdm-core/src/branding.rs (or the exclusions here) when a theme color is added");
  for (const t of SERVER_TOKENS) assert(LABELS[t], "a label for " + t);
  for (const [fg, bg] of PAIRS){ assert(SERVER_TOKENS.includes(fg), fg); assert(SERVER_TOKENS.includes(bg), bg); }
  same(EDITOR_THEMES.map(t => t[0]), ["dark", "light", "slate", "contrast"], "every theme can be branded");
});

test("index.html loads the sign-in branding after style.css and marks the brand spots", () => {
  const head = indexHtml.slice(0, indexHtml.indexOf("</head>"));
  const style = head.indexOf("href=\"/static/style.css\"");
  const brand = head.indexOf("<link id=\"brand-css\" rel=\"stylesheet\" href=\"/public/branding.css\">");
  assert(style >= 0 && brand > style, "#brand-css follows style.css in <head>");
  assert.match(head.slice(brand), /nobrand[\s\S]*getElementById\("brand-css"\)\.disabled = true/,
    "?nobrand switches #brand-css off before the first paint");
  assert.match(head, /<link id="favicon" rel="icon"/);
  assert.match(indexHtml, /<div class="auth-logo"><span class="mark" data-brand-mark>⬢<\/span><span data-brand-name>Foundry PDM<\/span><\/div>/);
  assert.match(indexHtml, /<div class="sb-brand"><span class="mark" data-brand-mark>⬢<\/span> <span data-brand-name>Foundry PDM<\/span><\/div>/);
});

test("the Branding card is mounted for admins only, and branding loads before and after sign-in", () => {
  const settings = app.slice(app.indexOf("async function viewSettings("), app.indexOf("\nfunction prefixTable("));
  assert.match(settings, /const canEdit = isRole\("admin"\);/);
  assert.match(settings, /if \(canEdit\) mountBrandingCard\(v\);/);
  const enter = app.slice(app.indexOf("async function enterWorkspace("), app.indexOf("\n// ── routing"));
  assert.match(enter, /loadOrgBranding\(\);/);
  assert.match(app, /\$\("authbuild"\)[\s\S]{0,200}\n {2}loadPublicBranding\(\);/);
  // The product name stays on the sign-in build line when the card is branded (B5).
  assert.match(app, /\$\("authbuild"\)\.textContent = "Foundry PDM " \+ parts\.join/);
});

test("the branding code sets no inline colors and uses no browser dialogs", () => {
  const start = app.indexOf("// ── organization branding");
  const end = app.indexOf("\n// ── file kinds / badges");
  const block = app.slice(start, end);
  assert(start >= 0 && end > start);
  assert.doesNotMatch(block, /style=['"][^'"]*(#[0-9a-fA-F]{3,8}|rgba?\()/, "colors come from tokens (T1)");
  assert.doesNotMatch(block, /\b(window\.)?(confirm|alert|prompt)\(/, "no browser dialogs (D3)");
  // Server and user text reaches HTML only through esc(): attribute values and the display name.
  assert.doesNotMatch(block, /value='" \+ (?!esc\()/, "attribute values are escaped");
  assert.doesNotMatch(block, /\+\s*(?:b|view|st\.view)\.display_name\b/, "the display name is escaped or set as text");
  assert.doesNotMatch(block, /\+\s*e\.message\s*\+\s*"<\//, "error text in HTML is escaped");
});
