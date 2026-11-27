//! Organization branding: the display name, logo flags and color overrides an admin can set,
//! their validation, the CSS the server builds from them, and the logo format check.
//!
//! Colors are only ever written into CSS by [`css`], from the [`THEMES`] and [`TOKENS`]
//! allow-lists and strict `#rrggbb` values, so nothing else from a request reaches a stylesheet.
//! Logos are PNG or WebP only; [`sniff_logo`] checks the magic bytes and reads the size from the
//! header without decoding the image.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::PdmError;

/// The portal's themes (`data-theme` values). Every one of them can be overridden, high
/// contrast included.
pub const THEMES: [&str; 4] = ["dark", "light", "slate", "contrast"];

/// The color tokens an organization may override, without the leading `--`: the per-theme
/// colors of `crates/pdm-api/static/style.css`. Keep it in step with the theme blocks there: a
/// token missing here cannot be branded, and a name here that style.css does not use has no
/// effect. Left out on purpose:
/// - `--shadow` and `--backdrop`: not plain colors (a shadow, a color with alpha);
/// - `--paper`, `--canvas` and `--draw-*`: preview and drawing surfaces that match images the
///   server renders and the nest legend, not branding;
/// - `--text-1`, `--text-3`, `--surface*`, `--focus-ring`: derived from the tokens below;
/// - the spacing, radius and type scales.
pub const TOKENS: &[&str] = &[
    // Surfaces, lines, text, accent, status colors, hover/selected rows, table heads.
    "bg",
    "panel",
    "panel2",
    "line",
    "fg",
    "dim",
    "acc",
    "ok",
    "err",
    "warn",
    "hover",
    "active",
    "head",
    // Text on the accent color, primary buttons, the selected segment, form-field edges,
    // placeholder marks, secondary text and lifecycle colors.
    "on-acc",
    "primary-bg",
    "primary-border",
    "primary-hover",
    "primary-fg",
    "seg-on-bg",
    "field-line",
    "mark-dim",
    "text-2",
    "life-wip",
    "life-in-review",
    "life-draft",
    "life-released",
    "life-obsolete",
    "life-deprecated",
    "life-lock",
];

/// Longest display name, in characters.
pub const DISPLAY_NAME_MAX: usize = 60;
/// Largest logo file, in bytes (256 KiB). The upload route and the database enforce it too.
pub const LOGO_MAX_BYTES: usize = 256 * 1024;
/// Smallest and largest logo width or height, in pixels.
pub const LOGO_MIN_PX: u32 = 16;
pub const LOGO_MAX_PX: u32 = 2048;
/// The `kind` of the main logo in `org_branding_assets` (`icon` is reserved for a separate
/// favicon later).
pub const LOGO: &str = "logo";

/// An organization's branding, stored in `organizations.settings->'branding'`. The logo bytes
/// live in `org_branding_assets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrandingSettings {
    /// Shown instead of the product name; `None` keeps the product name.
    pub display_name: Option<String>,
    /// Show the name next to the logo (otherwise the logo alone, with the name as its alt text).
    pub show_name_with_logo: bool,
    /// Use the logo as the browser tab icon.
    pub favicon_from_logo: bool,
    /// Color overrides: theme → token → `#rrggbb`.
    pub colors: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for BrandingSettings {
    fn default() -> Self {
        Self {
            display_name: None,
            show_name_with_logo: true,
            favicon_from_logo: false,
            colors: BTreeMap::new(),
        }
    }
}

/// What a partial update changed, for the audit record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BrandingChange {
    /// Top-level fields the update set.
    pub fields: Vec<String>,
    /// Themes whose colors it touched.
    pub themes: Vec<String>,
}

fn bad(msg: String) -> PdmError {
    PdmError::BadRequest(msg)
}

/// `#rrggbb` in lower case, or `None`. No short form, alpha, names, `var()` or anything else.
pub fn parse_hex(value: &str) -> Option<String> {
    let digits = value.strip_prefix('#')?;
    if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

/// Control characters and the invisible ones that change how text around them is shown:
/// zero-width characters, direction marks, embeddings and isolates, line and paragraph
/// separators, and the byte-order mark.
fn is_hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// The display name as stored: trimmed, at most [`DISPLAY_NAME_MAX`] characters, no control or
/// invisible formatting characters. An empty name clears it.
pub fn clean_display_name(name: &str) -> Result<Option<String>, PdmError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > DISPLAY_NAME_MAX {
        return Err(bad(format!(
            "display_name must be at most {DISPLAY_NAME_MAX} characters"
        )));
    }
    if name.chars().any(is_hidden_char) {
        return Err(bad(
            "display_name must not contain control or invisible formatting characters".into(),
        ));
    }
    Ok(Some(name.to_string()))
}

fn check_theme(theme: &str) -> Result<(), PdmError> {
    if THEMES.contains(&theme) {
        Ok(())
    } else {
        Err(bad(format!(
            "colors.{}: unknown theme (themes: {})",
            short_key(theme),
            THEMES.join(", ")
        )))
    }
}

fn check_token(theme: &str, token: &str) -> Result<(), PdmError> {
    if TOKENS.contains(&token) {
        Ok(())
    } else {
        Err(bad(format!(
            "colors.{theme}.{}: unknown color token",
            short_key(token)
        )))
    }
}

/// A request key as it is quoted back in an error: at most 40 characters.
fn short_key(key: &str) -> String {
    key.chars().take(40).collect()
}

impl BrandingSettings {
    /// Check every field and bring it to its stored form: the display name trimmed (an empty
    /// one cleared), colors in lower case, themes without overrides dropped. The error names
    /// the offending key.
    pub fn validate(&mut self) -> Result<(), PdmError> {
        self.display_name = match self.display_name.as_deref() {
            Some(name) => clean_display_name(name)?,
            None => None,
        };
        for (theme, tokens) in &mut self.colors {
            check_theme(theme)?;
            for (token, value) in tokens.iter_mut() {
                check_token(theme, token)?;
                *value = parse_hex(value).ok_or_else(|| {
                    bad(format!("colors.{theme}.{token} must be a #rrggbb color"))
                })?;
            }
        }
        self.colors.retain(|_, tokens| !tokens.is_empty());
        Ok(())
    }

    /// The stored settings as read back: like [`Self::validate`], but an entry that no longer
    /// passes (a token since dropped from [`TOKENS`], a hand-edited value) is dropped on its own
    /// instead of failing the whole object, so the rest of the branding survives.
    pub fn sanitized(mut self) -> Self {
        self.display_name = self
            .display_name
            .as_deref()
            .and_then(|name| clean_display_name(name).ok().flatten());
        self.colors
            .retain(|theme, _| THEMES.contains(&theme.as_str()));
        for tokens in self.colors.values_mut() {
            let kept: BTreeMap<String, String> = std::mem::take(tokens)
                .into_iter()
                .filter(|(token, _)| TOKENS.contains(&token.as_str()))
                .filter_map(|(token, value)| parse_hex(&value).map(|hex| (token, hex)))
                .collect();
            *tokens = kept;
        }
        self.colors.retain(|_, tokens| !tokens.is_empty());
        self
    }

    /// Apply a partial update (the body of `PUT /settings/branding`), then validate.
    ///
    /// An absent field keeps its value; `display_name: null` clears the name. `colors` merges
    /// per theme: a token set to `null` drops that override, a theme set to `null` drops the
    /// theme, and `colors: null` drops every override. Unknown fields are refused. A refused
    /// update changes nothing.
    pub fn apply_patch(&mut self, patch: &serde_json::Value) -> Result<BrandingChange, PdmError> {
        let mut next = self.clone();
        let change = next.patch(patch)?;
        next.validate()?;
        *self = next;
        Ok(change)
    }

    fn patch(&mut self, patch: &serde_json::Value) -> Result<BrandingChange, PdmError> {
        let Some(patch) = patch.as_object() else {
            return Err(bad("the body must be a JSON object".into()));
        };
        let mut change = BrandingChange::default();
        for (key, value) in patch {
            match key.as_str() {
                "display_name" => {
                    self.display_name = match value {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(s) => clean_display_name(s)?,
                        _ => return Err(bad("display_name must be a string or null".into())),
                    };
                }
                "show_name_with_logo" | "favicon_from_logo" => {
                    let Some(flag) = value.as_bool() else {
                        return Err(bad(format!("{key} must be true or false")));
                    };
                    if key == "show_name_with_logo" {
                        self.show_name_with_logo = flag;
                    } else {
                        self.favicon_from_logo = flag;
                    }
                }
                "colors" => self.patch_colors(value, &mut change)?,
                other => return Err(bad(format!("unknown field {:?}", short_key(other)))),
            }
            change.fields.push(key.clone());
        }
        Ok(change)
    }

    fn patch_colors(
        &mut self,
        value: &serde_json::Value,
        change: &mut BrandingChange,
    ) -> Result<(), PdmError> {
        let themes = match value {
            serde_json::Value::Null => {
                change.themes.extend(self.colors.keys().cloned());
                self.colors.clear();
                return Ok(());
            }
            serde_json::Value::Object(themes) => themes,
            _ => return Err(bad("colors must be an object or null".into())),
        };
        for (theme, tokens) in themes {
            check_theme(theme)?;
            change.themes.push(theme.clone());
            let tokens = match tokens {
                serde_json::Value::Null => {
                    self.colors.remove(theme);
                    continue;
                }
                serde_json::Value::Object(tokens) => tokens,
                _ => {
                    return Err(bad(format!("colors.{theme} must be an object or null")));
                }
            };
            let entry = self.colors.entry(theme.clone()).or_default();
            for (token, color) in tokens {
                check_token(theme, token)?;
                match color {
                    serde_json::Value::Null => {
                        entry.remove(token);
                    }
                    serde_json::Value::String(s) => {
                        let hex = parse_hex(s).ok_or_else(|| {
                            bad(format!("colors.{theme}.{token} must be a #rrggbb color"))
                        })?;
                        entry.insert(token.clone(), hex);
                    }
                    _ => {
                        return Err(bad(format!(
                            "colors.{theme}.{token} must be a #rrggbb color or null"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The stylesheet for an organization's color overrides: one rule per theme, in theme order,
/// e.g. `html[data-theme="dark"]{--acc:#3b82f6;--on-acc:#0b1220}`. Only allow-listed themes and
/// tokens and valid hex values are written; anything else is skipped. The `html[data-theme=…]`
/// selector (specificity 0,1,1) beats style.css's `[data-theme=…]` and `:root` (0,1,0) whichever
/// stylesheet loads last.
pub fn css(settings: &BrandingSettings) -> String {
    let mut out = String::new();
    for (theme, tokens) in &settings.colors {
        if !THEMES.contains(&theme.as_str()) {
            continue;
        }
        let decls: Vec<String> = tokens
            .iter()
            .filter(|(token, _)| TOKENS.contains(&token.as_str()))
            .filter_map(|(token, value)| parse_hex(value).map(|hex| format!("--{token}:{hex}")))
            .collect();
        if decls.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "html[data-theme=\"{theme}\"]{{{}}}\n",
            decls.join(";")
        ));
    }
    out
}

/// What [`sniff_logo`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogoInfo {
    /// `image/png` or `image/webp`.
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// A stored branding image without its bytes.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BrandingAssetMeta {
    pub mime: String,
    pub sha256: String,
    pub width: i32,
    pub height: i32,
    /// File size in bytes.
    pub size_bytes: i32,
    pub updated_at: DateTime<Utc>,
}

/// A stored branding image.
#[derive(Debug, Clone, FromRow)]
pub struct BrandingAsset {
    pub mime: String,
    pub sha256: String,
    pub width: i32,
    pub height: i32,
    pub size_bytes: i32,
    pub updated_at: DateTime<Utc>,
    pub bytes: Vec<u8>,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(u32::from(s[0]) | (u32::from(s[1]) << 8) | (u32::from(s[2]) << 16))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 2)?;
    Some(u32::from(s[0]) | (u32::from(s[1]) << 8))
}

/// The format and pixel size of a logo, from its header only (the image is never decoded).
/// Accepts PNG and WebP (VP8X, VP8L and VP8 forms) between [`LOGO_MIN_PX`] and
/// [`LOGO_MAX_PX`] on each side; refuses everything else, SVG included.
pub fn sniff_logo(data: &[u8]) -> Result<LogoInfo, PdmError> {
    const UNSUPPORTED: &str = "the logo must be a PNG or WebP image";
    if data.len() < 30 {
        return Err(bad(UNSUPPORTED.into()));
    }
    let size = if data.starts_with(b"\x89PNG\r\n\x1a\n") && data.get(12..16) == Some(b"IHDR") {
        be32(data, 16)
            .zip(be32(data, 20))
            .map(|(w, h)| ("image/png", w, h))
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        match data.get(12..16) {
            Some(b"VP8X") => le24(data, 24)
                .zip(le24(data, 27))
                .map(|(w, h)| ("image/webp", w + 1, h + 1)),
            Some(b"VP8L") if data[20] == 0x2f => le32(data, 21).map(|bits| {
                (
                    "image/webp",
                    (bits & 0x3fff) + 1,
                    ((bits >> 14) & 0x3fff) + 1,
                )
            }),
            Some(b"VP8 ") if data.get(23..26) == Some(&[0x9d, 0x01, 0x2a][..]) => le16(data, 26)
                .zip(le16(data, 28))
                .map(|(w, h)| ("image/webp", w & 0x3fff, h & 0x3fff)),
            _ => None,
        }
    } else {
        None
    };
    let Some((mime, width, height)) = size else {
        return Err(bad(UNSUPPORTED.into()));
    };
    let range = LOGO_MIN_PX..=LOGO_MAX_PX;
    if !range.contains(&width) || !range.contains(&height) {
        return Err(bad(format!(
            "the logo is {width}×{height} px; each side must be {LOGO_MIN_PX}–{LOGO_MAX_PX} px"
        )));
    }
    Ok(LogoInfo {
        mime,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]); // depth, color type, …, CRC
        b.extend_from_slice(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82");
        b
    }

    fn riff(chunk: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut b = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        b.extend_from_slice(chunk);
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(payload);
        b.resize(b.len().max(40), 0);
        b
    }

    fn webp_vp8x(w: u32, h: u32) -> Vec<u8> {
        let (w, h) = ((w - 1).to_le_bytes(), (h - 1).to_le_bytes());
        riff(
            b"VP8X",
            &[0x10, 0, 0, 0, w[0], w[1], w[2], h[0], h[1], h[2]],
        )
    }

    fn webp_vp8l(w: u32, h: u32) -> Vec<u8> {
        let bits = ((w - 1) & 0x3fff) | (((h - 1) & 0x3fff) << 14);
        let mut payload = vec![0x2f];
        payload.extend_from_slice(&bits.to_le_bytes());
        riff(b"VP8L", &payload)
    }

    fn webp_vp8(w: u32, h: u32) -> Vec<u8> {
        let (w, h) = ((w as u16).to_le_bytes(), (h as u16).to_le_bytes());
        riff(
            b"VP8 ",
            &[0x50, 0x02, 0x00, 0x9d, 0x01, 0x2a, w[0], w[1], h[0], h[1]],
        )
    }

    fn settings(colors: &[(&str, &str, &str)]) -> BrandingSettings {
        let mut s = BrandingSettings::default();
        for (theme, token, value) in colors {
            s.colors
                .entry((*theme).to_string())
                .or_default()
                .insert((*token).to_string(), (*value).to_string());
        }
        s
    }

    fn rejected(colors: &[(&str, &str, &str)]) -> bool {
        matches!(settings(colors).validate(), Err(PdmError::BadRequest(_)))
    }

    #[test]
    fn validate_accepts_a_known_token_and_lower_cases_it() -> Result<(), PdmError> {
        let mut s = settings(&[("dark", "acc", "#3B82F6")]);
        s.validate()?;
        assert_eq!(s.colors["dark"]["acc"], "#3b82f6");
        Ok(())
    }

    #[test]
    fn validate_rejects_unknown_themes_tokens_and_anything_but_rrggbb() {
        assert!(rejected(&[("sepia", "acc", "#3b82f6")]));
        assert!(rejected(&[("dark", "shadow", "#000000")]));
        assert!(rejected(&[("dark", "font", "#000000")]));
        assert!(rejected(&[("dark", "--acc", "#000000")]));
        for value in [
            "red",
            "#fff",
            "#3b82f6ff",
            "#3b82f6;}body{display:none",
            "url(x)",
            "var(--acc)",
            "3b82f6",
            " #3b82f6",
            "#3b82g6",
            "",
        ] {
            assert!(
                rejected(&[("dark", "acc", value)]),
                "{value:?} was accepted"
            );
        }
    }

    #[test]
    fn validate_checks_the_display_name() -> Result<(), PdmError> {
        let mut s = BrandingSettings {
            display_name: Some("  Example Works  ".into()),
            ..Default::default()
        };
        s.validate()?;
        assert_eq!(s.display_name.as_deref(), Some("Example Works"));
        let mut blank = BrandingSettings {
            display_name: Some("   ".into()),
            ..Default::default()
        };
        blank.validate()?;
        assert_eq!(blank.display_name, None);
        // 60 characters (multi-byte ones count once) is the limit.
        let mut max = BrandingSettings {
            display_name: Some("é".repeat(60)),
            ..Default::default()
        };
        max.validate()?;
        for name in [
            "x".repeat(61),
            "Bell\u{0007}".into(),
            "Two\nLines".into(),
            "Right\u{202E}to left".into(),
            "Isolate\u{2066}x".into(),
            "Zero\u{200B}width".into(),
            "Line\u{2028}separator".into(),
            "Byte\u{FEFF}order".into(),
        ] {
            let mut s = BrandingSettings {
                display_name: Some(name),
                ..Default::default()
            };
            assert!(matches!(s.validate(), Err(PdmError::BadRequest(_))));
        }
        Ok(())
    }

    #[test]
    fn sanitized_drops_only_the_entries_that_no_longer_pass() {
        // E.g. stored before a token was removed from the allow-list, or edited by hand.
        let mut stored = settings(&[
            ("dark", "acc", "#3B82F6"),
            ("dark", "retired-token", "#000000"),
            ("dark", "fg", "red"),
            ("sepia", "acc", "#000000"),
            ("light", "retired-token", "#000000"),
        ]);
        stored.display_name = Some(" Example Works ".into());
        stored.favicon_from_logo = true;
        let s = stored.sanitized();
        assert_eq!(s.display_name.as_deref(), Some("Example Works"));
        assert!(s.favicon_from_logo);
        assert_eq!(s.colors.len(), 1);
        assert_eq!(s.colors["dark"].len(), 1);
        assert_eq!(s.colors["dark"]["acc"], "#3b82f6");
        let bad_name = BrandingSettings {
            display_name: Some("x".repeat(61)),
            ..Default::default()
        };
        assert_eq!(bad_name.sanitized().display_name, None);
    }

    #[test]
    fn patch_keeps_absent_fields_and_merges_colors_per_theme() -> Result<(), PdmError> {
        let mut s = settings(&[("dark", "acc", "#111111"), ("light", "acc", "#222222")]);
        s.display_name = Some("Example".into());
        let change = s.apply_patch(&json!({
            "colors": { "dark": { "on-acc": "#0B1220" }, "light": null }
        }))?;
        assert_eq!(s.display_name.as_deref(), Some("Example"));
        assert!(s.show_name_with_logo);
        assert_eq!(s.colors["dark"]["acc"], "#111111");
        assert_eq!(s.colors["dark"]["on-acc"], "#0b1220");
        assert!(!s.colors.contains_key("light"));
        assert_eq!(change.fields, vec!["colors"]);
        assert_eq!(change.themes, vec!["dark", "light"]);

        s.apply_patch(&json!({
            "display_name": null,
            "show_name_with_logo": false,
            "favicon_from_logo": true,
            "colors": { "dark": { "acc": null, "on-acc": null } }
        }))?;
        assert_eq!(s.display_name, None);
        assert!(!s.show_name_with_logo && s.favicon_from_logo);
        // A theme left with no overrides is dropped.
        assert!(s.colors.is_empty());

        let mut s = settings(&[("dark", "acc", "#111111"), ("contrast", "fg", "#ffffff")]);
        s.apply_patch(&json!({ "colors": null }))?;
        assert!(s.colors.is_empty());
        Ok(())
    }

    #[test]
    fn patch_refuses_bad_input_and_names_the_key() {
        let cases = [
            (json!([]), "JSON object"),
            (json!({ "logo": "x" }), "unknown field"),
            (json!({ "display_name": 5 }), "display_name"),
            (json!({ "display_name": "x".repeat(61) }), "display_name"),
            (
                json!({ "show_name_with_logo": "yes" }),
                "show_name_with_logo",
            ),
            (json!({ "colors": "red" }), "colors"),
            (json!({ "colors": { "sepia": {} } }), "colors.sepia"),
            (json!({ "colors": { "dark": "red" } }), "colors.dark"),
            (
                json!({ "colors": { "dark": { "shadow": "#000000" } } }),
                "colors.dark.shadow",
            ),
            (
                json!({ "colors": { "dark": { "acc": "red" } } }),
                "colors.dark.acc",
            ),
            (
                json!({ "colors": { "dark": { "acc": 5 } } }),
                "colors.dark.acc",
            ),
        ];
        for (patch, key) in cases {
            let mut s = BrandingSettings::default();
            let result = s.apply_patch(&patch);
            assert!(
                matches!(&result, Err(PdmError::BadRequest(msg)) if msg.contains(key)),
                "{patch}: expected a bad request naming {key}, got {result:?}"
            );
            // A refused update leaves the settings as they were.
            assert_eq!(s, BrandingSettings::default(), "{patch}");
        }
    }

    #[test]
    fn css_writes_one_rule_per_theme_in_a_stable_order() -> Result<(), PdmError> {
        let mut s = BrandingSettings::default();
        s.apply_patch(&json!({ "colors": {
            "light": { "acc": "#1D4ED8" },
            "dark": { "on-acc": "#0b1220", "acc": "#3b82f6" }
        } }))?;
        assert_eq!(
            css(&s),
            "html[data-theme=\"dark\"]{--acc:#3b82f6;--on-acc:#0b1220}\n\
             html[data-theme=\"light\"]{--acc:#1d4ed8}\n"
        );
        assert_eq!(css(&BrandingSettings::default()), "");
        Ok(())
    }

    #[test]
    fn css_skips_anything_not_allow_listed() {
        // Settings built directly (e.g. a hand-edited database row) are filtered again.
        let s = settings(&[
            ("dark", "acc", "#3b82f6"),
            ("dark", "shadow", "#000000"),
            ("dark", "fg", "red;}body{display:none"),
            ("evil\"]{x:y}", "acc", "#000000"),
            ("light", "bg", "url(x)"),
        ]);
        let out = css(&s);
        assert_eq!(out, "html[data-theme=\"dark\"]{--acc:#3b82f6}\n");
    }

    #[test]
    fn token_names_are_plain_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for token in TOKENS {
            assert!(
                !token.is_empty()
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{token:?}"
            );
            assert!(seen.insert(token), "{token} is listed twice");
        }
        for excluded in [
            "shadow",
            "backdrop",
            "paper",
            "canvas",
            "draw-profile",
            "text-1",
            "surface",
            "focus-ring",
            "sp-1",
            "r-s",
            "fs-base",
        ] {
            assert!(!TOKENS.contains(&excluded), "{excluded}");
        }
    }

    #[test]
    fn sniff_accepts_png_and_the_three_webp_forms() -> Result<(), PdmError> {
        let expect = |mime, width, height| LogoInfo {
            mime,
            width,
            height,
        };
        assert_eq!(sniff_logo(&png(480, 120))?, expect("image/png", 480, 120));
        assert_eq!(
            sniff_logo(&webp_vp8x(480, 120))?,
            expect("image/webp", 480, 120)
        );
        assert_eq!(
            sniff_logo(&webp_vp8l(300, 64))?,
            expect("image/webp", 300, 64)
        );
        assert_eq!(
            sniff_logo(&webp_vp8(200, 50))?,
            expect("image/webp", 200, 50)
        );
        assert_eq!(sniff_logo(&png(16, 2048))?, expect("image/png", 16, 2048));
        Ok(())
    }

    #[test]
    fn sniff_refuses_other_formats_short_input_and_odd_sizes() {
        let mut svg =
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>x()</script></svg>".to_vec();
        svg.resize(64, b' ');
        let mut gif = b"GIF89a\x10\x00\x10\x00".to_vec();
        gif.resize(64, 0);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0];
        jpeg.resize(64, 0);
        let mut html = b"<!doctype html><html><body>hi</body></html>".to_vec();
        html.resize(64, b' ');
        let mut riff_avi = webp_vp8x(64, 64);
        riff_avi[8..12].copy_from_slice(b"AVI ");
        let mut vp8l_no_sig = webp_vp8l(64, 64);
        vp8l_no_sig[20] = 0;
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("svg", svg),
            ("gif", gif),
            ("jpeg", jpeg),
            ("html", html),
            ("empty", vec![]),
            ("truncated png", png(480, 120)[..20].to_vec()),
            ("0x0 png", png(0, 0)),
            ("4096x10 png", png(4096, 10)),
            ("15px png", png(15, 100)),
            ("4096 webp", webp_vp8x(4096, 64)),
            ("riff not webp", riff_avi),
            ("vp8l without signature", vp8l_no_sig),
        ];
        for (label, data) in cases {
            assert!(
                matches!(sniff_logo(&data), Err(PdmError::BadRequest(_))),
                "{label} was accepted"
            );
        }
    }
}
