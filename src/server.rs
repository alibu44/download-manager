//! Local HTTP API on 127.0.0.1 used by the app window and the browser extension.
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::engine::{is_video_page, AddReq, Manager};

pub const PORT: u16 = 52931;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
// Fixed ID derived from the "key" in extension/manifest.json.
const EXTENSION_ID: &str = "mgbbnbmokfiogmbbaobakhmfgmcagfbe";

const INDEX_HTML: &str = include_str!("../ui/index.html");
const SEARCH_JS: &str = include_str!("../ui/search.js");

pub type Callback = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<Manager>,
    pub on_show: Callback,
    pub on_quit: Callback,
}

fn host_ok(h: &HeaderMap) -> bool {
    // Blocks DNS rebinding: only our own loopback names are accepted.
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    host == format!("127.0.0.1:{PORT}") || host == format!("localhost:{PORT}")
}

fn origin_ok(h: &HeaderMap) -> bool {
    let o = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()).unwrap_or("");
    o == format!("http://127.0.0.1:{PORT}") || o == format!("http://localhost:{PORT}") || o == format!("chrome-extension://{EXTENSION_ID}")
}

fn reply(code: StatusCode, body: Value) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

async fn api_get(State(st): State<AppState>, Path(name): Path<String>, h: HeaderMap) -> Response {
    if !host_ok(&h) { return reply(StatusCode::FORBIDDEN, json!({ "error": "forbidden" })); }
    match name.as_str() {
        "ping" => reply(StatusCode::OK, json!({ "app": "download-manager", "version": VERSION })),
        "list" => reply(StatusCode::OK, json!({ "items": st.manager.list() })),
        "settings" => reply(StatusCode::OK, json!(st.manager.settings())),
        _ => reply(StatusCode::NOT_FOUND, json!({ "error": "not found" })),
    }
}

async fn api_post(State(st): State<AppState>, Path(name): Path<String>, h: HeaderMap, body: Bytes) -> Response {
    // Only the app's own page and our extension may change anything;
    // other web pages cannot forge the Origin header.
    if !host_ok(&h) || !origin_ok(&h) { return reply(StatusCode::FORBIDDEN, json!({ "error": "forbidden" })); }
    let b: Value = if body.is_empty() { json!({}) } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return reply(StatusCode::BAD_REQUEST, json!({ "error": e.to_string() })),
        }
    };
    match route(&st, &name, &b).await {
        Ok(v) => reply(StatusCode::OK, v),
        Err(e) => reply(StatusCode::BAD_REQUEST, json!({ "error": e })),
    }
}

/// Opens a file, folder or web page with the desktop's default handler.
fn shell_open(target: &str) {
    let _ = std::process::Command::new(if cfg!(windows) { "explorer" } else { "xdg-open" }).arg(target).spawn();
}

fn ids(b: &Value) -> Vec<String> {
    b["ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

async fn route(st: &AppState, name: &str, b: &Value) -> Result<Value, String> {
    let m = &st.manager;
    let s = |k: &str| b[k].as_str().unwrap_or("").to_owned();
    match name {
        "add" => {
            let format = match &b["format"] {
                Value::Null => None,
                f @ Value::Object(_) => Some(f.clone()),
                _ => return Err("bad format".into()),
            };
            let id = m.add(AddReq {
                url: s("url"), filename: b["filename"].as_str().map(str::to_owned), referer: s("referer"),
                cookies: s("cookies"), user_agent: s("userAgent"), description: s("description"),
                parent_page: s("parentPage"), format, title: b["title"].as_str().map(str::to_owned),
                ask: b["ask"].as_bool() == Some(true), size: b["size"].as_u64().unwrap_or(0),
            })?;
            if b["show"].as_bool() == Some(true) || b["ask"].as_bool() == Some(true) { (st.on_show)(); }
            Ok(json!({ "id": id }))
        }
        "confirm" => {
            m.confirm(&s("id"), &s("fileName"), &s("folder"), b["start"].as_bool() == Some(true));
            Ok(json!({ "ok": true }))
        }
        "formats" => {
            let url = s("url");
            if !url.starts_with("http://") && !url.starts_with("https://") { return Err("Not a supported video page".into()); }
            crate::ytdl::formats(&url).await
        }
        "isvideo" => Ok(json!({ "video": is_video_page(&s("url")) })),
        "resume" => { for i in ids(b) { m.resume(&i); } Ok(json!({ "ok": true })) }
        "stop" => { for i in ids(b) { m.stop(&i); } Ok(json!({ "ok": true })) }
        "stopall" => { m.stop_all(); Ok(json!({ "ok": true })) }
        "remove" => {
            let del = b["deleteFile"].as_bool() == Some(true);
            for i in ids(b) { m.remove(&i, del).await; }
            Ok(json!({ "ok": true }))
        }
        "removecompleted" => { m.remove_completed(); Ok(json!({ "ok": true })) }
        "open" => {
            let it = m.get(&s("id")).filter(|i| !i.path.is_empty()).ok_or("File not found")?;
            let exists = std::path::Path::new(&it.path).exists();
            if b["folder"].as_bool() == Some(true) {
                let target = if exists { it.path.clone() } else { format!("{}.part", it.path) };
                if cfg!(windows) {
                    let _ = std::process::Command::new("explorer").arg(format!("/select,{target}")).spawn();
                } else {
                    shell_open(std::path::Path::new(&target).parent().and_then(|p| p.to_str()).unwrap_or("."));
                }
            } else if it.status == "complete" && exists {
                shell_open(&it.path);
            } else {
                return Err("Download is not finished yet".into());
            }
            Ok(json!({ "ok": true }))
        }
        "refresh" => {
            // The next link the browser hands over continues this download instead of starting a new one.
            let page = m.await_refresh(&s("id")).ok_or("This download cannot be refreshed")?;
            let web = page.starts_with("http://") || page.starts_with("https://");
            if web { shell_open(&page); }
            Ok(json!({ "opened": web }))
        }
        "settings" => Ok(json!(m.save_settings(b))),
        "choosefolder" => {
            let f = rfd::AsyncFileDialog::new().set_directory(m.settings().folder).pick_folder().await;
            Ok(json!({ "folder": f.map(|h| h.path().to_string_lossy().into_owned()) }))
        }
        "show" => { (st.on_show)(); Ok(json!({ "ok": true })) }
        "quit" => { (st.on_quit)(); Ok(json!({ "ok": true })) }
        _ => Err("unknown endpoint".into()),
    }
}

async fn static_file(uri: Uri, h: HeaderMap) -> Response {
    if !host_ok(&h) { return reply(StatusCode::FORBIDDEN, json!({ "error": "forbidden" })); }
    let (ctype, body) = match uri.path() {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX_HTML),
        "/search.js" => ("text/javascript; charset=utf-8", SEARCH_JS),
        _ => return reply(StatusCode::NOT_FOUND, json!({ "error": "not found" })),
    };
    ([(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

pub async fn serve(listener: std::net::TcpListener, st: AppState) {
    let app = Router::new().route("/api/:name", get(api_get).post(api_post)).fallback(get(static_file)).with_state(st);
    let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
    let _ = axum::serve(listener, app).await;
}
