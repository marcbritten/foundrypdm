// Contrast of specific controls in their states (rest, hover, keyboard focus) and the theme
// picker swatches. web-ui-tokens.test.cjs checks token pairs; a pair of good tokens can still be
// combined wrongly by a state rule, so these checks tie each pair to the rule that draws it.
// Run with: node --test crates/pdm-api/tests/web-ui-a11y.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const STATIC = path.join(__dirname, "../static");
const css = fs.readFileSync(path.join(STATIC, "style.css"), "utf8");
const app = fs.readFileSync(path.join(STATIC, "app.js"), "utf8");

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

// ── style.css: rules, theme tokens, WCAG contrast ─────────────────────────
const THEMES = ["dark", "light", "slate", "contrast"];
const THEME_SELECTORS = { ':root, [data-theme="dark"]':"dark", '[data-theme="light"]':"light",
  '[data-theme="slate"]':"slate", '[data-theme="contrast"]':"contrast" };
function parseRules(src){
  const out = [];
  (function walk(text, selector){
    let i = 0, own = "";
    while (i < text.length){
      const open = text.indexOf("{", i);
      if (open < 0){ own += text.slice(i); break; }
      const chunk = text.slice(i, open), cut = chunk.lastIndexOf(";") + 1;
      own += chunk.slice(0, cut);
      const prelude = chunk.slice(cut).trim().replace(/\s+/g, " ");
      let depth = 1, j = open + 1;
      while (j < text.length && depth > 0){ if (text[j] === "{") depth++; else if (text[j] === "}") depth--; j++; }
      const body = text.slice(open + 1, j - 1);
      if (prelude.startsWith("@")){ if (!/^@keyframes\b/.test(prelude)) walk(body, null); }
      else walk(body, prelude);
      i = j;
    }
    if (selector !== null && own.trim()) out.push({ selector, body:own });
  })(src.replace(/\/\*[\s\S]*?\*\//g, ""), null);
  return out;
}
function declarations(body){
  const d = new Map();
  for (const part of body.split(";")){ const m = part.match(/^\s*([-\w]+)\s*:\s*([\s\S]+?)\s*$/); if (m) d.set(m[1], m[2]); }
  return d;
}
const RULES = parseRules(css);
const rule = selector => {
  const found = RULES.filter(r => r.selector === selector);
  assert(found.length > 0, "a rule for " + selector);
  return new Map(found.flatMap(r => [...declarations(r.body)]));
};
const TOKENS = new Map(THEMES.map(t => [t, new Map(RULES.filter(r => THEME_SELECTORS[r.selector] === t)
  .flatMap(r => [...declarations(r.body)].filter(([k]) => k.startsWith("--"))))]));
// The token a declaration uses: "var(--x)" or "var(--x, fallback)" gives "--x"; anything else gives null.
const tokenOf = value => { const m = (value || "").match(/^var\(\s*(--[\w-]+)\s*(?:,[^)]*)?\)$/); return m ? m[1] : null; };
function resolve(theme, name, seen = []){
  assert(!seen.includes(name), theme + ": token cycle " + seen.concat(name).join(" → "));
  const v = TOKENS.get(theme).get(name);
  assert(v !== undefined, theme + ": " + name + " is not defined");
  const ref = v.match(/^var\((--[\w-]+)\)$/);
  if (ref) return resolve(theme, ref[1], seen.concat(name));
  const hex = v.match(/^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/);
  assert(hex, theme + ": " + name + " = " + v + " is not a solid hex color");
  const h = hex[1].length === 3 ? hex[1].replace(/./g, c => c + c) : hex[1];
  return [0, 2, 4].map(i => parseInt(h.slice(i, i + 2), 16));
}
function luminance(rgb){
  const [r, g, b] = rgb.map(c => { c /= 255; return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(a, b){ const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); }

// The token a rule declares for a property; fails if the rule uses a literal or another form.
function declared(selector, prop){
  const token = tokenOf(rule(selector).get(prop));
  assert(token, selector + " { " + prop + " } uses a theme token (found " + rule(selector).get(prop) + ")");
  return token;
}
function failsBelow(min, label, fg, bg){
  const out = [];
  for (const t of THEMES){
    const r = contrast(resolve(t, fg), resolve(t, bg));
    if (r < min) out.push(t + ": " + label + " " + fg + " on " + bg + " = " + r.toFixed(2) + " (needs " + min + ")");
  }
  return out;
}

// ── Danger buttons ──────────────────────────────────────────────────────
test("danger buttons: red text on the normal button fill, at rest and on hover, meets 4.5:1", () => {
  // button.danger has no fill of its own: it keeps the button fill and the button hover fill.
  assert(!rule("button.danger").has("background") && !rule("button.danger").has("background-color"),
    "a filled danger button needs its own text color token on that fill (guideline T3); add it to all four themes and to this test");
  const text = declared("button.danger", "color");
  const fails = [
    ...failsBelow(4.5, "button.danger", text, declared("button", "background")),
    ...failsBelow(4.5, "button.danger:hover", text, declared("button:hover", "background")),
  ];
  assert.deepEqual(fails, []);
});

// Solid status fills only; tinted backgrounds (color-mix of a status color into a surface) carry
// ordinary text and are covered by the surface pairs.
test("no rule fills solid --err, --ok or --warn without a paired text color", () => {
  const fills = [];
  for (const r of RULES){
    const d = declarations(r.body);
    const fill = tokenOf(d.get("background")) || tokenOf(d.get("background-color"));
    if (!["--err", "--ok", "--warn"].includes(fill)) continue;
    const ink = tokenOf(d.get("color"));
    if (!ink || ink !== "--on-" + fill.slice(2)) fills.push(r.selector + " fills with " + fill + " but its text is " + (d.get("color") || "inherited"));
    else fills.push(...failsBelow(4.5, r.selector, ink, fill));
  }
  assert.deepEqual(fills, []);
});

// ── Selected laser-nest part row: the remove button ─────────────────────
test("remove button on a selected part row is readable at rest, on hover and on keyboard focus", () => {
  const row = declared(".nc-partrow.sel", "background");
  assert.equal(row, "--acc");
  const fails = [...failsBelow(4.5, "x at rest", declared(".nc-partrow.sel .nc-rmline, .nc-partrow.sel .nc-lineerr", "color"), row)];
  // .nc-rmline:hover gives every remove button a --hover fill and red text; on the accent row the
  // selected state must replace both, and the focus ring (normally --acc) must not match the row.
  const STATE = ".nc-partrow.sel .nc-rmline:hover, .nc-partrow.sel .nc-rmline:focus-visible";
  const text = declared(STATE, "color"), fill = declared(STATE, "background");
  fails.push(...failsBelow(4.5, "x on hover/focus", text, fill));
  fails.push(...failsBelow(3, "x hover fill against the row", fill, row));
  fails.push(...failsBelow(3, "x focus ring against the row", declared(STATE, "outline-color"), row));
  assert.deepEqual(fails, []);
  // The generic rules the state rule must override still look the way this test assumes.
  assert.equal(declared(".nc-rmline:hover", "background"), "--hover");
  assert.match(rule("button:focus-visible, a:focus-visible, input:focus-visible, select:focus-visible, " +
    "textarea:focus-visible, [tabindex]:focus-visible, .tab:focus-visible").get("outline") || "", /var\(--acc\)/);
});

// ── Laser-nest legend ───────────────────────────────────────────────────
test("laser-nest preview legend text meets 4.5:1 on its own box", () => {
  assert.deepEqual(failsBelow(4.5, ".nc-legend", declared(".nc-legend", "color"), declared(".nc-legend", "background")), []);
});

// ── Theme picker swatches come from the stylesheet ──────────────────────
function themeSource(){
  const src = app.match(/const THEME_SWATCHES = \[[^\]]*\];\r?\nconst THEMES = \[[\s\S]*?\n\];/);
  assert(src, "app.js declares THEME_SWATCHES right before THEMES");
  return src[0];
}
test("theme swatches are theme tokens drawn in each theme's own scope, not copied colors", () => {
  assert.doesNotMatch(themeSource(), /#[0-9a-fA-F]{3,8}\b|rgba?\(/, "THEMES holds no color values");
  const ctx = loadFunctions(["currentTheme", "themePickerHTML"], {
    localStorage:{ getItem:() => "dark" }, esc:s => String(s),
  });
  vm.runInContext(themeSource().replace(/^const /gm, "var "), ctx);
  assert(ctx.THEME_SWATCHES.length > 0, "the picker shows swatches");
  const html = ctx.themePickerHTML();
  const cards = [...html.matchAll(/data-theme-id='([\w-]+)'[^>]*>([\s\S]*?)<\/button>/g)];
  assert.deepEqual(cards.map(c => c[1]), THEMES, "one card per theme");
  for (const [, id, body] of cards){
    const spans = [...body.matchAll(/<span data-theme='([\w-]+)' style='background:var\((--[\w-]+)\)'><\/span>/g)];
    assert.equal(spans.length, ctx.THEME_SWATCHES.length, id + " card has a swatch per token");
    for (const [, scope, token] of spans){
      assert.equal(scope, id, id + " swatch is drawn in its own theme");
      for (const t of THEMES) resolve(t, token);  // a token every theme defines, as a solid color
    }
    assert.doesNotMatch(body, /#[0-9a-fA-F]{3,8}\b/, id + " card has no color literal");
  }
  // The swatch edge stays the current theme's line, not the previewed theme's.
  assert.equal(declared(".theme-swatches", "--swatch-edge"), "--line");
  assert.match(rule(".theme-swatches span").get("border") || "", /var\(--swatch-edge\)/);
});
