//! Each person's own sidebar menu: `GET/PUT/DELETE /me/menu`.
//!
//! The menu holds the order of the sidebar links, the person's own groups (three levels at most),
//! their names and icons, and the links the person hides. It follows the person to every browser.
//! It changes only what the sidebar shows. Every route keeps its own role checks, so a hidden
//! link removes no access and a shown link grants none.
//!
//! The server checks each menu before it stores it: known link ids, each link once, group names of
//! 1 to 60 characters with no markup, known default-group keys, and icons from the bundled Font
//! Awesome Free set (`static/icons/index.json`).
use std::collections::HashSet;
use std::sync::LazyLock;

use axum::Json;
use axum::extract::{DefaultBodyLimit, Extension, State};
use axum::routing::{MethodRouter, get};
use bytes::Bytes;
use serde_json::{Map, Value, json};

use pdm_core::PdmError;

use crate::auth::{Authed, require_user};
use crate::error::AppError;
use crate::state::AppState;

/// A full menu is a few kilobytes. A larger body is refused before it is read.
const MAX_BODY_BYTES: usize = 64 * 1024;
/// Groups can hold groups, three levels deep at most.
const MAX_DEPTH: usize = 3;
/// The longest group name, in characters.
const MAX_NAME_CHARS: usize = 60;
/// The most entries (links and groups) one menu holds.
const MAX_ENTRIES: usize = 200;

/// The sidebar's links: `data-nav` in `static/index.html`, in the default order.
pub const NAV_IDS: [&str; 16] = [
    "dashboard",
    "projects",
    "items",
    "search",
    "mfg",
    "machines",
    "tools",
    "post-processors",
    "sheet-metal",
    "laser-nests",
    "approvals",
    "activity",
    "jobs",
    "system",
    "admin",
    "settings",
];

/// The default groups: `data-group` in `static/index.html`. A group made from one keeps its icon.
pub const GROUP_KEYS: [&str; 5] = ["workspace", "data", "mfg", "actions", "admin"];

/// Every icon a menu may name, as `solid/<name>` or `regular/<name>`.
static ICONS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let index: Value =
        serde_json::from_str(include_str!("../static/icons/index.json")).unwrap_or(Value::Null);
    let mut out = HashSet::new();
    for icon in index["icons"].as_array().into_iter().flatten() {
        let (Some(name), Some(styles)) = (icon[0].as_str(), icon[1].as_str()) else {
            continue;
        };
        if styles.contains('s') {
            out.insert(format!("solid/{name}"));
        }
        if styles.contains('r') {
            out.insert(format!("regular/{name}"));
        }
    }
    out
});

/// The route's handlers, with the body limit.
pub fn method_router() -> MethodRouter<AppState> {
    get(get_menu)
        .put(put_menu)
        .delete(delete_menu)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

fn bad(msg: impl Into<String>) -> AppError {
    AppError(PdmError::BadRequest(msg.into()))
}

/// The caller's stored menu as `{ items, close_others }`. `items` is null for the default menu.
async fn stored(state: &AppState, user_id: uuid::Uuid) -> Result<Json<Value>, AppError> {
    let (items, close_others) = pdm_db::get_user_menu(&state.pool, user_id).await?;
    Ok(Json(
        json!({ "items": items, "close_others": close_others }),
    ))
}

/// GET /me/menu → `{ items, close_others }` for the caller.
pub async fn get_menu(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
) -> Result<Json<Value>, AppError> {
    let user = require_user(&authed)?;
    stored(&state, user.id).await
}

/// PUT /me/menu `{ items?, close_others? }` → the stored menu. The server stores each field the
/// body has and keeps the other. `items: null` puts the default menu back. The server checks the
/// whole body before it stores anything.
pub async fn put_menu(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let user = require_user(&authed)?;
    let body: Value =
        serde_json::from_slice(&body).map_err(|e| bad(format!("menu: not valid JSON ({e})")))?;
    let obj = body
        .as_object()
        .ok_or_else(|| bad("menu: send an object with items, close_others or both"))?;
    if let Some(k) = obj.keys().find(|k| *k != "items" && *k != "close_others") {
        return Err(bad(format!("menu: unknown field {k:?}")));
    }
    if obj.is_empty() {
        return Err(bad("menu: send items, close_others or both"));
    }
    let close_others = match obj.get("close_others") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(bad("menu: close_others must be true or false")),
    };
    let items = match obj.get("items") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => {
            validate_items(v).map_err(|e| bad(format!("menu: {e}")))?;
            Some(Some(v))
        }
    };
    // One statement for each request, so a body with both fields stores both or neither.
    match (items, close_others) {
        (Some(items), Some(close_others)) => {
            pdm_db::set_user_menu(&state.pool, user.id, items, close_others).await?
        }
        (Some(items), None) => pdm_db::set_user_menu_items(&state.pool, user.id, items).await?,
        (None, Some(close_others)) => {
            pdm_db::set_user_menu_close_others(&state.pool, user.id, close_others).await?
        }
        (None, None) => {}
    }
    stored(&state, user.id).await
}

/// DELETE /me/menu → the default menu again. The close option stays as it is.
pub async fn delete_menu(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
) -> Result<Json<Value>, AppError> {
    let user = require_user(&authed)?;
    pdm_db::set_user_menu_items(&state.pool, user.id, None).await?;
    stored(&state, user.id).await
}

/// Checks a menu's entries. The error names the first problem.
pub fn validate_items(items: &Value) -> Result<(), String> {
    let mut seen = HashSet::new();
    let mut count = 0;
    check_list(items, 0, &mut seen, &mut count)
}

fn check_list(
    list: &Value,
    depth: usize,
    seen: &mut HashSet<String>,
    count: &mut usize,
) -> Result<(), String> {
    let list = list.as_array().ok_or("items must be a list")?;
    for entry in list {
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(format!("a menu holds {MAX_ENTRIES} entries at most"));
        }
        let obj = entry
            .as_object()
            .ok_or("each entry must be an object with nav or group")?;
        if let Some(hidden) = obj.get("hidden")
            && !hidden.is_boolean()
        {
            return Err("hidden must be true or false".into());
        }
        if let Some(icon) = obj.get("icon")
            && !icon.as_str().is_some_and(|i| ICONS.contains(i))
        {
            return Err(format!("unknown icon {icon}"));
        }
        match (obj.get("nav"), obj.get("group")) {
            (Some(nav), None) => {
                only_fields(obj, &["nav", "hidden", "icon"])?;
                let nav = nav.as_str().unwrap_or_default();
                if !NAV_IDS.contains(&nav) {
                    return Err(format!("unknown link {nav:?}"));
                }
                if !seen.insert(format!("nav:{nav}")) {
                    return Err(format!("the link {nav:?} is in the menu twice"));
                }
            }
            (None, Some(name)) => {
                only_fields(obj, &["group", "key", "hidden", "icon", "items"])?;
                check_name(name)?;
                // Group names are compared without case, at every level: each group folds on its own.
                let lower = name.as_str().unwrap_or_default().to_lowercase();
                if !seen.insert(format!("group:{lower}")) {
                    return Err(format!("the group name {name} is in the menu twice"));
                }
                if let Some(key) = obj.get("key")
                    && !key.as_str().is_some_and(|k| GROUP_KEYS.contains(&k))
                {
                    return Err(format!("unknown group key {key}"));
                }
                if let Some(key) = obj.get("key").and_then(Value::as_str)
                    && !seen.insert(format!("key:{key}"))
                {
                    return Err(format!("the group key {key:?} is in the menu twice"));
                }
                if depth + 1 > MAX_DEPTH {
                    return Err(format!("groups can go {MAX_DEPTH} levels deep at most"));
                }
                if let Some(items) = obj.get("items") {
                    check_list(items, depth + 1, seen, count)?;
                }
            }
            _ => return Err("each entry has nav (a link) or group (a group), not both".into()),
        }
    }
    Ok(())
}

fn only_fields(obj: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unknown field {k:?}")),
        None => Ok(()),
    }
}

fn check_name(name: &Value) -> Result<(), String> {
    let name = name.as_str().ok_or("a group name must be text")?;
    if name.trim().is_empty() || name.trim() != name {
        return Err("a group name needs text, with no space at either end".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "a group name has {MAX_NAME_CHARS} characters at most"
        ));
    }
    if name.chars().any(|c| c == '<' || c == '>' || c.is_control()) {
        return Err("a group name cannot hold < or > or control characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_menu() -> Value {
        json!([
            { "group": "Workspace", "key": "workspace", "items": [{ "nav": "dashboard" }, { "nav": "projects" }] },
            { "group": "Data", "key": "data", "items": [{ "nav": "items" }, { "nav": "search", "hidden": true }] },
            { "group": "Mine", "icon": "solid/star", "items": [
                { "group": "Level 2", "items": [{ "group": "Level 3", "items": [{ "nav": "jobs", "icon": "regular/bell" }] }] }
            ] }
        ])
    }

    #[test]
    fn a_menu_with_groups_three_deep_passes() {
        assert_eq!(validate_items(&default_menu()), Ok(()));
        assert_eq!(validate_items(&json!([])), Ok(()));
    }

    #[test]
    fn bad_menus_are_refused_with_the_reason() {
        let cases = [
            (json!({ "nav": "dashboard" }), "items must be a list"),
            (json!([{ "nav": "not-a-page" }]), "unknown link"),
            (json!([{ "nav": "items" }, { "nav": "items" }]), "twice"),
            (json!([{ "nav": "items", "group": "X" }]), "not both"),
            (json!([{ "nav": "items", "extra": 1 }]), "unknown field"),
            (
                json!([{ "nav": "items", "hidden": "yes" }]),
                "true or false",
            ),
            (
                json!([{ "nav": "items", "icon": "solid/no-such-icon" }]),
                "unknown icon",
            ),
            (json!([{ "nav": "items", "icon": "house" }]), "unknown icon"),
            (
                json!([{ "group": "X", "key": "other" }]),
                "unknown group key",
            ),
            (json!([{ "group": "" }]), "needs text"),
            (json!([{ "group": " X" }]), "needs text"),
            (json!([{ "group": "x".repeat(61) }]), "60 characters"),
            (json!([{ "group": "<b>X</b>" }]), "cannot hold"),
            (
                json!([{ "group": "Shop" }, { "group": "A", "items": [{ "group": "shop" }] }]),
                "twice",
            ),
            (
                json!([{ "group": "A", "key": "data" }, { "group": "B", "key": "data" }]),
                "twice",
            ),
            (json!([{ "group": "A\u{7}" }]), "cannot hold"),
            (
                json!([{ "group": "1", "items": [{ "group": "2", "items": [{ "group": "3", "items": [{ "group": "4" }] }] }] }]),
                "levels deep",
            ),
        ];
        for (menu, want) in cases {
            // A menu that passes gives an empty reason, which fails the check below.
            let err = validate_items(&menu).err().unwrap_or_default();
            assert!(err.contains(want), "{menu}: {err:?}");
        }
        let many: Vec<Value> = (0..201)
            .map(|i| json!({ "group": format!("G{i}") }))
            .collect();
        assert!(
            validate_items(&Value::Array(many))
                .err()
                .unwrap_or_default()
                .contains("200 entries")
        );
        assert_eq!(
            validate_items(&json!([{ "group": "x".repeat(60) }])),
            Ok(()),
            "60 characters is allowed"
        );
    }

    #[test]
    fn the_icon_list_comes_from_the_bundled_index() {
        assert!(ICONS.len() > 1500, "{}", ICONS.len());
        for icon in ["solid/house", "solid/table-cells-large", "regular/bell"] {
            assert!(ICONS.contains(icon), "{icon}");
        }
        assert!(
            !ICONS.contains("regular/table-cells-large"),
            "no regular style in the free set"
        );
    }

    #[test]
    fn link_ids_and_group_keys_match_the_portal() {
        let html = include_str!("../static/index.html");
        let start = html.find("<nav id=\"sidebar\"").unwrap_or(0);
        let end = html[start..]
            .find("</nav>")
            .map_or(html.len(), |e| start + e);
        let nav = &html[start..end];
        let values = |attr: &str| -> Vec<String> {
            nav.split(attr)
                .skip(1)
                .filter_map(|s| s.split('"').next())
                .map(str::to_string)
                .collect()
        };
        assert_eq!(values("data-nav=\""), NAV_IDS);
        assert_eq!(values("data-group=\""), GROUP_KEYS);
    }
}
