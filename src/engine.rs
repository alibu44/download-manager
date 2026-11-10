//! Download engine: dynamically segmented HTTP downloads and yt-dlp video downloads.
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use percent_encoding::percent_decode_str;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING, IF_RANGE, RANGE};
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
const MIN_SEGMENT: u64 = 1024 * 1024;
/// A busy segment is only split for an idle connection if at least this much is left.
const MIN_STEAL: u64 = 4 * 1024 * 1024;
const FLUSH_AT: usize = 256 * 1024;
const MAX_CONNECTIONS: u32 = 32;
const VIDEO_HOSTS: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "dailymotion.com", "twitch.tv",
    "twitter.com", "x.com", "tiktok.com", "instagram.com", "facebook.com", "fb.watch",
];

pub fn is_video_page(url: &str) -> bool {
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_default();
    VIDEO_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

fn home_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

pub fn app_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| Some(home_dir().join(".local/share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("DownloadManager")
}

/// Global token bucket shared by every connection (0 = unlimited).
pub struct Limiter {
    rate: AtomicU64, // bytes per second
    state: Mutex<(Instant, f64)>,
}

impl Limiter {
    async fn take(&self, n: usize) {
        let rate = self.rate.load(Relaxed) as f64;
        if rate == 0.0 { return; }
        let wait = {
            let mut s = self.state.lock().unwrap();
            let now = Instant::now();
            s.1 = (s.1 + now.duration_since(s.0).as_secs_f64() * rate).min(rate) - n as f64;
            s.0 = now;
            if s.1 < 0.0 { -s.1 / rate } else { 0.0 }
        };
        if wait > 0.0 { tokio::time::sleep(Duration::from_secs_f64(wait.min(2.0))).await; }
    }
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c < ' ' || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let s: String = s.trim_matches(|c| c == ' ' || c == '.').chars().take(200).collect();
    // CON, NUL, COM1... are devices on Windows whatever their extension.
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if s.is_empty() { "download".into() } else if device { format!("_{s}") } else { s }
}

/// Mark-of-the-Web: without it Windows treats the file as locally created, so SmartScreen and
/// Office Protected View would not apply to what was downloaded.
fn mark_downloaded(path: &str, url: &str, referer: &str) {
    if cfg!(windows) {
        let clean = |s: &str| s.replace(['\r', '\n'], "");
        let mut body = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
        if !referer.is_empty() { body += &format!("ReferrerUrl={}\r\n", clean(referer)); }
        body += &format!("HostUrl={}\r\n", clean(url));
        let _ = std::fs::write(format!("{path}:Zone.Identifier"), body);
    }
}

fn unique_path(folder: &Path, name: &str) -> PathBuf {
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut path = folder.join(name);
    let mut n = 2;
    while path.exists() || part_path(&path).exists() {
        path = folder.join(format!("{base} ({n}){ext}"));
        n += 1;
    }
    path
}

fn part_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn has_ext(name: &str) -> bool {
    matches!(name.rfind('.'), Some(i) if i > 0 && i + 1 < name.len())
}

fn url_basename(url: &str) -> String {
    reqwest::Url::parse(url).ok()
        .and_then(|u| u.path_segments().and_then(|s| s.last().map(str::to_owned)))
        .map(|s| percent_decode_str(&s).decode_utf8_lossy().into_owned())
        .unwrap_or_default()
}

fn cd_filename(cd: &str) -> Option<String> {
    let find = |key: &str| cd.split(';').map(str::trim).find_map(|p| {
        (p.len() > key.len() && p[..key.len()].eq_ignore_ascii_case(key)).then(|| p[key.len()..].trim().trim_matches('"').to_owned())
    });
    if let Some(v) = find("filename*=") {
        let enc = v.rfind("''").map(|i| &v[i + 2..]).unwrap_or(&v);
        return Some(percent_decode_str(enc).decode_utf8_lossy().into_owned());
    }
    find("filename=").filter(|v| !v.is_empty()).map(|v| percent_decode_str(&v).decode_utf8_lossy().into_owned())
}

fn name_from_response(r: &Response) -> String {
    let cd = r.headers().get("content-disposition").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    cd.and_then(|c| cd_filename(&c)).filter(|n| !n.is_empty()).unwrap_or_else(|| {
        let n = url_basename(r.url().as_str());
        if n.is_empty() { "download".into() } else { n }
    })
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Part {
    pub start: u64,
    pub end: i64, // inclusive; -1 when the size is unknown
    pub done: u64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub url: String,
    pub file_name: String,
    pub name_fixed: bool,
    pub path: String,
    pub size: u64,
    pub downloaded: u64,
    pub speed: u64,
    pub eta: Option<f64>,
    pub status: String,
    pub error: String,
    pub added: f64,
    pub last_try: f64,
    pub description: String,
    pub referer: String,
    pub parent_page: String,
    pub format: Option<Value>,
    pub format_label: String,
    pub headers: HashMap<String, String>,
    pub parts: Vec<Part>,
    /// ETag / Last-Modified sent as If-Range so a changed file is never stitched together.
    pub validator: String,
    pub connections: u32,
    /// Save folder chosen in the confirmation dialog; empty = the default folder.
    pub folder: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub folder: String,
    pub connections: u32,
    /// Downloads running at once; the rest wait as "queued".
    pub max_active: u32,
    /// KB/s across all downloads, 0 = unlimited.
    pub speed_limit: u64,
    /// Show the "download file info" dialog before a captured download starts.
    pub ask_before: bool,
    /// Extension origins (store builds, Firefox) the user allowed in the pairing dialog.
    pub extension_origins: Vec<String>,
    pub v: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { folder: home_dir().join("Downloads").to_string_lossy().into_owned(), connections: 16,
                   max_active: 4, speed_limit: 0, ask_before: true, extension_origins: Vec::new(), v: 2 }
    }
}

pub enum DlErr {
    Stopped,
    Fail(String),
}

impl From<std::io::Error> for DlErr {
    fn from(e: std::io::Error) -> Self { DlErr::Fail(e.to_string()) }
}

#[derive(Default)]
pub struct AddReq {
    pub url: String,
    pub filename: Option<String>,
    pub referer: String,
    pub cookies: String,
    pub user_agent: String,
    pub description: String,
    pub parent_page: String,
    pub format: Option<Value>,
    pub title: Option<String>,
    /// Captured by the browser: wait for the user's confirmation instead of starting.
    pub ask: bool,
    pub size: u64,
}

pub struct Manager {
    items: Mutex<HashMap<String, Item>>,
    workers: Mutex<HashMap<String, CancellationToken>>,
    settings: Mutex<Settings>,
    save_lock: Mutex<()>,
    client: reqwest::Client,
    counter: AtomicU64,
    slots: Arc<tokio::sync::Semaphore>,
    limiter: Arc<Limiter>,
    /// Item waiting for a fresh address: the next captured link continues it.
    refresh: Mutex<Option<(String, Instant)>>,
}

impl Manager {
    pub fn new() -> Arc<Self> {
        let dir = app_dir();
        let _ = std::fs::create_dir_all(&dir);
        let mut settings = Settings::default();
        if let Ok(s) = std::fs::read(dir.join("settings.json")) {
            if let Ok(v) = serde_json::from_slice::<Value>(&s) {
                let old = v.get("v").and_then(Value::as_u64).unwrap_or(1) < 2;
                if let Ok(mut loaded) = serde_json::from_value::<Settings>(v) {
                    if old { loaded.connections = Settings::default().connections; loaded.v = 2; }
                    settings = loaded;
                }
            }
        }
        let mut items = HashMap::new();
        if let Ok(s) = std::fs::read(dir.join("downloads.json")) {
            for mut it in serde_json::from_slice::<Vec<Item>>(&s).unwrap_or_default() {
                if matches!(it.status.as_str(), "downloading" | "queued" | "starting" | "merging") {
                    it.status = "paused".into();
                }
                it.speed = 0;
                items.insert(it.id.clone(), it);
            }
        }
        // HTTP/1.1 on purpose: every segment gets its own TCP connection (and congestion window);
        // the pool then reuses those connections when a worker moves on to its next segment.
        let client = reqwest::Client::builder()
            .http1_only()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(MAX_CONNECTIONS as usize)
            .pool_idle_timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("http client");
        settings.max_active = settings.max_active.clamp(1, 16);
        Arc::new(Manager {
            slots: Arc::new(tokio::sync::Semaphore::new(settings.max_active as usize)),
            limiter: Arc::new(Limiter { rate: AtomicU64::new(settings.speed_limit * 1024), state: Mutex::new((Instant::now(), 0.0)) }),
            refresh: Mutex::new(None),
            items: Mutex::new(items), workers: Mutex::new(HashMap::new()), settings: Mutex::new(settings),
            save_lock: Mutex::new(()), client, counter: AtomicU64::new(0),
        })
    }

    // ---------- persistence ----------
    pub fn save(&self) {
        let data = {
            let items = self.items.lock().unwrap();
            serde_json::to_vec(&items.values().collect::<Vec<_>>()).unwrap_or_default()
        };
        let _g = self.save_lock.lock().unwrap();
        let file = app_dir().join("downloads.json");
        let tmp = app_dir().join("downloads.json.tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }

    pub fn settings(&self) -> Settings { self.settings.lock().unwrap().clone() }

    pub fn allow_origin(&self, origin: &str) {
        let mut s = self.settings.lock().unwrap();
        if !s.extension_origins.iter().any(|o| o == origin) { s.extension_origins.push(origin.to_owned()); }
        let _ = std::fs::write(app_dir().join("settings.json"), serde_json::to_vec(&*s).unwrap_or_default());
    }

    pub fn save_settings(&self, new: &Value) -> Settings {
        let mut s = self.settings.lock().unwrap();
        if let Some(f) = new.get("folder").and_then(Value::as_str) {
            if Path::new(f).is_dir() { s.folder = f.to_owned(); }
        }
        if let Some(c) = new.get("connections").and_then(Value::as_u64) {
            s.connections = (c as u32).clamp(1, MAX_CONNECTIONS);
        }
        if let Some(a) = new.get("askBefore").and_then(Value::as_bool) { s.ask_before = a; }
        if let Some(l) = new.get("speedLimit").and_then(Value::as_u64) {
            s.speed_limit = l;
            self.limiter.rate.store(l * 1024, Relaxed);
        }
        if let Some(a) = new.get("maxActive").and_then(Value::as_u64) {
            let a = (a as u32).clamp(1, 16);
            if a > s.max_active { self.slots.add_permits((a - s.max_active) as usize); }
            if a < s.max_active { self.slots.forget_permits((s.max_active - a) as usize); }
            s.max_active = a;
        }
        let _ = std::fs::write(app_dir().join("settings.json"), serde_json::to_vec(&*s).unwrap_or_default());
        s.clone()
    }

    // ---------- public API ----------
    pub fn list(&self) -> Vec<Value> {
        let items = self.items.lock().unwrap();
        let mut all: Vec<&Item> = items.values().collect();
        all.sort_by(|a, b| b.added.total_cmp(&a.added));
        all.into_iter().map(|it| {
            let mut v = serde_json::to_value(it).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.remove("headers"); o.remove("parts"); o.remove("validator");
            }
            v
        }).collect()
    }

    pub fn get(&self, id: &str) -> Option<Item> { self.items.lock().unwrap().get(id).cloned() }

    pub fn with<R>(&self, id: &str, f: impl FnOnce(&mut Item) -> R) -> Option<R> {
        self.items.lock().unwrap().get_mut(id).map(f)
    }

    pub fn add(self: &Arc<Self>, r: AddReq) -> Result<String, String> {
        let lower = r.url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err("Only http(s) links can be downloaded".into());
        }
        if r.format.is_none() {
            if let Some(id) = self.refresh_target(&r) {
                // Same file, fresh address (expired link, new session): continue where it stopped.
                self.with(&id, |it| {
                    it.url = r.url.clone();
                    it.validator.clear(); // the size check in fetch() guards against a different file
                    if !r.user_agent.is_empty() { it.headers.insert("User-Agent".into(), r.user_agent.clone()); }
                    if !r.referer.is_empty() { it.headers.insert("Referer".into(), r.referer.clone()); }
                    if r.cookies.is_empty() { it.headers.remove("Cookie"); } else { it.headers.insert("Cookie".into(), r.cookies.clone()); }
                });
                self.resume(&id);
                return Ok(id);
            }
        }
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
        let id = format!("{:012x}", (nanos ^ self.counter.fetch_add(1, Relaxed).rotate_left(40)) & 0xffff_ffff_ffff);
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_owned(), if r.user_agent.is_empty() { DEFAULT_UA.to_owned() } else { r.user_agent });
        if !r.referer.is_empty() { headers.insert("Referer".to_owned(), r.referer.clone()); }
        if !r.cookies.is_empty() { headers.insert("Cookie".to_owned(), r.cookies); }
        let name = r.filename.clone().filter(|s| !s.is_empty()).or(r.title).unwrap_or_else(|| url_basename(&r.url));
        let it = Item {
            id: id.clone(),
            kind: if r.format.is_some() { "ytdl" } else { "http" }.into(),
            file_name: safe_name(&name),
            name_fixed: r.filename.is_some_and(|s| !s.is_empty()),
            status: "queued".into(), added: now(), last_try: now(),
            description: r.description,
            parent_page: if r.parent_page.is_empty() { r.referer.clone() } else { r.parent_page },
            referer: r.referer,
            format_label: r.format.as_ref().and_then(|f| f.get("label")).and_then(Value::as_str).unwrap_or("").to_owned(),
            format: r.format, url: r.url, headers, size: r.size,
            ..Default::default()
        };
        let ask = r.ask && it.kind == "http" && self.settings().ask_before;
        self.items.lock().unwrap().insert(id.clone(), it);
        if ask {
            self.with(&id, |it| it.status = "confirm".into());
            self.save();
        } else {
            self.save();
            self.resume(&id);
        }
        Ok(id)
    }

    /// Answer from the confirmation dialog: start now (optionally renamed / in another folder) or keep for later.
    pub fn confirm(self: &Arc<Self>, id: &str, name: &str, folder: &str, start: bool) {
        self.with(id, |it| {
            if it.status != "confirm" { return; }
            let name = safe_name(name);
            if !name.is_empty() && name != it.file_name { it.file_name = name; it.name_fixed = true; }
            if Path::new(folder).is_dir() { it.folder = folder.to_owned(); }
            it.status = "paused".into();
        });
        if start { self.resume(id); } else { self.save(); }
    }

    /// Marks an unfinished download as waiting for a new address and returns its source page.
    pub fn await_refresh(&self, id: &str) -> Option<String> {
        let it = self.get(id).filter(|i| i.kind == "http" && i.status != "complete")?;
        *self.refresh.lock().unwrap() = Some((id.to_owned(), Instant::now()));
        Some(it.parent_page)
    }

    fn refresh_target(&self, r: &AddReq) -> Option<String> {
        let workers = self.workers.lock().unwrap();
        let items = self.items.lock().unwrap();
        let resumable = |it: &Item| it.kind == "http" && matches!(it.status.as_str(), "error" | "paused")
            && !it.path.is_empty() && !workers.contains_key(&it.id);
        if let Some((id, at)) = self.refresh.lock().unwrap().take() {
            if at.elapsed() < Duration::from_secs(600) && items.get(&id).is_some_and(resumable) { return Some(id); }
        }
        // Otherwise match a partly downloaded file by name, or by address without its query string.
        let bare = |u: &str| u.split(['?', '#']).next().unwrap_or("").to_owned();
        let name = r.filename.as_deref().filter(|n| !n.is_empty()).map(safe_name);
        items.values().filter(|it| resumable(it) && it.downloaded > 0 && it.size > 0)
            .find(|it| name.as_deref() == Some(it.file_name.as_str()) || bare(&it.url) == bare(&r.url))
            .map(|it| it.id.clone())
    }

    pub fn resume(self: &Arc<Self>, id: &str) {
        let token = CancellationToken::new();
        {
            let mut workers = self.workers.lock().unwrap();
            let mut items = self.items.lock().unwrap();
            let Some(it) = items.get_mut(id) else { return };
            if it.status == "complete" || workers.contains_key(id) { return; }
            it.status = "queued".into();
            it.error.clear();
            it.last_try = now();
            workers.insert(id.to_owned(), token.clone());
        }
        let (me, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            let permit = tokio::select! {
                _ = token.cancelled() => None,
                p = me.slots.clone().acquire_owned() => p.ok(),
            };
            let kind = me.get(&id).map(|i| i.kind).unwrap_or_default();
            let res = if permit.is_none() {
                Err(DlErr::Stopped)
            } else {
                me.with(&id, |it| it.status = "starting".into());
                if kind == "ytdl" { crate::ytdl::run(&me, &id, &token).await } else { me.run_http(&id, &token).await }
            };
            drop(permit);
            me.with(&id, |it| {
                match res {
                    Ok(()) => {
                        it.status = "complete".into();
                        it.eta = Some(0.0);
                        it.headers.remove("Cookie"); // session cookies are not kept on disk once the file is done
                        mark_downloaded(&it.path, &it.url, &it.parent_page);
                    }
                    Err(DlErr::Stopped) => it.status = "paused".into(),
                    Err(DlErr::Fail(e)) => { it.status = "error".into(); it.error = e.chars().take(300).collect(); }
                }
                it.speed = 0;
            });
            me.workers.lock().unwrap().remove(&id);
            me.save();
        });
    }

    pub fn stop(&self, id: &str) {
        if let Some(t) = self.workers.lock().unwrap().get(id) { t.cancel(); }
    }

    pub fn stop_all(&self) {
        for t in self.workers.lock().unwrap().values() { t.cancel(); }
    }

    pub async fn remove(&self, id: &str, delete_file: bool) {
        self.stop(id);
        for _ in 0..50 {
            if !self.workers.lock().unwrap().contains_key(id) { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let it = self.items.lock().unwrap().remove(id);
        if let Some(it) = it.filter(|i| !i.path.is_empty()) {
            if delete_file { let _ = std::fs::remove_file(&it.path); }
            let _ = std::fs::remove_file(part_path(Path::new(&it.path)));
        }
        self.save();
    }

    pub fn remove_completed(&self) {
        self.items.lock().unwrap().retain(|_, it| it.status != "complete");
        self.save();
    }

    fn header_map(it: &Item) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in &it.headers {
            if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
                h.insert(k, v);
            }
        }
        // Byte ranges must address the stored bytes, never a compressed representation.
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        h
    }

    // ---------- HTTP ----------
    async fn run_http(self: &Arc<Self>, id: &str, stop: &CancellationToken) -> Result<(), DlErr> {
        let mut it = self.get(id).ok_or_else(|| DlErr::Fail("removed".into()))?;
        let headers = Self::header_map(&it);
        if !it.path.is_empty() && !part_path(Path::new(&it.path)).exists() {
            it.path.clear(); // the partial file is gone: start over
        }
        let mut first: Option<Response> = None;
        if it.path.is_empty() {
            let req = self.client.get(&it.url).headers(headers.clone()).header(RANGE, "bytes=0-").send();
            let r = tokio::select! {
                _ = stop.cancelled() => return Err(DlErr::Stopped),
                r = req => r.map_err(|e| DlErr::Fail(net_err(&e)))?,
            };
            if !r.status().is_success() {
                return Err(DlErr::Fail(format!("HTTP {}", r.status())));
            }
            let h = |n: &str| r.headers().get(n).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
            let lname = it.file_name.to_lowercase();
            if h("content-type").contains("text/html") && !(lname.ends_with(".htm") || lname.ends_with(".html")) {
                return Err(DlErr::Fail("Server returned a web page instead of a file (link may need login or has expired)".into()));
            }
            let partial = r.status() == StatusCode::PARTIAL_CONTENT;
            let mut total = h("content-length").parse::<u64>().unwrap_or(0);
            if partial {
                if let Some(t) = h("content-range").rsplit('/').next().and_then(|t| t.parse::<u64>().ok()) { total = t; }
            }
            let ranged = (partial || h("accept-ranges").eq_ignore_ascii_case("bytes")) && total > 0;
            let server_name = safe_name(&name_from_response(&r));
            // Content-Disposition name wins unless the browser already told us the final name.
            let mut name = if it.name_fixed { it.file_name.clone() } else { server_name.clone() };
            if !has_ext(&name) && has_ext(&server_name) { name = server_name; }
            let etag = h("etag");
            it.validator = if !etag.is_empty() && !etag.starts_with("W/") { etag } else { h("last-modified") };
            it.url = r.url().to_string();
            let s = self.settings();
            let folder = PathBuf::from(if it.folder.is_empty() { &s.folder } else { &it.folder });
            std::fs::create_dir_all(&folder)?;
            let path = unique_path(&folder, &name);
            it.file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
            it.path = path.to_string_lossy().into_owned();
            it.size = total;
            it.downloaded = 0;
            it.connections = s.connections;
            let n = if ranged { (s.connections as u64).min((total / MIN_SEGMENT).max(1)) } else { 1 };
            let step = total / n;
            it.parts = (0..n).map(|i| Part {
                start: i * step,
                end: if !ranged { -1 } else if i == n - 1 { total as i64 - 1 } else { ((i + 1) * step) as i64 - 1 },
                done: 0,
            }).collect();
            let f = File::create(part_path(&path))?;
            if ranged { f.set_len(total)?; }
            first = Some(r); // the probe's body is already streaming: use it for the first segment
            let snapshot = it.clone();
            self.with(id, |x| { *x = snapshot; x.status = "downloading".into(); });
            self.save();
        }
        self.with(id, |x| x.status = "downloading".into());
        let path = PathBuf::from(&it.path);
        let part = part_path(&path);
        let file = OpenOptions::new().write(true).open(&part)?;
        let ranged = it.parts.first().is_some_and(|p| p.end >= 0);
        if ranged {
            self.run_segmented(id, &it, file, headers, first, stop).await?;
        } else {
            self.run_single(id, &it, file, headers, first, stop).await?;
        }
        std::fs::rename(&part, &path)?;
        Ok(())
    }

    /// No range support / unknown size: one sequential stream, restarted from zero on resume.
    async fn run_single(&self, id: &str, it: &Item, file: File, headers: HeaderMap, first: Option<Response>,
                        stop: &CancellationToken) -> Result<(), DlErr> {
        let resp = match first {
            Some(r) => r,
            None => {
                let r = self.client.get(&it.url).headers(headers).send().await.map_err(|e| DlErr::Fail(net_err(&e)))?;
                if !r.status().is_success() { return Err(DlErr::Fail(format!("HTTP {}", r.status()))); }
                r
            }
        };
        file.set_len(0)?;
        let mut stream = resp.bytes_stream();
        let (mut pos, mut last_b, mut last_t, mut speed) = (0u64, 0u64, Instant::now(), 0f64);
        loop {
            let chunk = tokio::select! {
                _ = stop.cancelled() => return Err(DlErr::Stopped),
                c = tokio::time::timeout(Duration::from_secs(45), stream.next()) => c,
            };
            let bytes = match chunk {
                Err(_) => return Err(DlErr::Fail("Connection timed out".into())),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(DlErr::Fail(net_err(&e))),
                Ok(Some(Ok(b))) => b,
            };
            write_all_at(&file, &bytes, pos)?;
            pos += bytes.len() as u64;
            self.limiter.take(bytes.len()).await;
            let dt = last_t.elapsed().as_secs_f64();
            if dt >= 0.5 {
                let inst = (pos - last_b) as f64 / dt;
                speed = if speed == 0.0 { inst } else { speed * 0.6 + inst * 0.4 };
                (last_b, last_t) = (pos, Instant::now());
                self.with(id, |x| {
                    x.downloaded = pos;
                    x.speed = speed as u64;
                    x.eta = (x.size > pos && speed > 0.0).then(|| (x.size - pos) as f64 / speed);
                    if let Some(p) = x.parts.first_mut() { p.done = pos; }
                });
            }
        }
        if it.size > 0 && pos < it.size {
            return Err(DlErr::Fail("Connection closed early".into()));
        }
        self.with(id, |x| { x.downloaded = pos; x.size = pos; });
        Ok(())
    }

    /// IDM-style dynamic segmentation: N connections, and whenever one finishes its range it
    /// takes over half of the largest remaining range, so no connection ever sits idle.
    async fn run_segmented(&self, id: &str, it: &Item, file: File, headers: HeaderMap, first: Option<Response>,
                           stop: &CancellationToken) -> Result<(), DlErr> {
        let segs: Vec<Arc<Seg>> = it.parts.iter().map(|p| Arc::new(Seg {
            start: p.start, end: AtomicU64::new(p.end as u64 + 1), pos: AtomicU64::new(p.start + p.done),
            owned: AtomicBool::new(false),
        })).collect();
        let ctx = Arc::new(Ctx {
            segs: Mutex::new(segs), file, url: it.url.clone(), headers, client: self.client.clone(),
            stop: stop.clone(), fatal: Mutex::new(None), last_err: Mutex::new(String::new()),
            validator: it.validator.clone(), size: it.size, limiter: self.limiter.clone(),
        });
        let conns = if it.connections > 0 { it.connections } else { self.settings().connections }.clamp(1, MAX_CONNECTIONS);
        let mut set = JoinSet::new();
        let mut first = first.and_then(|r| {
            let s = ctx.segs.lock().unwrap().first().cloned()?;
            (s.start == 0 && s.pos.load(Relaxed) == 0 && !s.owned.swap(true, Relaxed)).then_some((s, r))
        });
        for _ in 0..conns {
            set.spawn(worker(ctx.clone(), first.take()));
        }
        let (mut last_b, mut last_t, mut speed, mut last_save) = (ctx.done(), Instant::now(), 0f64, Instant::now());
        loop {
            tokio::select! {
                r = set.join_next() => if r.is_none() { break },
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            let done = ctx.done();
            let dt = last_t.elapsed().as_secs_f64();
            if dt >= 0.4 {
                let inst = done.saturating_sub(last_b) as f64 / dt;
                speed = if speed == 0.0 { inst } else { speed * 0.6 + inst * 0.4 };
                (last_b, last_t) = (done, Instant::now());
            }
            let persist = last_save.elapsed() >= Duration::from_secs(2);
            self.with(id, |x| {
                x.downloaded = done;
                x.speed = speed as u64;
                x.eta = (speed > 0.0).then(|| x.size.saturating_sub(done) as f64 / speed);
                if persist { x.parts = ctx.parts(); }
            });
            if persist {
                self.save();
                last_save = Instant::now();
            }
        }
        let done = ctx.done();
        self.with(id, |x| { x.downloaded = done; x.parts = ctx.parts(); });
        if let Some(e) = ctx.fatal.lock().unwrap().take() { return Err(DlErr::Fail(e)); }
        if stop.is_cancelled() { return Err(DlErr::Stopped); }
        if done < it.size {
            let e = ctx.last_err.lock().unwrap().clone();
            return Err(DlErr::Fail(if e.is_empty() { "Connection lost".into() } else { e }));
        }
        Ok(())
    }
}

fn net_err(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(c) = src {
        s = c.to_string();
        src = c.source();
    }
    s
}

fn write_all_at(file: &File, mut buf: &[u8], mut pos: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        #[cfg(windows)]
        let n = file.seek_write(buf, pos)?;
        #[cfg(unix)]
        let n = file.write_at(buf, pos)?;
        if n == 0 { return Err(std::io::ErrorKind::WriteZero.into()); }
        buf = &buf[n..];
        pos += n as u64;
    }
    Ok(())
}

/// One byte range. `end` is exclusive and shrinks when another worker takes over the tail.
struct Seg {
    start: u64,
    end: AtomicU64,
    pos: AtomicU64,
    owned: AtomicBool,
}

impl Seg {
    fn remaining(&self) -> u64 { self.end.load(Relaxed).saturating_sub(self.pos.load(Relaxed)) }
}

struct Ctx {
    segs: Mutex<Vec<Arc<Seg>>>,
    file: File,
    url: String,
    headers: HeaderMap,
    client: reqwest::Client,
    stop: CancellationToken,
    fatal: Mutex<Option<String>>,
    last_err: Mutex<String>,
    validator: String,
    size: u64,
    limiter: Arc<Limiter>,
}

enum FetchErr {
    Stopped,
    Retry(String),
    Fatal(String),
}

impl Ctx {
    fn done(&self) -> u64 {
        self.segs.lock().unwrap().iter().map(|s| s.pos.load(Relaxed).min(s.end.load(Relaxed)) - s.start).sum()
    }

    /// Resume data; adjacent finished ranges are merged so the list stays short.
    fn parts(&self) -> Vec<Part> {
        let mut v: Vec<(u64, u64, u64)> = self.segs.lock().unwrap().iter().map(|s| {
            let end = s.end.load(Relaxed);
            (s.start, end, s.pos.load(Relaxed).min(end))
        }).collect();
        v.sort_unstable();
        let mut out: Vec<Part> = Vec::with_capacity(v.len());
        for (start, end, pos) in v {
            if end <= start { continue; }
            match out.last_mut() {
                Some(p) if p.start + p.done == p.end as u64 + 1 && p.end as u64 + 1 == start && pos == end => {
                    p.end = end as i64 - 1;
                    p.done += end - start;
                }
                _ => out.push(Part { start, end: end as i64 - 1, done: pos - start }),
            }
        }
        out
    }

    /// Next piece of work: an unowned unfinished range, else the second half of the busiest one.
    fn take(&self) -> Option<Arc<Seg>> {
        let mut segs = self.segs.lock().unwrap();
        let free = segs.iter().filter(|s| !s.owned.load(Relaxed) && s.remaining() > 0).max_by_key(|s| s.remaining()).cloned();
        if let Some(s) = free {
            s.owned.store(true, Relaxed);
            return Some(s);
        }
        let big = segs.iter().max_by_key(|s| s.remaining()).cloned()?;
        let (pos, end) = (big.pos.load(Relaxed), big.end.load(Relaxed));
        if end.saturating_sub(pos) < MIN_STEAL { return None; }
        let mid = (pos + (end - pos) / 2) & !0xffff;
        big.end.store(mid, Relaxed);
        let s = Arc::new(Seg { start: mid, end: AtomicU64::new(end), pos: AtomicU64::new(mid), owned: AtomicBool::new(true) });
        segs.push(s.clone());
        Some(s)
    }

    async fn fetch(&self, seg: &Seg, resp: Option<Response>) -> Result<(), FetchErr> {
        let from = seg.pos.load(Relaxed);
        let end = seg.end.load(Relaxed);
        if from >= end { return Ok(()); }
        let resp = match resp {
            Some(r) => r,
            None => {
                let mut rb = self.client.get(&self.url).headers(self.headers.clone())
                    .header(RANGE, format!("bytes={}-{}", from, end - 1));
                if !self.validator.is_empty() { rb = rb.header(IF_RANGE, &self.validator); }
                let r = tokio::select! {
                    _ = self.stop.cancelled() => return Err(FetchErr::Stopped),
                    r = rb.send() => r.map_err(|e| FetchErr::Retry(net_err(&e)))?,
                };
                let total = r.headers().get("content-range").and_then(|v| v.to_str().ok())
                    .and_then(|v| v.rsplit('/').next()).and_then(|t| t.parse::<u64>().ok());
                match r.status().as_u16() {
                    206 if total.is_some_and(|t| t != self.size) =>
                        return Err(FetchErr::Fatal("The address now points to a different file (size changed)".into())),
                    206 => r,
                    200 => return Err(FetchErr::Fatal("The file changed on the server or resuming is not supported".into())),
                    401 | 404 | 410 | 416 => return Err(FetchErr::Fatal(format!("HTTP {}", r.status()))),
                    _ => return Err(FetchErr::Retry(format!("HTTP {}", r.status()))),
                }
            }
        };
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_AT + 64 * 1024);
        let mut at = from; // file offset of buf[0]
        let res = loop {
            let chunk = tokio::select! {
                _ = self.stop.cancelled() => break Err(FetchErr::Stopped),
                c = tokio::time::timeout(Duration::from_secs(30), stream.next()) => c,
            };
            let bytes = match chunk {
                Err(_) => break Err(FetchErr::Retry("Connection timed out".into())),
                Ok(None) => break Err(FetchErr::Retry("Connection closed early".into())),
                Ok(Some(Err(e))) => break Err(FetchErr::Retry(net_err(&e))),
                Ok(Some(Ok(b))) => b,
            };
            let fetched = at + buf.len() as u64;
            let room = seg.end.load(Relaxed).saturating_sub(fetched);
            let take = (bytes.len() as u64).min(room) as usize;
            buf.extend_from_slice(&bytes[..take]);
            self.limiter.take(take).await;
            let finished = take as u64 == room;
            if buf.len() >= FLUSH_AT || finished {
                if let Err(e) = tokio::task::block_in_place(|| write_all_at(&self.file, &buf, at)) {
                    return Err(FetchErr::Fatal(e.to_string()));
                }
                at += buf.len() as u64;
                buf.clear();
                seg.pos.store(at, Relaxed);
            }
            if finished { break Ok(()); }
        };
        if !buf.is_empty() {
            if let Err(e) = tokio::task::block_in_place(|| write_all_at(&self.file, &buf, at)) {
                return Err(FetchErr::Fatal(e.to_string()));
            }
            seg.pos.store(at + buf.len() as u64, Relaxed);
        }
        res
    }
}

async fn worker(ctx: Arc<Ctx>, mut first: Option<(Arc<Seg>, Response)>) {
    let mut fails = 0u32;
    loop {
        if ctx.stop.is_cancelled() { return; }
        let (seg, resp) = match first.take() {
            Some((s, r)) => (s, Some(r)),
            None => match ctx.take() {
                Some(s) => (s, None),
                None => return,
            },
        };
        let before = seg.pos.load(Relaxed);
        let res = ctx.fetch(&seg, resp).await;
        seg.owned.store(false, Relaxed);
        match res {
            Ok(()) => fails = 0,
            Err(FetchErr::Stopped) => return,
            Err(FetchErr::Fatal(e)) => {
                ctx.fatal.lock().unwrap().get_or_insert(e);
                ctx.stop.cancel();
                return;
            }
            Err(FetchErr::Retry(e)) => {
                // Servers that cap parallel connections reject the extra ones: this worker then
                // backs off and eventually retires, while the accepted connections keep going.
                fails = if seg.pos.load(Relaxed) > before { 0 } else { fails + 1 };
                *ctx.last_err.lock().unwrap() = e;
                if fails >= 6 { return; }
                let wait = Duration::from_millis((1000u64 << fails.min(4)).min(15_000));
                tokio::select! {
                    _ = ctx.stop.cancelled() => return,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        }
    }
}
