# Web portal UI guidelines

Rules for changes to the web portal in `crates/pdm-api/static/` (`index.html`, `style.css`, `app.js`, `editors.*`). `onshape.html`, the page shown inside the CAD panel, follows the same rules.

The portal is a work tool. People spend hours in it reading part numbers, comparing versions, checking files in and out, building nests and changing settings. The rules favor legibility, predictability and density over decoration, and every rule is written so a reviewer can check it on a diff.

**How to use this**

- Rules are numbered by section (T1, Y2, S3 ...). Cite the number in review comments.
- Some existing code predates these rules. They apply to new and changed code; a gap in older code is a known gap, not a license. Fix it when you touch that code, or in a PR of its own.
- Some tokens these rules name are not in the stylesheet yet; a token is defined in every theme block (T2) when it is introduced.
- Do not restyle working code for taste: change what a rule or a reported problem justifies, and keep every id, `data-*` attribute and class that `app.js` reads.

## 1. Tokens and themes

- **T1 Tokens only.** Components take colors and fonts from tokens. No hex, `rgb()`, `hsl()`, named color or `font-family` literal outside the theme token blocks, and that includes inline `style` strings and SVG attributes built in JS. `transparent`, `currentColor` and `inherit` are fine. One exception: while a new token is still being added in another open change, `var(--new-token, <current value>)` may carry the current value as a fallback; drop the fallback once the token is defined in every theme.
- **T2 One token set per theme, declared once.** Every theme block defines every token. A new token lands in all four themes (dark, light, slate, contrast) in the same PR. A second, partial block would let the themes it leaves out silently inherit another theme's values.
- **T3 Every fill has an ink.** A color used as a background under text (accent, ok, err, warn) gets a paired text token, such as `--on-acc`, that passes C1 on it in every theme. Never assume white.
- **T4 Names say the role.** `--err`, never `--red`. Keep the existing names (such as `--bg --panel --panel2 --line --fg --dim --acc --ok --err --warn --hover --active --head`): add tokens, do not rename them.
- **T5 Drawings are themed too.** Nest previews, flat-pattern legends and any chart take named tokens (for example `--draw-sheet`, `--draw-profile`, `--draw-hole`) defined in every theme.

## 2. Organization branding

These rules apply when the portal lets an organization brand it.

- **B1 Branding is token values, nothing else.** An admin's color overrides are stored per theme as token and value pairs and applied after the theme blocks. No component reads branding settings directly.
- **B2 Overrides are checked as they are chosen.** Each override is tested against the pairs it takes part in (text on surface, ink on fill, focus ring on background) at the C1 thresholds. A failing pair shows a warning that names the pair and its measured ratio; the admin can still save. The branding editor previews all four themes and offers "Reset to default" per token and per theme.
- **B3 Every theme can be branded.** Overrides apply to all four themes, high contrast included; the contrast theme shows the same warnings, so an admin who weakens it sees exactly which pairs fall below AA.
- **B4 The logo has a fixed box.** It replaces the mark and product name in the sidebar and on the sign-in page. It fits a height token and the sidebar width, keeps its aspect ratio, has `alt` set to the organization's name, is never recolored with CSS filters, and passes 3:1 against `--panel` in each theme it shows in (or ships a light and a dark variant). When missing or broken it falls back to the text name.
- **B5 The product stays findable.** The product name and version are shown on the System page so support can tell what is running.

## 3. Type

- **Y1 Two font tokens.** `--font-ui` for reading and `--font-mono` for identifiers, measured values and code. Nothing else sets `font-family`.
- **Y2 A chosen reading font only sets the tokens.** When the portal lets users pick a reading font, the choice sets `--font-ui`, and optionally `--font-mono`, on `<html>`. Faces are bundled with the portal: open-license, self-hosted under `static/fonts/` with their license text beside them, and loaded by `@font-face` with `font-display: swap` and a system fallback. No CDN and no remote `@import`: installs can be offline. The default stays the system UI stack.
- **Y3 Layout survives a font swap.** Sizes in `rem`, widths in `rem`, `ch` or sized by content; long labels wrap or truncate with the full text still available. When reading fonts are bundled (Y2), test new layouts with the widest one.
- **Y4 Sizes come from the scale.** Use the `--fs-*` steps (11, 12, 14, 16, 20, 24px), moved to `rem` (Y3) when they are first wired in. Body text and table cells `--fs-base` (14px); compact cells, labels, badges and helper text `--fs-sm` (12px); `--fs-xs` (11px) only for text that repeats something shown elsewhere (a count on a tab, a shortcut hint). Nothing smaller, SVG annotations included.
- **Y5 Labels in sentence case, untracked.** Labels, table heads, section heads, tabs and badges are not uppercased (in CSS or in JS) and not letter-spaced: small capitals are harder to read and fight a chosen reading font. Data keeps its own case: part numbers, revision letters, file extensions.
- **Y6 Figures line up.** Numbers that compare use tabular numerals and right-align in tables. Units go in the column head, not in every cell.

## 4. Color and contrast

- **C1 WCAG 2.2 AA in every theme.** Text 4.5:1 against the surface it actually sits on; large text (24px, or 18.66px bold) 3:1; meaningful icons, focus rings and form-field edges 3:1. Check all four themes, not just the default.
- **C2 State is never color alone.** A status carries a word. Preview and diff marks differ by line style, shape or label as well as color. Red against green is never the only cue.
- **C3 The accent marks what you can act on.** The primary action, the current selection, links and focus. Not headings, not icon tiles, not decoration: used anywhere else, it stops pointing at actions.
- **C4 Status colors mean status.** `--ok`, `--err` and `--warn` appear only where something succeeded, failed or needs attention.

## 5. Layout and density

- **L1 Tables before cards.** Anything people sort, compare or scan (items, versions, jobs, users, tools) is a table. Cards are for things recognized by a picture, such as project covers.
- **L2 Table behavior.** Sticky header; one line per row by default, long text truncated with the full value in a tooltip and in the detail view; numbers per Y6; row actions in the last column; the row's main action reachable by keyboard (K1). Check the header really sticks: `overflow` on the table or an ancestor can stop it.
- **L3 Density changes padding, not legibility.** Comfortable and compact differ in padding (about 36px and 28px rows). Font sizes stay at or above Y4.
- **L4 One containment layer.** Inside a panel, group with a heading, space or a hairline, not another bordered box.
- **L5 Spacing from the scale.** Use `--sp-*`; a raw length needs a reason in the PR.
- **L6 Works at 375px.** No page-level horizontal scroll. Wide tables scroll inside their own container and keep the identifying column in view.

## 6. Buttons and actions

- **A1 One primary per view.** At most one filled primary button per view, for the action that screen exists for; a dialog counts as its own view. A page of several section forms (settings) uses default-style Save buttons named for their section.
- **A2 Verbs on buttons.** A label names the result: "Check in", "Queue nest", "Delete item". Never "OK", "Submit", "Yes" or "Go".
- **A3 Destructive actions look destructive.** Use the danger style (`button.danger`), never the primary fill.
- **A4 Icons from the sprite.** Use the `#ic-*` symbols in `index.html`. No text glyphs as icons. Icon-only buttons carry `aria-label`; a `title` alone is not reliably announced.
- **A5 Targets are big enough.** At least 24 by 24px (WCAG 2.5.8), 44px under `(pointer: coarse)`.

## 7. Words

- **W1 Sentence case everywhere.** Navigation, headings, buttons, tabs, dialog titles.
- **W2 Units on every quantity.** In the label ("Sheet width (mm)") or the column head. Where a setting picks the unit system, the fields it governs show the unit.
- **W3 No jargon without a hint.** A term a new user would not know gets a one-line helper. Raw JSON, internal ids and HTTP codes are not UI.
- **W4 Errors say what, why and what next.** Plain words first; the server's message is detail, not the headline. No "Something went wrong", "Error", "Oops" or exclamation marks.
- **W5 Use the glossary term.** Name each thing with its term from [`glossary.md`](glossary.md), and do not use the words on its "Not:" line. A new thing gets its glossary entry in the same PR.

## 8. States

- **S1 Every async action has three states.** Busy (control disabled, label says what is happening, such as "Queueing..."), success and error. A second click while busy does nothing.
- **S2 Every data region has four states.** Loading (in place, shaped like the content where practical), empty, error with Retry, and full.
- **S3 Never swallow an error.** A failed load says so where the data would be. Never fall back to a default that a later save would write back: an empty form shown for settings that failed to load lets Save erase them.
- **S4 Success shows where it happened.** When the result is visible (the row's state changes, the panel updates), that is the confirmation. Toasts are for effects off screen (a download started, a job queued, another view changed), for failures, and for a confirmation that carries Undo (D1).
- **S5 Errors stay and are announced.** Error messages remain until dismissed or resolved and carry Retry where retrying makes sense. Use `role="alert"` for errors and `role="status"` for information.
- **S6 Empty states say what to do next.** Name what is empty and the next step: an action, or where items come from ("No versions yet — check in a file to create v1.").
- **S7 Disabled controls say why** when the reason is not obvious from the screen.

## 9. Destructive actions

- **D1 Reversible actions offer Undo, not a dialog.** Archive, unarchive and undo-checkout run at once, and the confirmation offers Undo.
- **D2 Irreversible actions confirm and name the object.** The app's dialog names the object (part number and name, project name) and what goes with it, and the danger button repeats the verb ("Delete item"). When a delete takes other records with it, the user types the object's name.
- **D3 No browser dialogs.** No `window.confirm`, `alert` or `prompt`: they cannot be themed or styled as danger, and they block the page.

## 10. Keyboard and focus

- **K1 Clickable means focusable.** Anything clickable is a `<button>`, a link with `href`, or a form control. No click handlers on divs, spans or rows without a focusable equivalent.
- **K2 Focus is always visible.** A 2px `:focus-visible` ring at 3:1 against its background, shown at once. Never `outline: none` without a replacement.
- **K3 Standard widgets behave like the standard.** Tabs use `tablist`, `tab` and `tabpanel` roles with `aria-selected` and arrow keys. Menus open with Enter or Space, close with Escape and return focus to their trigger.
- **K4 Nothing is hover-only.** What hover reveals, focus reveals too.
- **K5 Dialogs trap and return focus.** Use a native `<dialog>` with `showModal()`: focus moves in, stays in, Escape closes, focus returns to the opener.
- **K6 Shortcuts are discoverable.** Each shortcut is shown where it applies, as the search field's placeholder shows its "/" shortcut.

## 11. Forms

- **F1 Visible labels, bound to their fields.** `<label for>` or a wrapping label. A placeholder shows a format or an example, never the label: it disappears as soon as the user types.
- **F2 Validate inline.** Check on blur once a field is touched, then as the user types. The message sits under the field (`aria-describedby`, `aria-invalid`), and a failed submit focuses the first invalid field. Never correct a value silently.
- **F3 Say what is required and allowed.** Required fields are marked in words. Number fields state their unit and range.
- **F4 Secrets show state, never value.** A blank secret field means "unchanged" and says so.

## 12. Settings

- **G1 Each setting explains itself.** What it changes, where it applies (this browser, my account, the organization) and its default.
- **G2 Changes from default are visible and reversible.** A changed value is marked, and each section can return to its defaults.

## 13. Motion

- **M1 Motion shows a change of state, nothing else.** 100 to 250ms on `transform` and `opacity`; never `transition: all`. Infinite animation only for indeterminate progress. Everything stops under `prefers-reduced-motion`.

## 14. Before you merge

- [ ] No new color or `font-family` literal outside the token blocks: `git diff -U0 -- crates/pdm-api/static | grep -nE '^\+.*(#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\(|font-family)'` prints only token-block lines (and T1's temporary `var()` fallbacks).
- [ ] No new `confirm(`, `alert(` or `prompt(`; no new click handler on a div, span or row.
- [ ] Looked at in all four themes; new text, fills and field edges meet C1.
- [ ] Keyboard only: every new control is reached with Tab, works with Enter or Space, shows focus; Escape closes overlays.
- [ ] Every new async action shows busy, success and error; a double click cannot submit twice.
- [ ] Every new data region has loading, empty (with a next step) and error (with Retry) states.
- [ ] Destructive actions follow D1 to D3.
- [ ] Words: sentence case, a verb on every button, a unit on every quantity, no "OK".
- [ ] Checked at 375px wide and in compact density; when reading fonts are bundled (Y2), also with the widest one.
- [ ] Every id, `data-*` attribute and class that `app.js` reads still exists, or every reader was updated.
- [ ] Visual changes have before and after screenshots in the PR, dark and light at least.

---

Sources: WCAG 2.2 (contrast, non-text contrast, target size, focus). The state, ink-pairing and keep-what-works rules adapt ideas from the open-source Hallmark design skill, retargeted from marketing pages to this application.
