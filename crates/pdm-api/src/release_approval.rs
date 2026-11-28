//! Org setting: releases need an approval first.
//!
//! With `require_approval_on_release` on, `POST /items/{id}/release` opens an approval
//! request and puts the item in review; the revision is made when the request is
//! approved. Any member can read the setting; only admins change it.

use axum::Json;
use axum::extract::{Extension, State};
use serde::Deserialize;

use crate::auth::Authed;
use crate::error::AppError;
use crate::routes::{require_admin, require_org};
use crate::state::AppState;

/// The key in the organization's `settings` that the release route reads.
pub(crate) const KEY: &str = "require_approval_on_release";

#[derive(Deserialize)]
pub struct ReleaseApprovalReq {
    pub require_approval_on_release: bool,
}

fn answer(on: bool) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "require_approval_on_release": on }))
}

/// GET /settings/release-approval — whether a release must be approved first (default off).
pub async fn get_release_approval(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
) -> Result<Json<serde_json::Value>, AppError> {
    let org = require_org(&authed);
    Ok(answer(
        pdm_db::org_setting_bool(&state.pool, org, KEY).await?,
    ))
}

/// PUT /settings/release-approval — turn the approval step for releases on or off (admin).
/// Answers with the value the release route will now read.
pub async fn put_release_approval(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
    Json(body): Json<ReleaseApprovalReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&authed)?;
    let org = require_org(&authed);
    pdm_db::set_org_setting_bool(&state.pool, org, KEY, body.require_approval_on_release).await?;
    Ok(answer(
        pdm_db::org_setting_bool(&state.pool, org, KEY).await?,
    ))
}
