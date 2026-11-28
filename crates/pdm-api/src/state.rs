use std::sync::Arc;
use std::time::Instant;

use pdm_storage::BlobStore;
use pdm_update::UpdateState;
use sqlx::PgPool;

use crate::design_link::DesignLinks;
use crate::events::EventHub;
use crate::session::SessionLimits;

/// Shared application state, cloned into each handler.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub store: Arc<BlobStore>,
    /// Maximum request body size in bytes (uploads).
    pub max_upload: u64,
    /// Lock lease length in hours.
    pub lease_hours: i64,
    /// When this process started, for uptime reporting.
    pub started_at: Instant,
    /// Latest-release check (refreshed at most once a day by the boot task).
    pub update: Arc<tokio::sync::RwLock<UpdateState>>,
    /// Signs and checks the short-lived `/public/design/...` links.
    pub design_links: DesignLinks,
    /// How long a web sign-in lasts.
    pub sessions: SessionLimits,
    /// Passes live change hints to `GET /events/stream` (see `events::spawn_listener`).
    pub events: EventHub,
}
