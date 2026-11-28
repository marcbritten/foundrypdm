//! Live updates: `GET /events/stream` tells an organization's open pages, as it happens, that
//! something was recorded in the audit log, so they can refresh what changed.
//!
//! `pdm_db::record_audit` sends each new row's [`AuditHint`] on the Postgres channel
//! [`pdm_db::AUDIT_CHANNEL`], whichever process wrote it (this one, or a separate worker started
//! with `PDM_WORKER=0` elsewhere). One listener task per API process ([`spawn_listener`]) hands the
//! hints to a broadcast channel, and each stream passes on its own organization's. Postgres
//! delivers a notification only once the row is committed, so a page that refetches on a hint
//! sees the change.
//!
//! The stream is an ordinary authenticated request: the portal reads it with `fetch()` and the
//! bearer header, so no token goes into a URL. It sends, as Server-Sent Events:
//! - with `?since=<audit id>` (a page reconnecting), first a `change` for each row recorded after
//!   that id, the same rows `GET /audit?since=` lists, or `reset` when there are too many to go
//!   through one by one;
//! - `ready`, with `last_id`, the newest audit id: the cursor for the next `?since=`;
//! - `change` for each hint (`id:` is the audit id);
//! - `resync` when hints may have been missed (the listener reconnected, or this stream fell
//!   behind): reconnect with `?since=` to catch up;
//! - a comment every [`HEARTBEAT`], which keeps proxies from closing an idle connection;
//! - `expired` just before it closes because the sign-in ended (signed out, expired, user
//!   deactivated). The heartbeat re-checks this. The stream itself does not count as use, so an
//!   open page does not keep an idle sign-in alive.
use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;
use pdm_db::AuditHint;
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::time::{Instant, Interval, MissedTickBehavior};
use uuid::Uuid;

use crate::auth::Authed;
use crate::error::AppError;
use crate::session::SessionLimits;
use crate::state::AppState;

/// How often an idle stream sends a comment and re-checks the sign-in. Below the usual 60 s
/// proxy read timeout.
pub const HEARTBEAT: Duration = Duration::from_secs(25);

/// Hints kept for streams that have not read them yet; one that falls further behind is told to
/// resync instead.
const CAPACITY: usize = 1024;

/// At most this many missed changes are replayed to a reconnecting page; past that it is told to
/// refresh everything (`reset`).
const REPLAY_MAX: u32 = 200;

/// What the listener passes to the streams.
#[derive(Clone, Debug)]
pub enum Message {
    Hint(Arc<AuditHint>),
    /// Hints may have been lost (the listener's connection dropped).
    Resync,
}

/// The fan-out from the one listener to every open stream in this process. Cloning is cheap.
#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<Message>,
    /// Whether the listener is connected: while it is not, no hints arrive, so streams are
    /// refused and pages keep their timed refresh.
    listening: Arc<AtomicBool>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::with_capacity(CAPACITY)
    }
}

impl EventHub {
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            listening: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn listening(&self) -> bool {
        self.listening.load(Ordering::Relaxed)
    }

    fn set_listening(&self, on: bool) {
        self.listening.store(on, Ordering::Relaxed);
    }

    /// Pass a message to every open stream (none open is fine).
    pub fn publish(&self, message: Message) {
        let _ = self.tx.send(message);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Message> {
        self.tx.subscribe()
    }
}

/// Listen for audit hints for as long as the process runs, reconnecting with a growing pause
/// (1 s up to 30 s) when the database connection is lost. Holds one pooled connection.
pub fn spawn_listener(pool: PgPool, hub: EventHub) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut pause = Duration::from_secs(1);
        loop {
            let result = listen_until_lost(&pool, &hub).await;
            hub.set_listening(false);
            match result {
                Ok(()) => pause = Duration::from_secs(1),
                Err(e) => tracing::warn!(error = %e, "live updates: listener failed; retrying"),
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(30));
        }
    })
}

/// One listener connection: returns when it is lost.
async fn listen_until_lost(pool: &PgPool, hub: &EventHub) -> Result<(), sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(pdm_db::AUDIT_CHANNEL).await?;
    tracing::info!("live updates: listening for audit hints");
    hub.set_listening(true);
    // Anything recorded before this point (while the listener was away, or a stream that opened
    // before it first connected) was not passed on.
    hub.publish(Message::Resync);
    loop {
        // `None`: the connection was lost. Start over on a fresh one, then send a resync.
        let Some(n) = listener.try_recv().await? else {
            tracing::warn!("live updates: listener connection lost; reconnecting");
            return Ok(());
        };
        match serde_json::from_str::<AuditHint>(n.payload()) {
            Ok(hint) => hub.publish(Message::Hint(Arc::new(hint))),
            Err(e) => tracing::warn!(error = %e, "live updates: unreadable audit hint"),
        }
    }
}

/// What a stream does with one broadcast result.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Change(Arc<AuditHint>),
    Resync,
    /// Another organization's hint.
    Skip,
    /// The process is shutting down.
    Close,
}

fn step(received: Result<Message, RecvError>, org_id: Uuid) -> Step {
    match received {
        Ok(Message::Hint(h)) if h.org_id == org_id => Step::Change(h),
        Ok(Message::Hint(_)) => Step::Skip,
        // Fell behind the broadcast buffer: some hints are gone, so catch up instead.
        Ok(Message::Resync) | Err(RecvError::Lagged(_)) => Step::Resync,
        Err(RecvError::Closed) => Step::Close,
    }
}

fn change_event(hint: &AuditHint) -> Event {
    let data = serde_json::json!({
        "audit_id": hint.audit_id,
        "actor_id": hint.actor_id,
        "action": hint.action,
        "object_type": hint.object_type,
        "object_id": hint.object_id,
        "item_id": hint.item_id,
        "nest_id": hint.nest_id,
    });
    Event::default()
        .event("change")
        .id(hint.audit_id.to_string())
        .data(data.to_string())
}

struct Feed {
    pool: PgPool,
    rx: broadcast::Receiver<Message>,
    org_id: Uuid,
    token_id: Uuid,
    limits: SessionLimits,
    heartbeat: Interval,
    /// Sent before anything live: the replayed changes, then `ready`.
    queue: VecDeque<Event>,
    /// Replayed audit ids, so a hint for one of them that arrives live is not sent twice.
    replayed: HashSet<i64>,
    ended: bool,
}

async fn next_event(mut f: Feed) -> Option<(Result<Event, Infallible>, Feed)> {
    if f.ended {
        return None;
    }
    if let Some(ev) = f.queue.pop_front() {
        return Some((Ok(ev), f));
    }
    loop {
        tokio::select! {
            received = f.rx.recv() => match step(received, f.org_id) {
                Step::Change(h) if f.replayed.contains(&h.audit_id) => continue,
                Step::Change(h) => return Some((Ok(change_event(&h)), f)),
                Step::Resync => return Some((Ok(Event::default().event("resync").data("{}")), f)),
                Step::Skip => continue,
                Step::Close => return None,
            },
            _ = f.heartbeat.tick() => {
                let still = pdm_db::token_still_valid(
                    &f.pool,
                    f.token_id,
                    f.limits.idle_minutes,
                    f.limits.lifetime_minutes,
                )
                .await;
                // A database hiccup keeps the stream: the page's own requests will tell.
                if let Ok(false) = still {
                    f.ended = true;
                    return Some((Ok(Event::default().event("expired").data("{}")), f));
                }
                return Some((Ok(Event::default().comment("ping")), f));
            }
        }
    }
}

/// `?since=<audit id>`: also send what was recorded after that id (a page reconnecting).
#[derive(Deserialize)]
pub struct StreamQuery {
    pub since: Option<i64>,
}

/// GET /events/stream — this organization's live change hints (Server-Sent Events).
pub async fn stream(
    State(state): State<AppState>,
    Extension(authed): Extension<Authed>,
    Query(q): Query<StreamQuery>,
) -> Result<Response, AppError> {
    if !state.events.listening() {
        // Nothing would arrive: say so, and the page keeps its timed refresh and tries again.
        let body = serde_json::json!({ "error": {
            "code": "live_unavailable",
            "message": "live updates are not available right now",
        }});
        return Ok((StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response());
    }
    // Subscribe before reading the database, so no hint falls in between.
    let rx = state.events.subscribe();
    let last_id = pdm_db::last_audit_id(&state.pool, authed.org_id).await?;
    let mut queue = VecDeque::new();
    let mut replayed = HashSet::new();
    if let Some(since) = q.since {
        let missed =
            pdm_db::list_audit_since(&state.pool, authed.org_id, since, REPLAY_MAX).await?;
        if missed.len() >= REPLAY_MAX as usize {
            // Too much to go through one by one: the page refreshes everything instead.
            queue.push_back(Event::default().event("reset").data("{}"));
        } else {
            for ev in &missed {
                replayed.insert(ev.id);
                queue.push_back(change_event(&AuditHint::of(ev)));
            }
        }
    }
    queue.push_back(
        Event::default()
            .event("ready")
            .data(serde_json::json!({ "last_id": last_id }).to_string()),
    );
    let mut heartbeat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let feed = Feed {
        pool: state.pool.clone(),
        rx,
        org_id: authed.org_id,
        token_id: authed.token_id,
        limits: state.sessions,
        heartbeat,
        queue,
        replayed,
        ended: false,
    };
    let mut response = Sse::new(stream::unfold(feed, next_event)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    // Nginx (and proxies that follow it) pass the stream through instead of buffering it.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pdm_core::AuditEvent;

    fn hint(org_id: Uuid, audit_id: i64) -> AuditHint {
        AuditHint {
            audit_id,
            org_id,
            actor_id: None,
            action: "item.updated".into(),
            object_type: "item".into(),
            object_id: Some(Uuid::new_v4()),
            item_id: None,
            nest_id: None,
        }
    }

    #[test]
    fn a_stream_passes_on_only_its_own_organizations_hints() {
        let (mine, other) = (Uuid::new_v4(), Uuid::new_v4());
        let h = Arc::new(hint(mine, 7));
        assert_eq!(
            step(Ok(Message::Hint(h.clone())), mine),
            Step::Change(h.clone())
        );
        assert_eq!(step(Ok(Message::Hint(h)), other), Step::Skip);
    }

    #[test]
    fn a_hub_refuses_streams_until_its_listener_connects() {
        let hub = EventHub::default();
        assert!(!hub.listening());
        hub.clone().set_listening(true);
        assert!(hub.listening(), "clones share the flag");
    }

    #[test]
    fn missed_hints_ask_for_a_resync() {
        let org = Uuid::new_v4();
        assert_eq!(step(Ok(Message::Resync), org), Step::Resync);
        assert_eq!(step(Err(RecvError::Lagged(3)), org), Step::Resync);
        assert_eq!(step(Err(RecvError::Closed), org), Step::Close);
    }

    #[tokio::test]
    async fn a_stream_that_falls_behind_is_told_to_resync() {
        let hub = EventHub::with_capacity(2);
        let mut rx = hub.subscribe();
        let org = Uuid::new_v4();
        for id in 1..=4 {
            hub.publish(Message::Hint(Arc::new(hint(org, id))));
        }
        assert_eq!(step(rx.recv().await, org), Step::Resync);
        // Then the hints still in the buffer arrive.
        assert!(matches!(step(rx.recv().await, org), Step::Change(h) if h.audit_id == 3));
    }

    #[test]
    fn a_hint_names_the_item_or_nest_from_the_detail_and_round_trips()
    -> Result<(), serde_json::Error> {
        let (org, job, item) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let ev = AuditEvent {
            id: 42,
            org_id: org,
            actor_id: None,
            agent_id: String::new(),
            action: "job.finished".into(),
            object_type: "job".into(),
            object_id: Some(job),
            detail: Some(serde_json::json!({
                "kind": "preview.render", "item_id": item.to_string(), "nest_id": "not-a-uuid"
            })),
            at: Utc::now(),
        };
        let h = AuditHint::of(&ev);
        assert_eq!((h.audit_id, h.org_id, h.object_id), (42, org, Some(job)));
        assert_eq!((h.item_id, h.nest_id), (Some(item), None));
        let text = serde_json::to_string(&h)?;
        assert!(!text.contains("preview.render"), "no detail in the hint");
        assert_eq!(serde_json::from_str::<AuditHint>(&text)?, h);
        Ok(())
    }
}
