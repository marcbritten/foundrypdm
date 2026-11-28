//! Organization branding routes: a display name, a logo and color overrides that replace the
//! "Foundry PDM" mark and theme colors in the portal.
//!
//! - `GET /settings/branding` (any signed-in caller), `PUT` and `DELETE` (admin): the settings.
//! - `GET /settings/branding/logo` (any signed-in caller), `PUT` and `DELETE` (admin): the logo,
//!   PNG or WebP only, at most 256 KiB.
//! - `GET /public/branding`, `/public/branding.css`, `/public/branding/logo` (no sign-in): what
//!   the sign-in page shows. Only the *sign-in organization* is ever served here: the one named
//!   by `PDM_SIGNIN_ORG`, or the only organization on a single-organization server. On a server
//!   with several organizations and no `PDM_SIGNIN_ORG`, these answer with the defaults, so one
//!   tenant's name and logo are never shown to another's users.
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Extension, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, put};
use axum::{Json, Router};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use pdm_core::branding::{self, BrandingAsset, LOGO};
use pdm_core::{PdmError, User};

use crate::auth::{Authed, require_user};
use crate::error::AppError;
use crate::state::AppState;

/// Largest `PUT /settings/branding` body. A full set of overrides for every theme is about 5 KB.
const SETTINGS_MAX_BYTES: usize = 64 * 1024;

/// The branding routes, merged into the API router in `lib.rs`. The two uploads have their own
/// body limits (the global one is the file-upload cap, 2 GiB by default).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/settings/branding",
            put(put_branding)
                .layer(DefaultBodyLimit::max(SETTINGS_MAX_BYTES))
                .get(get_branding)
                .delete(reset_branding),
        )
        .route(
            "/settings/branding/logo",
            put(put_logo)
                .layer(DefaultBodyLimit::max(branding::LOGO_MAX_BYTES))
                .get(get_logo)
                .delete(delete_logo),
        )
        .route("/public/branding", get(public_branding))
        .route("/public/branding.css", get(public_branding_css))
        .route("/public/branding/logo", get(public_logo))
}

fn require_admin(authed: &Authed) -> Result<&User, AppError> {
    let user = require_user(authed)?;
    if user.role != "admin" {
        return Err(AppError(PdmError::Forbidden("admin only".into())));
    }
    Ok(user)
}

/// `PDM_SIGNIN_ORG`, trimmed; unset or blank means "not set".
fn signin_org_name() -> Option<String> {
    std::env::var("PDM_SIGNIN_ORG")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn signin_org(state: &AppState) -> Result<Option<Uuid>, AppError> {
    Ok(pdm_db::signin_org(&state.pool, signin_org_name().as_deref()).await?)
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// The `v` in versioned logo URLs: the first 12 hex digits of the logo's SHA-256.
fn version_tag(sha256: &str) -> &str {
    sha256.get(..12).unwrap_or(sha256)
}

/// The settings plus what the editor needs: the logo's details, the allow-lists the server
/// enforces, and whether the sign-in page shows this organization.
async fn branding_view(state: &AppState, org: Uuid) -> Result<Value, AppError> {
    let settings = pdm_db::get_branding(&state.pool, org).await?;
    let logo = pdm_db::get_branding_asset_meta(&state.pool, org, LOGO).await?;
    let is_signin_org = signin_org(state).await? == Some(org);
    let logo = logo.map(|m| {
        let v = version_tag(&m.sha256);
        json!({
            "url": format!("/settings/branding/logo?v={v}"),
            "public_url": is_signin_org.then(|| format!("/public/branding/logo?v={v}")),
            "mime": m.mime,
            "width": m.width,
            "height": m.height,
            "bytes": m.size_bytes,
            "updated_at": m.updated_at,
        })
    });
    Ok(json!({
        "display_name": settings.display_name,
        "show_name_with_logo": settings.show_name_with_logo,
        "favicon_from_logo": settings.favicon_from_logo,
        "logo": logo,
        "colors": settings.colors,
        "themes": branding::THEMES,
        "tokens": branding::TOKENS,
        "signin_org": is_signin_org,
    }))
}

async fn audit(
    state: &AppState,
    org: Uuid,
    admin: &User,
    action: &str,
    detail: Option<Value>,
) -> Result<(), AppError> {
    pdm_db::record_audit(
        &state.pool,
        org,
        Some(admin.id),
        "",
        action,
        "organization",
        Some(org),
        detail,
    )
    .await?;
    Ok(())
}

/// GET /settings/branding — the caller's organization's branding.
async fn get_branding(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(branding_view(&state, authed.org_id).await?))
}

/// PUT /settings/branding — partial update (admin). Absent fields keep their value; see
/// `BrandingSettings::apply_patch` for the merge rules. 400 names the offending key.
async fn put_branding(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let admin = require_admin(&authed)?;
    let org = authed.org_id;
    let (_, change) = pdm_db::update_branding(&state.pool, org, &body).await?;
    audit(
        &state,
        org,
        admin,
        "org.branding_updated",
        Some(json!({ "fields": change.fields, "themes": change.themes })),
    )
    .await?;
    Ok(Json(branding_view(&state, org).await?))
}

/// DELETE /settings/branding — back to the defaults, logo included (admin).
async fn reset_branding(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
) -> Result<Json<Value>, AppError> {
    let admin = require_admin(&authed)?;
    let org = authed.org_id;
    pdm_db::clear_branding(&state.pool, org).await?;
    audit(&state, org, admin, "org.branding_reset", None).await?;
    Ok(Json(branding_view(&state, org).await?))
}

/// The media type of a request, without parameters, in lower case.
fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// PUT /settings/branding/logo — the raw image as the body (admin). `Content-Type` must be
/// `image/png` or `image/webp` and match the file; anything else, SVG included, is refused.
async fn put_logo(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let admin = require_admin(&authed)?;
    let org = authed.org_id;
    let declared = content_type(&headers);
    if declared != "image/png" && declared != "image/webp" {
        return Err(AppError(PdmError::BadRequest(
            "Content-Type must be image/png or image/webp (convert SVG to PNG first)".into(),
        )));
    }
    if body.len() > branding::LOGO_MAX_BYTES {
        return Err(AppError(PdmError::BadRequest(
            "the logo must be at most 256 KiB".into(),
        )));
    }
    let info = branding::sniff_logo(&body)?;
    if info.mime != declared {
        return Err(AppError(PdmError::BadRequest(format!(
            "Content-Type is {declared} but the file is {}",
            info.mime
        ))));
    }
    let sha = sha256_hex(&body);
    pdm_db::put_branding_asset(&state.pool, org, LOGO, &info, &body, &sha, Some(admin.id)).await?;
    audit(
        &state,
        org,
        admin,
        "org.branding_logo_set",
        Some(json!({ "mime": info.mime, "bytes": body.len(), "sha256": sha })),
    )
    .await?;
    Ok(Json(branding_view(&state, org).await?))
}

/// DELETE /settings/branding/logo — remove the logo (admin).
async fn delete_logo(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
) -> Result<Json<Value>, AppError> {
    let admin = require_admin(&authed)?;
    let org = authed.org_id;
    if pdm_db::delete_branding_asset(&state.pool, org, LOGO).await? {
        audit(&state, org, admin, "org.branding_logo_cleared", None).await?;
    }
    Ok(Json(branding_view(&state, org).await?))
}

#[derive(Deserialize)]
struct LogoQuery {
    v: Option<String>,
}

fn not_found(what: &str) -> AppError {
    AppError(PdmError::NotFound(what.into()))
}

/// GET /settings/branding/logo — the caller's organization's logo.
async fn get_logo(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
    Query(q): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_logo(&state, authed.org_id, q.v.as_deref(), &headers, "private").await
}

/// GET /public/branding/logo — the sign-in organization's logo (no sign-in).
async fn public_logo(
    State(state): State<AppState>,
    Query(q): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let org = signin_org(&state)
        .await?
        .ok_or_else(|| not_found("no logo"))?;
    serve_logo(&state, org, q.v.as_deref(), &headers, "public").await
}

/// An organization's logo. A request whose `If-None-Match` already names the current logo is
/// answered from the stored details alone; the image bytes are read only when they are sent.
async fn serve_logo(
    state: &AppState,
    org: Uuid,
    v: Option<&str>,
    request: &HeaderMap,
    scope: &str,
) -> Result<Response, AppError> {
    let meta = pdm_db::get_branding_asset_meta(&state.pool, org, LOGO)
        .await?
        .ok_or_else(|| not_found("no logo"))?;
    if etag_matches(request, &quoted(&meta.sha256)) {
        return logo_response(&meta.mime, &meta.sha256, Vec::new(), v, request, scope);
    }
    let asset: BrandingAsset = pdm_db::get_branding_asset(&state.pool, org, LOGO)
        .await?
        .ok_or_else(|| not_found("no logo"))?;
    logo_response(&asset.mime, &asset.sha256, asset.bytes, v, request, scope)
}

fn quoted(sha256: &str) -> String {
    format!("\"{sha256}\"")
}

/// Whether `If-None-Match` names `etag` (or is `*`); weak validators compare equal.
fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim())
        .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag)
}

fn header_value(v: &str) -> Result<HeaderValue, AppError> {
    HeaderValue::from_str(v).map_err(|e| AppError(PdmError::Internal(format!("header: {e}"))))
}

/// A response with `body`, or a 304 when the request's `If-None-Match` already names `etag`.
fn cached_response(
    request: &HeaderMap,
    etag: &str,
    content_type: &str,
    cache_control: &str,
    extra: &[(header::HeaderName, &str)],
    body: Vec<u8>,
) -> Result<Response, AppError> {
    let not_modified = etag_matches(request, etag);
    let mut response = if not_modified {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(body))
    };
    if not_modified {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    }
    let h = response.headers_mut();
    h.insert(header::ETAG, header_value(etag)?);
    h.insert(header::CACHE_CONTROL, header_value(cache_control)?);
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if !not_modified {
        h.insert(header::CONTENT_TYPE, header_value(content_type)?);
    }
    for (name, value) in extra {
        h.insert(name.clone(), header_value(value)?);
    }
    Ok(response)
}

/// A stored logo, served so that a browser can only ever show it as an image: its own
/// allow-listed type, `nosniff`, a sandbox CSP and same-origin only. A request whose `v` is the
/// current version may cache it for a year; others revalidate.
fn logo_response(
    mime: &str,
    sha256: &str,
    bytes: Vec<u8>,
    v: Option<&str>,
    request: &HeaderMap,
    scope: &str,
) -> Result<Response, AppError> {
    let ext = match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        other => {
            return Err(AppError(PdmError::Internal(format!(
                "stored logo has type {other}"
            ))));
        }
    };
    let cache = if v == Some(version_tag(sha256)) {
        format!("{scope}, max-age=31536000, immutable")
    } else if scope == "public" {
        "no-cache".to_string()
    } else {
        format!("{scope}, no-cache")
    };
    let disposition = format!("inline; filename=\"logo.{ext}\"");
    cached_response(
        request,
        &quoted(sha256),
        mime,
        &cache,
        &[
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox",
            ),
            (
                header::HeaderName::from_static("cross-origin-resource-policy"),
                "same-origin",
            ),
            (header::CONTENT_DISPOSITION, disposition.as_str()),
        ],
        bytes,
    )
}

/// What the sign-in page shows, or every field `null` when there is no sign-in organization.
/// Carries only what the admin chose to show: never the organization's id or registered name.
async fn public_view(state: &AppState) -> Result<(Value, branding::BrandingSettings), AppError> {
    let Some(org) = signin_org(state).await? else {
        let empty = json!({
            "display_name": null,
            "show_name_with_logo": null,
            "logo_url": null,
            "favicon_url": null,
        });
        return Ok((empty, branding::BrandingSettings::default()));
    };
    let settings = pdm_db::get_branding(&state.pool, org).await?;
    let logo_url = pdm_db::get_branding_asset_meta(&state.pool, org, LOGO)
        .await?
        .map(|m| format!("/public/branding/logo?v={}", version_tag(&m.sha256)));
    let favicon_url = logo_url.clone().filter(|_| settings.favicon_from_logo);
    let view = json!({
        "display_name": settings.display_name,
        "show_name_with_logo": settings.show_name_with_logo,
        "logo_url": logo_url,
        "favicon_url": favicon_url,
    });
    Ok((view, settings))
}

/// GET /public/branding — the sign-in organization's name and logo URLs (no sign-in).
async fn public_branding(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (view, _) = public_view(&state).await?;
    let body =
        serde_json::to_vec(&view).map_err(|e| AppError(PdmError::Internal(e.to_string())))?;
    let etag = format!("\"{}\"", sha256_hex(&body));
    cached_response(&headers, &etag, "application/json", "no-cache", &[], body)
}

/// GET /public/branding.css — the sign-in organization's color overrides (no sign-in); empty
/// when there is none. The portal links it after style.css.
async fn public_branding_css(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (_, settings) = public_view(&state).await?;
    let body = branding::css(&settings).into_bytes();
    let etag = format!("\"{}\"", sha256_hex(&body));
    cached_response(
        &headers,
        &etag,
        "text/css; charset=utf-8",
        "no-cache",
        &[],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_if_none_match(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            if let Ok(v) = HeaderValue::from_str(v) {
                h.append(header::IF_NONE_MATCH, v);
            }
        }
        h
    }

    #[test]
    fn if_none_match_handles_lists_weak_tags_and_star() {
        let etag = "\"abc\"";
        assert!(etag_matches(&with_if_none_match(&["\"abc\""]), etag));
        assert!(etag_matches(&with_if_none_match(&["W/\"abc\""]), etag));
        assert!(etag_matches(&with_if_none_match(&["\"x\", \"abc\""]), etag));
        assert!(etag_matches(
            &with_if_none_match(&["\"x\"", "\"abc\""]),
            etag
        ));
        assert!(etag_matches(&with_if_none_match(&["*"]), etag));
        assert!(!etag_matches(&with_if_none_match(&["\"abcd\""]), etag));
        assert!(!etag_matches(&with_if_none_match(&[]), etag));
    }

    #[test]
    fn content_type_drops_parameters_and_case() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("Image/PNG; charset=binary"),
        );
        assert_eq!(content_type(&h), "image/png");
        assert_eq!(content_type(&HeaderMap::new()), "");
    }

    fn asset(mime: &str) -> BrandingAsset {
        BrandingAsset {
            mime: mime.into(),
            sha256: "0123456789abcdef".repeat(4),
            width: 64,
            height: 32,
            size_bytes: 3,
            updated_at: chrono::Utc::now(),
            bytes: vec![1, 2, 3],
        }
    }

    fn respond(
        a: &BrandingAsset,
        v: Option<&str>,
        request: &HeaderMap,
        scope: &str,
    ) -> Result<Response, AppError> {
        logo_response(&a.mime, &a.sha256, a.bytes.clone(), v, request, scope)
    }

    fn header_of<'a>(r: &'a Response, name: &str) -> &'a str {
        r.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    #[test]
    fn logo_responses_carry_the_safety_headers_and_cache_by_version() -> Result<(), AppError> {
        let a = asset("image/png");
        let r = respond(&a, Some("0123456789ab"), &HeaderMap::new(), "public")?;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header_of(&r, "content-type"), "image/png");
        assert_eq!(header_of(&r, "x-content-type-options"), "nosniff");
        assert!(header_of(&r, "content-security-policy").contains("sandbox"));
        assert_eq!(header_of(&r, "cross-origin-resource-policy"), "same-origin");
        assert_eq!(
            header_of(&r, "content-disposition"),
            "inline; filename=\"logo.png\""
        );
        assert_eq!(header_of(&r, "etag"), format!("\"{}\"", a.sha256));
        assert_eq!(
            header_of(&r, "cache-control"),
            "public, max-age=31536000, immutable"
        );

        let r = respond(&a, Some("stale"), &HeaderMap::new(), "public")?;
        assert_eq!(header_of(&r, "cache-control"), "no-cache");
        let r = respond(&a, None, &HeaderMap::new(), "private")?;
        assert_eq!(header_of(&r, "cache-control"), "private, no-cache");
        let r = respond(&a, Some("0123456789ab"), &HeaderMap::new(), "private")?;
        assert_eq!(
            header_of(&r, "cache-control"),
            "private, max-age=31536000, immutable"
        );

        let etag = format!("\"{}\"", a.sha256);
        let r = respond(&a, None, &with_if_none_match(&[etag.as_str()]), "public")?;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);

        let r = respond(&asset("image/webp"), None, &HeaderMap::new(), "public")?;
        assert_eq!(
            header_of(&r, "content-disposition"),
            "inline; filename=\"logo.webp\""
        );
        assert!(respond(&asset("image/svg+xml"), None, &HeaderMap::new(), "public").is_err());
        Ok(())
    }

    #[test]
    fn version_tags_are_twelve_hex_digits() {
        assert_eq!(version_tag(&"ab".repeat(32)), "abababababab");
        assert_eq!(version_tag("short"), "short");
    }
}
