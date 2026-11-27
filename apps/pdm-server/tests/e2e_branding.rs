//! End-to-end test of organization branding (`/settings/branding*`, `/public/branding*`).
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE   (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN   an admin of the organization under test
//! Optional:
//!   PDM_E2E_EXPECT_SIGNIN_ORG=1|0   whether the server's sign-in page is expected to show this
//!   organization (single-organization server, or `PDM_SIGNIN_ORG` naming it) or not.
//!
//! Works on a single- or multi-organization server: the public routes must show this run's
//! name, color and logo exactly when `signin_org` is true, and never otherwise. Leaves the
//! organization's branding reset; adds one viewer account per run.

use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}

fn url(path: &str) -> String {
    format!("{}{path}", base())
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// A real PNG of the given size (RGBA, a simple gradient). `seed` colors the first pixel, so
/// each run uploads files no earlier run did.
fn png(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        let mut data: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 251) as u8, (i % 7) as u8 * 30, 200, 255])
            .collect();
        data[..3].copy_from_slice(&seed.to_le_bytes()[..3]);
        writer.write_image_data(&data).expect("png data");
    }
    out
}

/// A WebP header in the VP8X form (the server checks the header only).
fn webp_vp8x(w: u32, h: u32) -> Vec<u8> {
    let (w, h) = ((w - 1).to_le_bytes(), (h - 1).to_le_bytes());
    let mut b = b"RIFF\x00\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00\x10\x00\x00\x00".to_vec();
    b.extend_from_slice(&[w[0], w[1], w[2], h[0], h[1], h[2]]);
    b.resize(64, 0);
    b
}

fn header(r: &Response, name: &str) -> String {
    r.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

async fn json_of(r: Response, what: &str) -> Value {
    let status = r.status();
    let text = r.text().await.expect("body");
    assert!(status.is_success(), "{what}: {status} {text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {text}"))
}

async fn get(client: &Client, token: Option<&str>, path: &str) -> Response {
    let mut req = client.get(url(path));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    req.send().await.expect(path)
}

async fn put_json(client: &Client, token: &str, body: &Value) -> Response {
    client
        .put(url("/settings/branding"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .expect("PUT /settings/branding")
}

async fn put_logo(client: &Client, token: &str, mime: Option<&str>, body: Vec<u8>) -> Response {
    let mut req = client
        .put(url("/settings/branding/logo"))
        .bearer_auth(token)
        .body(body);
    if let Some(m) = mime {
        req = req.header("content-type", m);
    }
    req.send().await.expect("PUT /settings/branding/logo")
}

async fn delete(client: &Client, token: &str, path: &str) -> Response {
    client
        .delete(url(path))
        .bearer_auth(token)
        .send()
        .await
        .expect(path)
}

/// A refused branding update: 400 with a message naming `key`.
async fn assert_refused(client: &Client, token: &str, body: Value, key: &str) {
    let r = put_json(client, token, &body).await;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {text}");
    assert!(text.contains(key), "{body}: {text} does not name {key}");
}

async fn create_viewer(client: &Client, run: &str) -> String {
    let r = client
        .post(url("/users"))
        .bearer_auth(admin_token())
        .json(&json!({
            "email": format!("branding-viewer-{run}@e2e.test"),
            "display_name": "Branding viewer",
            "role": "viewer",
        }))
        .send()
        .await
        .expect("POST /users");
    let user = json_of(r, "create viewer").await;
    let id = user["user"]["id"].as_str().expect("user id").to_string();
    let r = client
        .post(url(&format!("/users/{id}/tokens")))
        .bearer_auth(admin_token())
        .json(&json!({ "name": "e2e-branding" }))
        .send()
        .await
        .expect("POST /users/{id}/tokens");
    json_of(r, "viewer token").await["token"]
        .as_str()
        .expect("token")
        .to_string()
}

#[tokio::test]
#[ignore]
async fn e2e_branding() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let admin = admin_token();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let run = format!("{}-{}", std::process::id(), nanos % 1_000_000_000);
    // This run's markers: they may appear on the public routes only for the sign-in org.
    let name = format!("E2E Brand {run}");
    let color = format!("#{:06x}", (nanos % 0xff_ffff) as u32 | 0x10_0000);
    let seed = (nanos % 0x100_0000) as u32;

    assert_eq!(get(&client, None, "/healthz").await.status(), 200);
    let me = json_of(get(&client, Some(&admin), "/me").await, "GET /me").await;
    let org_id = me["org_id"].as_str().expect("org_id").to_string();

    // ── Start from the defaults ──
    let view = json_of(delete(&client, &admin, "/settings/branding").await, "reset").await;
    assert_eq!(view["display_name"], Value::Null);
    assert_eq!(view["show_name_with_logo"], true);
    assert_eq!(view["favicon_from_logo"], false);
    assert_eq!(view["logo"], Value::Null);
    assert_eq!(view["colors"], json!({}));
    assert_eq!(
        view["themes"],
        json!(["dark", "light", "slate", "contrast"])
    );
    let tokens = view["tokens"].as_array().expect("tokens");
    for t in ["bg", "panel", "fg", "acc", "on-acc"] {
        assert!(tokens.contains(&json!(t)), "tokens lack {t}");
    }
    assert!(!tokens.contains(&json!("shadow")));
    let signin = view["signin_org"].as_bool().expect("signin_org");
    eprintln!("signin_org = {signin}");
    if let Ok(expect) = std::env::var("PDM_E2E_EXPECT_SIGNIN_ORG") {
        assert_eq!(signin, expect == "1", "signin_org");
    }

    // ── Who may do what ──
    let viewer = create_viewer(&client, &run).await;
    let r = get(&client, Some(&viewer), "/settings/branding").await;
    assert_eq!(r.status(), 200, "a viewer can read the branding");
    assert_eq!(
        put_json(&client, &viewer, &json!({ "display_name": "x" }))
            .await
            .status(),
        403
    );
    assert_eq!(
        delete(&client, &viewer, "/settings/branding")
            .await
            .status(),
        403
    );
    assert_eq!(
        put_logo(&client, &viewer, Some("image/png"), png(64, 32, seed))
            .await
            .status(),
        403
    );
    assert_eq!(
        delete(&client, &viewer, "/settings/branding/logo")
            .await
            .status(),
        403
    );
    assert_eq!(get(&client, None, "/settings/branding").await.status(), 401);

    // ── Name and colors ──
    let r = put_json(
        &client,
        &admin,
        &json!({
            "display_name": format!("  {name}  "),
            "colors": {
                "dark": { "acc": color.to_uppercase(), "on-acc": "#000000" },
                "contrast": { "fg": "#FFFFFF" }
            }
        }),
    )
    .await;
    let view = json_of(r, "PUT colors").await;
    assert_eq!(view["display_name"], name.as_str());
    assert_eq!(view["colors"]["dark"]["acc"], color.as_str());
    assert_eq!(view["colors"]["dark"]["on-acc"], "#000000");
    assert_eq!(view["colors"]["contrast"]["fg"], "#ffffff");

    // An absent field keeps its value; a null token drops one override.
    let r = put_json(
        &client,
        &admin,
        &json!({ "show_name_with_logo": false, "colors": { "contrast": { "fg": null } } }),
    )
    .await;
    let view = json_of(r, "PUT partial").await;
    assert_eq!(view["display_name"], name.as_str());
    assert_eq!(view["show_name_with_logo"], false);
    assert_eq!(view["colors"]["dark"]["acc"], color.as_str());
    assert!(view["colors"].get("contrast").is_none());

    for (body, key) in [
        (
            json!({ "colors": { "sepia": { "acc": "#000000" } } }),
            "colors.sepia",
        ),
        (
            json!({ "colors": { "dark": { "shadow": "#000000" } } }),
            "colors.dark.shadow",
        ),
        (
            json!({ "colors": { "dark": { "acc": "red" } } }),
            "colors.dark.acc",
        ),
        (
            json!({ "colors": { "dark": { "acc": "#fff" } } }),
            "colors.dark.acc",
        ),
        (
            json!({ "colors": { "dark": { "acc": "#3b82f6ff" } } }),
            "colors.dark.acc",
        ),
        (
            json!({ "colors": { "dark": { "acc": "#3b82f6;}body{display:none" } } }),
            "colors.dark.acc",
        ),
        (
            json!({ "colors": { "dark": { "acc": "url(x)" } } }),
            "colors.dark.acc",
        ),
        (json!({ "display_name": "x".repeat(61) }), "display_name"),
        (json!({ "display_name": "Bell\u{0007}" }), "display_name"),
        (json!({ "logo_url": "/x" }), "unknown field"),
    ] {
        assert_refused(&client, &admin, body, key).await;
    }
    let view = json_of(
        get(&client, Some(&admin), "/settings/branding").await,
        "GET",
    )
    .await;
    assert_eq!(
        view["display_name"],
        name.as_str(),
        "a refused update changed nothing"
    );
    assert_eq!(view["colors"]["dark"]["acc"], color.as_str());

    // ── Logo: upload, read back, replace ──
    let first = png(64, 32, seed);
    let view = json_of(
        put_logo(&client, &admin, Some("image/png"), first.clone()).await,
        "upload PNG",
    )
    .await;
    let sha = sha256_hex(&first);
    assert_eq!(view["logo"]["mime"], "image/png");
    assert_eq!(view["logo"]["width"], 64);
    assert_eq!(view["logo"]["height"], 32);
    assert_eq!(view["logo"]["bytes"], first.len());
    assert_eq!(
        view["logo"]["url"],
        format!("/settings/branding/logo?v={}", &sha[..12])
    );

    let r = get(&client, Some(&admin), "/settings/branding/logo").await;
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "content-type"), "image/png");
    assert_eq!(header(&r, "x-content-type-options"), "nosniff");
    assert!(header(&r, "content-security-policy").contains("sandbox"));
    assert_eq!(header(&r, "cross-origin-resource-policy"), "same-origin");
    assert_eq!(
        header(&r, "content-disposition"),
        "inline; filename=\"logo.png\""
    );
    assert_eq!(header(&r, "cache-control"), "private, no-cache");
    let etag = header(&r, "etag");
    assert_eq!(etag, format!("\"{sha}\""));
    assert_eq!(r.bytes().await.expect("logo").as_ref(), first.as_slice());
    let r = get(
        &client,
        Some(&admin),
        &format!("/settings/branding/logo?v={}", &sha[..12]),
    )
    .await;
    assert_eq!(
        header(&r, "cache-control"),
        "private, max-age=31536000, immutable"
    );
    let r = client
        .get(url("/settings/branding/logo"))
        .bearer_auth(&admin)
        .header("if-none-match", &etag)
        .send()
        .await
        .expect("conditional GET");
    assert_eq!(r.status(), 304);
    assert_eq!(
        get(&client, Some(&viewer), "/settings/branding/logo")
            .await
            .status(),
        200,
        "a viewer can load the logo"
    );
    assert_eq!(
        get(&client, None, "/settings/branding/logo").await.status(),
        401
    );

    let second = png(128, 48, seed);
    let sha2 = sha256_hex(&second);
    let view = json_of(
        put_logo(&client, &admin, Some("image/png"), second.clone()).await,
        "replace PNG",
    )
    .await;
    assert_eq!(view["logo"]["width"], 128);
    assert_ne!(sha, sha2);
    let r = get(&client, Some(&admin), "/settings/branding/logo").await;
    assert_eq!(header(&r, "etag"), format!("\"{sha2}\""));

    // ── Logo refusals: type, mismatch, size ──
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"><script>alert(1)</script></svg>".to_vec();
    let mut gif = b"GIF89a\x40\x00\x20\x00".to_vec();
    gif.resize(64, 0);
    for (mime, body, why) in [
        (Some("image/svg+xml"), svg.clone(), "SVG"),
        (Some("image/png"), svg.clone(), "SVG declared as PNG"),
        (Some("image/webp"), second.clone(), "PNG declared as WebP"),
        (Some("image/gif"), gif.clone(), "GIF"),
        (Some("image/png"), gif, "GIF declared as PNG"),
        (Some("image/png"), png(8, 8, seed), "8x8 PNG"),
        (Some("image/png"), png(4096, 16, seed), "4096 px wide PNG"),
        (None, second.clone(), "no Content-Type"),
        (Some("image/png"), Vec::new(), "empty body"),
    ] {
        let r = put_logo(&client, &admin, mime, body).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{why}");
    }
    // 300 KiB: refused by the route's own 256 KiB limit, not the 2 GiB upload cap. The server
    // may close the connection before the client has sent everything; that is a refusal too.
    let mut big = second.clone();
    big.resize(300 * 1024, 0);
    match client
        .put(url("/settings/branding/logo"))
        .bearer_auth(&admin)
        .header("content-type", "image/png")
        .body(big)
        .send()
        .await
    {
        Ok(r) => assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE, "300 KiB"),
        Err(e) => eprintln!("300 KiB upload: connection closed early ({e})"),
    }
    let view = json_of(
        get(&client, Some(&admin), "/settings/branding").await,
        "GET",
    )
    .await;
    // The 300 KiB body starts with the same PNG header, so compare size and version too.
    assert_eq!(
        view["logo"]["bytes"],
        second.len(),
        "refused uploads changed nothing"
    );
    assert_eq!(
        view["logo"]["url"],
        format!("/settings/branding/logo?v={}", &sha2[..12])
    );
    // The settings update has its own small body limit too.
    match client
        .put(url("/settings/branding"))
        .bearer_auth(&admin)
        .json(&json!({ "display_name": "x".repeat(70 * 1024) }))
        .send()
        .await
    {
        Ok(r) => assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE, "70 KiB settings"),
        Err(e) => eprintln!("70 KiB settings: connection closed early ({e})"),
    }

    // WebP is accepted.
    let view = json_of(
        put_logo(&client, &admin, Some("image/webp"), webp_vp8x(100, 40)).await,
        "upload WebP",
    )
    .await;
    assert_eq!(view["logo"]["mime"], "image/webp");
    let r = get(&client, Some(&admin), "/settings/branding/logo").await;
    assert_eq!(header(&r, "content-type"), "image/webp");
    assert_eq!(
        header(&r, "content-disposition"),
        "inline; filename=\"logo.webp\""
    );
    // Back to the PNG for the public checks.
    json_of(
        put_logo(&client, &admin, Some("image/png"), second.clone()).await,
        "re-upload PNG",
    )
    .await;
    let view = json_of(
        put_json(&client, &admin, &json!({ "favicon_from_logo": true })).await,
        "favicon flag",
    )
    .await;
    assert_eq!(
        view["logo"]["public_url"].is_string(),
        signin,
        "public_url only for the sign-in organization"
    );

    // ── Public routes (no token) ──
    let r = get(&client, None, "/public/branding").await;
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "cache-control"), "no-cache");
    let public_etag = header(&r, "etag");
    assert!(!public_etag.is_empty());
    let text = r.text().await.expect("public branding");
    assert!(
        !text.contains(&org_id),
        "the public view names no organization id"
    );
    let public: Value = serde_json::from_str(&text).expect("public JSON");
    let mut keys: Vec<&String> = public.as_object().expect("object").keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "display_name",
            "favicon_url",
            "logo_url",
            "show_name_with_logo"
        ]
    );
    let logo_url = format!("/public/branding/logo?v={}", &sha2[..12]);
    if signin {
        assert_eq!(public["display_name"], name.as_str());
        assert_eq!(public["show_name_with_logo"], false);
        assert_eq!(public["logo_url"], logo_url.as_str());
        assert_eq!(public["favicon_url"], logo_url.as_str());
    } else {
        assert!(
            !text.contains(&name),
            "another org's sign-in page shows this name"
        );
        assert!(
            !text.contains(&sha2[..12]),
            "another org's sign-in page shows this logo"
        );
    }
    let r = client
        .get(url("/public/branding"))
        .header("if-none-match", &public_etag)
        .send()
        .await
        .expect("conditional public GET");
    assert_eq!(r.status(), 304);

    let r = get(&client, None, "/public/branding.css").await;
    assert_eq!(r.status(), 200);
    assert!(header(&r, "content-type").starts_with("text/css"));
    assert_eq!(header(&r, "x-content-type-options"), "nosniff");
    let css = r.text().await.expect("css");
    if signin {
        assert!(
            css.contains(&format!(
                "html[data-theme=\"dark\"]{{--acc:{color};--on-acc:#000000}}"
            )),
            "{css}"
        );
    } else {
        assert!(
            !css.contains(&color),
            "another org's sign-in page uses this color"
        );
    }

    let r = get(&client, None, "/public/branding/logo").await;
    if signin {
        assert_eq!(r.status(), 200);
        assert_eq!(header(&r, "cache-control"), "no-cache");
        assert_eq!(header(&r, "x-content-type-options"), "nosniff");
        assert!(header(&r, "content-security-policy").contains("sandbox"));
        assert_eq!(r.bytes().await.expect("logo").as_ref(), second.as_slice());
        let r = get(&client, None, &logo_url).await;
        assert_eq!(
            header(&r, "cache-control"),
            "public, max-age=31536000, immutable"
        );
    } else {
        let status = r.status();
        let bytes = r.bytes().await.unwrap_or_default();
        assert!(
            status == 404 || bytes.as_ref() != second.as_slice(),
            "another org's sign-in page serves this logo"
        );
    }

    // ── Remove the logo, then reset everything ──
    let view = json_of(
        delete(&client, &admin, "/settings/branding/logo").await,
        "delete logo",
    )
    .await;
    assert_eq!(view["logo"], Value::Null);
    assert_eq!(view["display_name"], name.as_str(), "the name stays");
    assert_eq!(
        get(&client, Some(&admin), "/settings/branding/logo")
            .await
            .status(),
        404
    );
    if signin {
        assert_eq!(
            get(&client, None, "/public/branding/logo").await.status(),
            404
        );
    }
    let view = json_of(delete(&client, &admin, "/settings/branding").await, "reset").await;
    assert_eq!(view["display_name"], Value::Null);
    assert_eq!(view["colors"], json!({}));
    assert_eq!(view["show_name_with_logo"], true);
    if signin {
        let css = get(&client, None, "/public/branding.css")
            .await
            .text()
            .await
            .expect("css");
        assert_eq!(css, "", "no overrides after a reset");
    }

    // ── Every change is audited ──
    let audit = json_of(
        get(&client, Some(&admin), "/audit?limit=200").await,
        "GET /audit",
    )
    .await;
    // Newest first. This run's last logo upload (unique to the run) marks where its events end;
    // the favicon update, the logo removal and the reset come after it.
    let events = audit["events"].as_array().expect("events");
    let last_upload = events
        .iter()
        .position(|e| {
            e["action"] == "org.branding_logo_set" && e["detail"]["sha256"] == sha2.as_str()
        })
        .expect("this run's logo upload is audited");
    let after = &events[..=last_upload];
    for action in [
        "org.branding_updated",
        "org.branding_logo_set",
        "org.branding_logo_cleared",
        "org.branding_reset",
    ] {
        let found = after.iter().find(|e| e["action"] == action);
        let found = found.unwrap_or_else(|| panic!("no {action} audit event from this run"));
        assert_eq!(found["object_type"], "organization");
        assert_eq!(found["object_id"], org_id.as_str());
    }
    assert_eq!(events[last_upload]["detail"]["mime"], "image/png");
    assert_eq!(events[last_upload]["detail"]["bytes"], second.len());
}
