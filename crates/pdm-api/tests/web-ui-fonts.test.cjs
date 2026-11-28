// Reading-font setting checks for the web portal: bundled faces, @font-face rules, the per-browser
// choice in Settings and the <head> script that applies it before first paint.
// Run with: node --test crates/pdm-api/tests/web-ui-fonts.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
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
const settings = () => loadFunctions(["fontSettings"], {}).fontSettings();
// Declarations of the rules with exactly this selector, as a Map.
function ruleDecls(selector){
  const found = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(m => m[1].trim() === selector);
  assert(found.length > 0, "a rule for " + selector);
  return new Map(found.flatMap(m => m[2].split(";").map(d => d.match(/^\s*([-\w]+)\s*:\s*([\s\S]+?)\s*$/))
    .filter(Boolean).map(d => [d[1], d[2]])));
}
const fontFaces = () => [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map(m => new Map(m[1].split(";")
  .map(d => d.match(/^\s*([-\w]+)\s*:\s*([\s\S]+?)\s*$/)).filter(Boolean).map(d => [d[1], d[2]])));

// ── Bundled files ───────────────────────────────────────────────────────
const FONTS_DIR = path.join(STATIC, "fonts");
const FAMILIES = {
  "Atkinson Hyperlegible Next":"atkinson-hyperlegible-next",
  "Atkinson Hyperlegible Mono":"atkinson-hyperlegible-mono",
};

// FONTS.txt records the SHA-256 of each file as published at the source commit it names.
test("each bundled file matches the SHA-256 that FONTS.txt records for it", () => {
  const listed = [...fs.readFileSync(path.join(FONTS_DIR, "FONTS.txt"), "utf8")
    .matchAll(/^([0-9a-f]{64})\s+(\S+\.(?:ttf|txt))\b/gm)].map(m => ({ sha:m[1], file:m[2] }));
  const onDisk = Object.values(FAMILIES).flatMap(dir => fs.readdirSync(path.join(FONTS_DIR, dir)).map(f => dir + "/" + f));
  assert.deepEqual(listed.map(l => l.file).sort(), onDisk.sort(), "FONTS.txt lists exactly the files beside the fonts");
  assert.equal(onDisk.filter(f => /\.(ttf|otf|woff2?)$/.test(f)).length, 4, "upright and italic of each family");
  for (const { sha, file } of listed){
    let bytes = fs.readFileSync(path.join(FONTS_DIR, file));
    // License text is hashed with LF line endings; a Windows checkout may have turned them into CRLF.
    if (file.endsWith(".txt")) bytes = Buffer.from(bytes.toString("latin1").replace(/\r\n/g, "\n"), "latin1");
    assert.equal(crypto.createHash("sha256").update(bytes).digest("hex"), sha, file + " matches its recorded SHA-256");
  }
  const total = onDisk.reduce((n, f) => n + fs.statSync(path.join(FONTS_DIR, f)).size, 0);
  assert(total < 400 * 1024, "all faces together stay under 400 KB (" + total + " bytes)");
});

test("the SIL Open Font License travels beside each family", () => {
  for (const [family, dir] of Object.entries(FAMILIES)){
    const ofl = fs.readFileSync(path.join(FONTS_DIR, dir, "OFL.txt"), "utf8");
    assert.match(ofl, new RegExp("^Copyright .*" + family + " Project Authors"), dir + "/OFL.txt keeps its copyright line");
    assert.match(ofl, /SIL OPEN FONT LICENSE Version 1\.1/, dir + "/OFL.txt is the OFL 1.1 text");
  }
});

// ── @font-face and the font tokens ──────────────────────────────────────
test("@font-face declares an upright and an italic variable face per family, swapped in when loaded", () => {
  const faces = fontFaces();
  const seen = [];
  for (const f of faces){
    const family = (f.get("font-family") || "").replace(/^"|"$/g, "");
    assert(FAMILIES[family], "known family " + family);
    seen.push(family + "/" + f.get("font-style"));
    assert.equal(f.get("font-weight"), "200 800", family + " covers the weight axis");
    assert.equal(f.get("font-display"), "swap", family + " shows fallback text while loading");
    const src = (f.get("src") || "").match(/^url\("fonts\/([^"]+)"\) format\("truetype"\)$/);
    assert(src, family + " src is one local file: " + f.get("src"));
    assert(src[1].startsWith(FAMILIES[family] + "/"), family + " file sits in its own folder");
    assert(fs.existsSync(path.join(FONTS_DIR, src[1])), src[1] + " exists");
    assert.equal(/Italic/.test(src[1]), f.get("font-style") === "italic", src[1] + " matches its font-style");
  }
  assert.deepEqual(seen.sort(), Object.keys(FAMILIES).flatMap(f => [f + "/italic", f + "/normal"]).sort());
  assert.doesNotMatch(css, /@import|https?:\/\//, "no remote fonts or stylesheets: installs can be offline");
});

test("the font tokens default to the system stacks and a choice prepends its bundled face", () => {
  const root = ruleDecls(":root");
  assert.match(root.get("--font-ui"), /^system-ui,.*sans-serif$/);
  assert.match(root.get("--font-mono"), /^ui-monospace,.*monospace$/);
  for (const s of settings()){
    const token = "--" + s.attr.replace(/^data-/, "");
    assert(root.has(token), s.attr + " has a matching token " + token);
    for (const o of s.options.filter(o => o.id !== "system")){
      const v = ruleDecls("html[" + s.attr + "=\"" + o.id + "\"]").get(token);
      assert.equal(v, "\"" + o.name + "\"," + root.get(token), o.name + " falls back to the default stack");
      assert(fontFaces().some(f => f.get("font-family") === "\"" + o.name + "\""), o.name + " has @font-face rules");
    }
  }
  assert.equal(ruleDecls("body").get("font"), "14px/1.5 var(--font-ui)", "body text uses the reading font, same size");
  assert.equal(ruleDecls(".mono").get("font-family"), "var(--font-mono)");
});

test("a bundled face is named only by its @font-face and choice rules, so the default loads no font file", () => {
  const outside = css.replace(/@font-face\s*\{[^}]*\}/g, "").replace(/html\[data-font-[\w-]+="[\w-]+"\]\s*\{[^}]*\}/g, "");
  assert.doesNotMatch(outside, /Atkinson/, "style.css names a bundled face outside its rules");
  assert.doesNotMatch(app, /style=[^>]*Atkinson|font-family/, "app.js shows no sample in a bundled face");
});

// ── Applied before first paint ──────────────────────────────────────────
function headScript(){
  const head = indexHtml.slice(0, indexHtml.indexOf("</head>"));
  const scripts = [...head.matchAll(/<script>([\s\S]*?)<\/script>/g)].map(m => m[1]).filter(s => s.includes("pdm_font_ui"));
  assert.equal(scripts.length, 1, "one inline <head> script reads the font choice");
  assert.match(scripts[0], /pdm_theme/, "the font choice is applied by the same script as the theme");
  return scripts[0];
}
// Returns the attributes set on <html> and the links added to <head>; throws if the script itself fails.
function runHead(stored, opts = {}){
  const attrs = {}, links = [];
  const localStorage = { getItem(k){ if (opts.throws) throw new Error("storage blocked"); return k in stored ? stored[k] : null; } };
  const document = {
    documentElement:{ setAttribute(k, v){ attrs[k] = v; }, getAttribute(k){ return attrs[k]; } },
    createElement(tag){ assert.equal(tag, "link"); return {}; },
    head:{ appendChild(el){ links.push(Object.assign({}, el)); } },
  };
  const context = vm.createContext({ localStorage, document, assert });
  vm.runInContext(headScript().replace("} catch (e) {}", "} catch (e) { if (!" + Boolean(opts.throws) + ") throw e; }"), context);
  assert.deepEqual(Object.keys(context).sort(), ["assert", "document", "localStorage"], "the script leaves no globals behind");
  return { attrs, links };
}
// The upright face of a family, as style.css's @font-face names it, as a URL path.
const uprightSrc = family => "/static/fonts/" + fontFaces().find(f => f.get("font-family") === "\"" + family + "\"" &&
  f.get("font-style") === "normal").get("src").match(/url\("fonts\/([^"]+)"\)/)[1];

test("index.html applies the saved fonts in <head>, with the same keys, ids and default as app.js", () => {
  assert(indexHtml.indexOf("pdm_font_ui") < indexHtml.indexOf("<link rel=\"stylesheet\""), "before the stylesheets load");
  for (const s of settings()){
    for (const id of [...s.options.map(o => o.id), "neon", "Atkinson", "", null]){
      const { attrs, links } = runHead({ [s.key]:id });
      const chosen = id !== "system" && s.options.some(o => o.id === id);
      assert.deepEqual(attrs, chosen ? { [s.attr]:id } : {}, s.key + "=" + id);
      // A chosen face is fetched at once (preload), so the first paint can use it instead of swapping later.
      const face = chosen && s.options.find(o => o.id === id).name;
      assert.deepEqual(links, chosen ? [{ rel:"preload", as:"font", type:"font/ttf", crossOrigin:"anonymous", href:uprightSrc(face) }] : [],
        s.key + "=" + id + " preload");
    }
  }
  const both = runHead({ pdm_theme:"light", pdm_font_ui:"atkinson", pdm_font_mono:"atkinson" });
  assert.deepEqual(both.attrs, { "data-theme":"light", "data-font-ui":"atkinson", "data-font-mono":"atkinson" });
  assert.equal(both.links.length, 2);
  assert.deepEqual(runHead({ pdm_font_ui:"atkinson" }, { throws:true }), { attrs:{}, links:[] }, "blocked storage does not stop the page");
  assert.doesNotMatch(indexHtml.match(/<html[^>]*>/)[0], /data-font/, "no font attribute without a saved choice");
});

// ── The setting in Settings ─────────────────────────────────────────────
function fontCtx(stored = {}){
  const attrs = {};
  const ctx = loadFunctions(["fontSettings", "currentFont", "applyFont", "fontPickerHTML", "wireFontPicker"], {
    esc,
    localStorage:{ getItem:k => (k in stored ? stored[k] : null), setItem(k, v){ stored[k] = String(v); } },
    document:{ documentElement:{ setAttribute(k, v){ attrs[k] = v; }, removeAttribute(k){ delete attrs[k]; } } },
  });
  return { ctx, attrs, stored };
}

test("each font setting defaults to System default and ignores unknown saved values", () => {
  const { ctx } = fontCtx({ pdm_font_ui:"atkinson", pdm_font_mono:"comic" });
  const [ui, mono] = ctx.fontSettings();
  assert.equal(ctx.currentFont(ui), "atkinson");
  assert.equal(ctx.currentFont(mono), "system");
  assert.equal(fontCtx().ctx.currentFont(ui), "system");
  assert.deepEqual([...ctx.fontSettings().map(s => s.key)], ["pdm_font_ui", "pdm_font_mono"]);
});

test("choosing a font sets it on <html> at once and remembers it in this browser", () => {
  const { ctx, attrs, stored } = fontCtx();
  const [ui, mono] = ctx.fontSettings();
  ctx.applyFont(ui, "atkinson");
  ctx.applyFont(mono, "atkinson");
  assert.deepEqual(attrs, { "data-font-ui":"atkinson", "data-font-mono":"atkinson" });
  assert.deepEqual(stored, { pdm_font_ui:"atkinson", pdm_font_mono:"atkinson" });
  ctx.applyFont(ui, "system");
  ctx.applyFont(mono, "nope");
  assert.deepEqual(attrs, {}, "System default removes the attribute, so the token keeps its default");
  assert.deepEqual(stored, { pdm_font_ui:"system", pdm_font_mono:"system" });
});

test("the picker labels each select, marks the current choice and explains effect, scope and default", () => {
  const { ctx } = fontCtx({ pdm_font_mono:"atkinson" });
  const html = ctx.fontPickerHTML();
  for (const s of ctx.fontSettings()){
    assert.match(html, new RegExp("<label for='" + s.id + "'>" + s.label + "</label><select id='" + s.id + "'"));
    assert.match(html, new RegExp("aria-describedby='" + s.id + "-hint'"));
    assert.match(html, new RegExp("<p class='dim' id='" + s.id + "-hint'>[^<]*Default: System default"), s.label + " says its default");
  }
  assert.match(html, /<option value='atkinson' selected>Atkinson Hyperlegible Mono<\/option>/);
  assert.match(html, /<option value='system' selected>System default<\/option>/, "reading font still on its default");
  assert.match(html, /stored in this browser/i, "says where the choice applies");
  assert.match(html, /nothing is downloaded from the internet/, "says the fonts are bundled");
});

test("changing a select applies that font", () => {
  const { ctx, attrs } = fontCtx();
  const sel = { dataset:{ fontKey:"pdm_font_ui" }, value:"atkinson" };
  ctx.wireFontPicker({ querySelectorAll:q => (q === "[data-font-key]" ? [sel] : []) });
  sel.onchange();
  assert.deepEqual(attrs, { "data-font-ui":"atkinson" });
});

test("Settings shows the font settings in the Appearance card, right after the theme picker", () => {
  const start = app.indexOf("async function viewSettings(");
  const body = app.slice(start, app.indexOf("\nasync function ", start + 1));
  assert.match(body, /themePickerHTML\(\) \+ fontPickerHTML\(\) \+ "<\/div>"/);
  assert.match(body, /wireFontPicker\(v\);\r?\n\s*wireThemePicker\(v\);/);
});
