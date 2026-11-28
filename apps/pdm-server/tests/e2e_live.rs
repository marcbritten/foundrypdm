//! End-to-end checks for live updates (`GET /events/stream`).
//!
//! Run against a server with:
//!   PDM_E2E=1
//!   PDM_E2E_BASE              (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN       an admin's API token (`pdm-cli create-token`)
//!   PDM_E2E_OTHER_ORG_TOKEN   optional: a token from a second organization, to check that its
//!                             stream sees none of the first organization's changes
//!   cargo test -p pdm-server --test e2e_live -- --ignored
//!
//! The worker check needs a worker on the same database: the server's own (the default) or a
//! separate process (`PDM_WORKER=0` on the server). The sign-out check waits for one heartbeat.

use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}

fn enabled() -> bool {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return false;
    }
    true
}

/// One Server-Sent Event.
#[derive(Debug)]
struct Sse {
    event: String,
    data: Value,
}

/// Reads events off an open stream.
struct Events {
    resp: Response,
    buf: String,
}

impl Events {
    async fn open(client: &Client, token: &str, since: Option<i64>) -> Events {
        let url = match since {
            Some(s) => format!("{}/events/stream?since={s}", base()),
            None => format!("{}/events/stream", base()),
        };
        let resp = client
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .expect("GET /events/stream");
        assert_eq!(resp.status(), StatusCode::OK, "stream status");
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            ctype.starts_with("text/event-stream"),
            "content type {ctype}"
        );
        assert_eq!(
            resp.headers()
                .get("x-accel-buffering")
                .and_then(|v| v.to_str().ok()),
            Some("no"),
            "proxies are asked not to buffer"
        );
        Events {
            resp,
            buf: String::new(),
        }
    }

    /// The next event within `limit`; `None` on timeout or when the stream ends.
    async fn next(&mut self, limit: Duration) -> Option<Sse> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..i + 2).collect();
                let (mut event, mut data) = ("message".to_string(), String::new());
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if block.lines().all(|l| l.is_empty() || l.starts_with(':')) {
                    continue; // heartbeat comment
                }
                let data = serde_json::from_str(&data).unwrap_or(Value::Null);
                return Some(Sse { event, data });
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            match tokio::time::timeout(left, self.resp.chunk()).await {
                Ok(Ok(Some(bytes))) => self.buf.push_str(&String::from_utf8_lossy(&bytes)),
                _ => return None,
            }
        }
    }

    /// Skip events until one matches, within `limit`.
    async fn until(&mut self, limit: Duration, want: impl Fn(&Sse) -> bool) -> Option<Sse> {
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let ev = self.next(left).await?;
            if want(&ev) {
                return Some(ev);
            }
        }
    }

    async fn ready(&mut self) -> i64 {
        let ev = self
            .until(Duration::from_secs(5), |e| e.event == "ready")
            .await
            .expect("ready event at once");
        ev.data["last_id"].as_i64().expect("last_id")
    }
}

async fn post(client: &Client, token: &str, path: &str, body: Value) -> Value {
    let r = client
        .post(format!("{}{path}", base()))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .expect("POST");
    assert!(r.status().is_success(), "POST {path}: {}", r.status());
    r.json().await.expect("JSON")
}

async fn new_item(client: &Client) -> String {
    let token = admin_token();
    let tag = uuid::Uuid::new_v4();
    let p = post(
        client,
        &token,
        "/projects",
        json!({ "name": format!("live-{tag}") }),
    )
    .await;
    let pid = p["project"]["id"].as_str().expect("project id");
    let i = post(
        client,
        &token,
        &format!("/projects/{pid}/items"),
        json!({ "description": "live updates e2e" }),
    )
    .await;
    i["item"]["id"].as_str().expect("item id").to_string()
}

fn is_change_of(ev: &Sse, object_id: &str) -> bool {
    ev.event == "change" && ev.data["object_id"].as_str() == Some(object_id)
}

#[tokio::test]
#[ignore = "needs a running server (PDM_E2E=1)"]
async fn a_stream_needs_a_token() {
    if !enabled() {
        return;
    }
    let r = Client::new()
        .get(format!("{}/events/stream", base()))
        .send()
        .await
        .expect("GET");
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs a running server (PDM_E2E=1)"]
async fn a_change_arrives_live_and_is_replayed_after_a_reconnect() {
    if !enabled() {
        return;
    }
    let client = Client::new();
    let mut live = Events::open(&client, &admin_token(), None).await;
    let before = live.ready().await;
    let other_token = std::env::var("PDM_E2E_OTHER_ORG_TOKEN").ok();
    let mut other = match &other_token {
        Some(t) => {
            let mut s = Events::open(&client, t, None).await;
            s.ready().await;
            Some(s)
        }
        None => {
            eprintln!("PDM_E2E_OTHER_ORG_TOKEN not set; not checking other organizations");
            None
        }
    };

    let started = Instant::now();
    let item = new_item(&client).await;
    let ev = live
        .until(Duration::from_secs(5), |e| is_change_of(e, &item))
        .await
        .expect("the change arrives on the stream");
    let took = started.elapsed();
    eprintln!("item.created arrived after {took:?}");
    assert!(took < Duration::from_secs(2), "arrived in {took:?}");
    assert_eq!(ev.data["action"], "item.created");
    assert_eq!(ev.data["object_type"], "item");
    assert!(ev.data.get("detail").is_none(), "hints carry no detail");
    let audit_id = ev.data["audit_id"].as_i64().expect("audit_id");
    assert!(audit_id > before);

    if let Some(s) = other.as_mut() {
        let leaked = s
            .until(Duration::from_secs(3), |e| is_change_of(e, &item))
            .await;
        assert!(
            leaked.is_none(),
            "another organization saw the change: {leaked:?}"
        );
    }

    // A page that reconnects gets what it missed, then `ready`.
    let mut again = Events::open(&client, &admin_token(), Some(audit_id - 1)).await;
    let first = again
        .next(Duration::from_secs(5))
        .await
        .expect("a replayed event");
    assert!(is_change_of(&first, &item), "replayed first, got {first:?}");
    assert!(again.ready().await >= audit_id);

    // The same rows through GET /audit?since=.
    let r = client
        .get(format!("{}/audit?since={}", base(), audit_id - 1))
        .bearer_auth(admin_token())
        .send()
        .await
        .expect("GET /audit?since");
    assert!(r.status().is_success());
    let body: Value = r.json().await.expect("JSON");
    assert_eq!(body["events"][0]["id"].as_i64(), Some(audit_id));
    assert!(body["last_id"].as_i64().unwrap_or(0) >= audit_id);
}

#[tokio::test]
#[ignore = "needs a running server and a worker (PDM_E2E=1)"]
async fn background_jobs_report_on_the_stream() {
    if !enabled() {
        return;
    }
    let client = Client::new();
    let token = admin_token();
    let mut live = Events::open(&client, &token, None).await;
    live.ready().await;
    let item = new_item(&client).await;
    // A check-in queues the analysis and preview jobs for the new version.
    let design =
        br#"{"format":"solvecraft/5","name":"live","units":"mm","params":[],"features":[]}"#;
    let sha = hex::encode(Sha256::digest(design));
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&token)
        .header("Content-Type", "application/octet-stream")
        .body(design.to_vec())
        .send()
        .await
        .expect("PUT /blobs");
    assert!(r.status().is_success(), "PUT /blobs: {}", r.status());
    post(
        &client,
        &token,
        &format!("/items/{item}/checkout"),
        json!({ "workstation": "e2e-live" }),
    )
    .await;
    post(
        &client,
        &token,
        &format!("/items/{item}/checkin"),
        json!({ "sha256": sha, "filename": "live.solvecraft", "base_version": null }),
    )
    .await;
    // Whatever the job's outcome (a server without the CAD tools fails it), the worker reports.
    let ev = live
        .until(Duration::from_secs(30), |e| {
            e.event == "change"
                && e.data["object_type"] == "job"
                && e.data["item_id"].as_str() == Some(item.as_str())
                && e.data["action"] != "job.started"
        })
        .await
        .expect("a job for the item finishes or fails on the stream");
    eprintln!("worker event: {}", ev.data["action"]);
    assert!(ev.data["actor_id"].is_null());
}

#[tokio::test]
#[ignore = "needs a running server (PDM_E2E=1); waits for one heartbeat"]
async fn signing_out_ends_the_stream() {
    if !enabled() {
        return;
    }
    let client = Client::new();
    let email = format!("live-{}@example.test", uuid::Uuid::new_v4());
    let password = format!("pw-{}", uuid::Uuid::new_v4());
    post(
        &client,
        &admin_token(),
        "/users",
        json!({ "email": email, "display_name": "Live", "role": "viewer", "password": password }),
    )
    .await;
    let r = client
        .post(format!("{}/auth/login", base()))
        .json(&json!({ "username": email, "password": password }))
        .send()
        .await
        .expect("login");
    assert!(r.status().is_success(), "login: {}", r.status());
    let session = r.json::<Value>().await.expect("JSON")["token"]
        .as_str()
        .expect("token")
        .to_string();
    let mut live = Events::open(&client, &session, None).await;
    live.ready().await;
    post(&client, &session, "/auth/logout", json!({})).await;
    let ev = live
        .until(Duration::from_secs(40), |e| e.event == "expired")
        .await
        .expect("the stream says the sign-in ended");
    assert_eq!(ev.event, "expired");
    assert!(
        live.next(Duration::from_secs(5)).await.is_none(),
        "and then closes"
    );
}
