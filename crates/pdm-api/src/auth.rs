//! Bearer-token authentication middleware.
//!
//! Every non-public request must carry `Authorization: Bearer <token>`. The token is
//! SHA-256-hashed and looked up in `api_tokens`; the owning user and organization are
//! resolved and injected as an `Authed` extension for the handler.
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use pdm_core::User;

use crate::error::AppError;
use crate::state::AppState;

/// Authenticated caller, injected into request extensions by [`auth_middleware`].
#[derive(Clone)]
pub struct Authed {
    pub org_id: Uuid,
    pub user: Option<User>,
    pub token_id: Uuid,
    pub scopes: String,
}

impl Authed {
    pub fn user_id(&self) -> Option<Uuid> {
        self.user.as_ref().map(|u| u.id)
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// Public paths that skip authentication.
fn is_public(path: &str) -> bool {
    path == "/healthz"
        || path == "/readyz"
        || path == "/version"
        || path == "/"
        || path == "/auth/login"
        || path.starts_with("/static/")
        // The bundled SolidCraft web editor's design-link fetch sends no token; the link
        // carries an expiry and a signature instead (see public_design_download).
        || path.starts_with("/public/design/")
        // The sign-in page's branding (see branding.rs): exact paths only.
        || path == "/public/branding"
        || path == "/public/branding.css"
        || path == "/public/branding/logo"
}

/// Axum middleware: validate the Bearer token, inject `Authed`.
pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = req.uri().path().to_string();
    if is_public(&path) {
        return Ok(next.run(req).await);
    }

    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(header) = header else {
        return Err(AppError(pdm_core::PdmError::AuthFailed));
    };
    let Some(token) = header.strip_prefix("Bearer ") else {
        return Err(AppError(pdm_core::PdmError::AuthFailed));
    };

    let hash = sha256_hex(token.as_bytes());
    let Some((tok, user)) = pdm_db::get_token_by_hash(&state.pool, &hash).await? else {
        return Err(AppError(pdm_core::PdmError::AuthFailed));
    };
    if tok.expires_at.is_some_and(|exp| exp < Utc::now()) {
        return Err(AppError(pdm_core::PdmError::AuthFailed));
    }
    // A web sign-in ends when idle too long or at its lifetime; API tokens never do. An ended
    // sign-in is deleted when it is presented, so it stays refused even if the limits are raised
    // later. A failed check refuses the request, and so does a token deleted since the lookup.
    let limits = state.sessions;
    if limits.any() {
        let ended = pdm_db::session_expired(
            &state.pool,
            tok.id,
            limits.idle_minutes,
            limits.lifetime_minutes,
        )
        .await?;
        match ended {
            None => return Err(AppError(pdm_core::PdmError::AuthFailed)),
            Some(true) => {
                pdm_db::delete_token(&state.pool, tok.id).await?;
                return Err(AppError(pdm_core::PdmError::SessionExpired));
            }
            Some(false) => {}
        }
    }
    let user = match user {
        Some(u) => {
            if !u.active {
                return Err(AppError(pdm_core::PdmError::Forbidden(
                    "user is inactive".into(),
                )));
            }
            Some(u)
        }
        None => None,
    };

    // Fire-and-forget last-used update; do not fail the request on it. Opening the live-updates
    // stream is not use: a page left open must not keep an idle sign-in alive by reconnecting.
    if path != "/events/stream" {
        let pool = state.pool.clone();
        let id = tok.id;
        tokio::spawn(async move {
            let _ = pdm_db::touch_token(&pool, id).await;
        });
    }

    let authed = Authed {
        org_id: tok.org_id,
        user,
        token_id: tok.id,
        scopes: tok.scopes,
    };
    let mut req = req;
    req.extensions_mut().insert(authed);
    Ok(next.run(req).await)
}

/// Return the authenticated user or a forbidden error (for endpoints that need a real user).
pub fn require_user(authed: &Authed) -> Result<&User, AppError> {
    authed.user.as_ref().ok_or_else(|| {
        AppError(pdm_core::PdmError::Forbidden(
            "a user identity is required".into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::is_public;

    #[test]
    fn public_paths_are_exact() {
        for path in [
            "/healthz",
            "/readyz",
            "/version",
            "/",
            "/auth/login",
            "/static/app.js",
            "/public/design/v/f.solvecraft",
            "/public/branding",
            "/public/branding.css",
            "/public/branding/logo",
        ] {
            assert!(is_public(path), "{path} should be public");
        }
        for path in [
            "/settings/branding",
            "/settings/branding/logo",
            "/public/brandingx",
            "/public/branding/logo/x",
            "/public/branding.css/x",
            "/public/",
            "/public",
            "/me",
            "/auth/logout",
        ] {
            assert!(!is_public(path), "{path} should need a token");
        }
    }
}
