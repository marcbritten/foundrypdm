// Type scale and Text size setting checks for the web portal.
// Run with: node --test crates/pdm-api/tests/web-ui-type-scale.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const STATIC = path.join(__dirname, "../static");
const css = fs.readFileSync(path.join(STATIC, "style.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const app = fs.readFileSync(path.join(STATIC, "app.js"), "utf8");
const indexHtml = fs.readFileSync(path.join(STATIC, "index.html"), "utf8");

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
const esc = s => String(s).replace(/[&<>"']/g, c => ({ "&":"&amp;", "<":"&lt;", ">":"&gt;", "\"":"&quot;", "'":"&#39;" })[c]);
// Flat list of { selector, decls } for every rule, @media bodies included.
const RULES = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({
  selector: m[1].trim().replace(/\s+/g, " "),
  decls: new Map(m[2].split(";").map(d => d.match(/^\s*([-\w]+)\s*:\s*([\s\S]+?)\s*$/)).filter(Boolean).map(d => [d[1], d[2]])),
}));
const rule = sel => { const r = RULES.filter(x => x.selector === sel); assert(r.length, "a rule for " + sel); return new Map(r.flatMap(x => [...x.decls])); };

test("the scale is rem steps from 11 to 24px, and the root size follows the Text size choice", () => {
  const root = rule(":root");
  assert.deepEqual(["xs", "sm", "base", "lg", "xl", "2xl"].map(s => root.get("--fs-" + s)),
    [".6875rem", ".75rem", ".875rem", "1rem", "1.25rem", "1.5rem"]);
  assert.equal(root.get("--icon-xl"), "2rem", "large decorative glyphs use an icon size, not a text step");
  const icons = RULES.filter(r => [...r.decls.values()].some(v => v.includes("var(--icon-xl)"))).map(r => r.selector).sort();
  assert.deepEqual(icons, [".emptyState .ic", ".pv-empty .big", ".soon .ic"], "only the decorative glyphs");
  assert.equal(root.get("--fs-scale"), "1");
  assert.equal(rule("html").get("font-size"), "calc(100% * var(--fs-scale))", "100% keeps the browser's own text size");
  assert.equal(rule("html[data-text-size=\"112\"]").get("--fs-scale"), "1.125");
  assert.equal(rule("html[data-text-size=\"125\"]").get("--fs-scale"), "1.25");
});

test("11px is used only for text that repeats something shown elsewhere (Y4)", () => {
  const xs = RULES.filter(r => [...r.decls.values()].some(v => v.includes("var(--fs-xs)"))).map(r => r.selector).sort();
  assert.deepEqual(xs, [".avatar", ".ctxmenu .ci .k", ".kebabmenu .ki .k", ".sb-count", ".tab .count", ".tree .cnt"]);
  assert.doesNotMatch(app, /var\(--fs-xs\)/, "no inline 11px text");
});

test("compact density changes padding only, never font size (L3)", () => {
  const dense = RULES.filter(r => /\.dense\b/.test(r.selector));
  assert(dense.length > 0);
  for (const r of dense) for (const p of r.decls.keys())
    assert(p !== "font-size" && p !== "font" && !p.startsWith("--fs-"), r.selector + " sets " + p);
});

test("table cells are body text, also when they show codes or muted text (Y4)", () => {
  assert.equal(rule("th, td").get("font-size"), "var(--fs-base)");
  assert.equal(rule("td.mono, td.dim").get("font-size"), "var(--fs-base)", "the class rules for .mono and .dim would make them 12px");
});

test("numbers compared down a column use tabular, right-aligned digits (Y6)", () => {
  const num = rule(".num");
  assert.equal(num.get("font-variant-numeric"), "tabular-nums");
  assert.equal(num.get("text-align"), "right");
  assert.match(app, /<th class='num'>Size<\/th><th>Added<\/th>/);
  assert.match(app, /"<td class='muted num'>" \+ bytes\(doc\.size_bytes\)/);
  assert.match(app, /<th class='num'>Size<\/th><th>SHA-256<\/th>/);
  assert.match(app, /"<td class='muted num'>" \+ bytes\(vv\.size_bytes\)/);
});

// ── Text size, applied before first paint ───────────────────────────────
function runHead(stored){
  const attrs = {};
  const document = { documentElement:{ setAttribute(k, v){ attrs[k] = v; }, getAttribute(k){ return attrs[k]; } },
    createElement: () => ({}), head:{ appendChild(){} } };
  const localStorage = { getItem: k => (k in stored ? stored[k] : null) };
  const head = indexHtml.slice(0, indexHtml.indexOf("</head>"));
  const script = [...head.matchAll(/<script>([\s\S]*?)<\/script>/g)].map(m => m[1]).filter(s => s.includes("pdm_text_size"));
  assert.equal(script.length, 1, "one inline <head> script reads the text size");
  const at = script[0].lastIndexOf("} catch (e) {}");
  vm.runInContext(script[0].slice(0, at) + "} catch (e) { throw e; }" + script[0].slice(at + 14), vm.createContext({ localStorage, document }));
  return attrs["data-text-size"];
}
function sizeCtx(stored = {}){
  const attrs = {};
  const ctx = loadFunctions(["textSizeOptions", "currentTextSize", "applyTextSize", "textSizeHTML"], {
    esc,
    localStorage:{ getItem:k => (k in stored ? stored[k] : null), setItem(k, v){ stored[k] = String(v); }, removeItem(k){ delete stored[k]; } },
    document:{ documentElement:{ setAttribute(k, v){ attrs[k] = v; }, removeAttribute(k){ delete attrs[k]; } } },
  });
  return { ctx, attrs, stored };
}

test("index.html applies the saved text size before first paint, with the same ids and default as app.js", () => {
  const ids = [...sizeCtx().ctx.textSizeOptions().map(o => o.id)];
  assert.deepEqual(ids, ["100", "112", "125"]);
  for (const id of [...ids, "150", "", "112.5", null])
    assert.equal(runHead(id === null ? {} : { pdm_text_size:id }), ["112", "125"].includes(id) ? id : undefined, "pdm_text_size=" + id);
  for (const id of ids.filter(i => i !== "100")) assert(RULES.some(r => r.selector === "html[data-text-size=\"" + id + "\"]"), "a rule for " + id);
});

test("choosing a text size applies it at once and remembers it; Default removes it", () => {
  const { ctx, attrs, stored } = sizeCtx();
  assert.equal(ctx.currentTextSize(), "100");
  ctx.applyTextSize("125");
  assert.deepEqual([attrs["data-text-size"], stored.pdm_text_size], ["125", "125"]);
  ctx.applyTextSize("100");
  assert.deepEqual([attrs["data-text-size"], stored.pdm_text_size], [undefined, undefined]);
  ctx.applyTextSize("150");
  assert.deepEqual(attrs, {}, "an unknown size falls back to the default");
  assert.equal(sizeCtx({ pdm_text_size:"nope" }).ctx.currentTextSize(), "100");
});

test("the Text size setting is labeled, says what it changes and its default, and sits with the font settings", () => {
  const html = sizeCtx({ pdm_text_size:"112" }).ctx.textSizeHTML();
  assert.match(html, /<label for='text-size'>Text size<\/label><select id='text-size' aria-describedby='text-size-hint'>/);
  assert.match(html, /<option value='112' selected>Larger \(112\.5%\)<\/option>/);
  assert.match(html, /<p class='dim' id='text-size-hint'>Sets the size of all text in the workspace, and the default of 100% follows/);
  assert.match(app, /\}\)\.join\(""\) \+ textSizeHTML\(\) \+/, "rendered by fontPickerHTML, next to the font settings");
  assert.match(app, /if \(size\) size\.onchange = \(\) => applyTextSize\(size\.value\);/, "wired by wireFontPicker");
});
