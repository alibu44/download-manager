//! Video downloads through the bundled yt-dlp.exe / ffmpeg.exe.
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::engine::{DlErr, Manager, DEFAULT_UA};

/// Console tools must not flash a window on Windows.
fn hidden(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

/// A bundled tool next to the app, else whatever is on PATH.
fn tool(name: &str) -> PathBuf {
    let name = &format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    [dir.join(name), dir.join("bin").join(name)].into_iter().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

fn base_cmd() -> Command {
    static NODE: OnceLock<bool> = OnceLock::new();
    let mut c = Command::new(tool("yt-dlp"));
    hidden(&mut c).stdin(Stdio::null()).kill_on_drop(true);
    c.args(["--no-warnings", "--no-playlist", "--encoding", "utf-8"]);
    let ff = tool("ffmpeg");
    if ff.is_file() { c.arg("--ffmpeg-location").arg(ff); }
    // YouTube needs a JavaScript runtime; yt-dlp only looks for deno unless told otherwise.
    if *NODE.get_or_init(|| on_path(&format!("node{}", std::env::consts::EXE_SUFFIX))) { c.args(["--js-runtimes", "node"]); }
    c
}

fn last_error(stderr: &str) -> String {
    let line = stderr.lines().rev().find(|l| l.contains("ERROR")).or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()));
    line.unwrap_or("yt-dlp failed").trim().trim_start_matches("ERROR:").trim().to_owned()
}

/// Title + quality choices for a video page (like IDM's "Download this video" menu).
pub async fn formats(url: &str) -> Result<Value, String> {
    let out = base_cmd().arg("-J").arg(url).output().await
        .map_err(|e| format!("yt-dlp could not be started: {e}"))?;
    if !out.status.success() {
        return Err(last_error(&String::from_utf8_lossy(&out.stderr)));
    }
    let info: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let empty = Vec::new();
    let fmts = info["formats"].as_array().unwrap_or(&empty);
    let dur = info["duration"].as_f64().unwrap_or(0.0);
    let s = |f: &Value, k: &str| f[k].as_str().unwrap_or("").to_owned();
    let has = |f: &Value, k: &str| !matches!(f[k].as_str(), None | Some("none"));
    let size = |f: &Value| -> u64 {
        f["filesize"].as_u64().or(f["filesize_approx"].as_u64())
            .unwrap_or_else(|| f["tbr"].as_f64().map(|t| (t * 1000.0 / 8.0 * dur) as u64).unwrap_or(0))
    };
    let best_m4a = fmts.iter().filter(|f| !has(f, "vcodec") && has(f, "acodec") && s(f, "ext") == "m4a")
        .max_by(|a, b| a["abr"].as_f64().unwrap_or(0.0).total_cmp(&b["abr"].as_f64().unwrap_or(0.0)));
    let any_audio = fmts.iter().any(|f| !has(f, "vcodec") && has(f, "acodec"));
    let mut heights: Vec<u64> = fmts.iter().filter(|f| has(f, "vcodec")).filter_map(|f| f["height"].as_u64()).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let mut options = Vec::new();
    for h in heights {
        let cands: Vec<&Value> = fmts.iter().filter(|f| has(f, "vcodec") && f["height"].as_u64() == Some(h)).collect();
        let avc: Vec<&Value> = cands.iter().copied().filter(|f| s(f, "vcodec").starts_with("avc1")).collect();
        let mp4: Vec<&Value> = cands.iter().copied().filter(|f| s(f, "ext") == "mp4").collect();
        let pool = if !avc.is_empty() { avc } else if !mp4.is_empty() { mp4 } else { cands };
        let Some(best) = pool.into_iter().max_by(|a, b| a["tbr"].as_f64().unwrap_or(0.0).total_cmp(&b["tbr"].as_f64().unwrap_or(0.0))) else { continue };
        let fps = best["fps"].as_f64().unwrap_or(0.0);
        let label = format!("{h}p{} MP4", if fps > 30.0 { format!("{}", fps as u64) } else { String::new() });
        let est = size(best) + if !has(best, "acodec") { best_m4a.map(size).unwrap_or(0) } else { 0 };
        options.push(json!({
            "label": label, "height": h, "size": est,
            // Prefer H.264 so the file plays everywhere; fall back to VP9/AV1 above 1080p.
            "format": format!("bv*[height={h}][vcodec^=avc1]+ba[ext=m4a]/bv*[height={h}][ext=mp4]+ba[ext=m4a]/bv*[height={h}]+ba/b[height={h}]"),
            "merge": "mp4",
        }));
    }
    if options.is_empty() {
        options.push(json!({ "label": "En iyi kalite", "height": 0, "size": 0, "format": "bv*+ba/b", "merge": "mp4" }));
    }
    if any_audio {
        options.push(json!({ "label": "Sadece ses (M4A)", "height": 0, "size": best_m4a.map(size).unwrap_or(0),
                             "format": "ba[ext=m4a]/ba", "merge": Value::Null }));
    }
    Ok(json!({
        "title": info["title"].as_str().unwrap_or("video"), "thumbnail": info["thumbnail"], "duration": dur,
        "webpage_url": info["webpage_url"].as_str().unwrap_or(url), "options": options,
    }))
}

async fn kill_tree(pid: Option<u32>) {
    // yt-dlp.exe is a self-extracting launcher: the real work happens in a child process.
    if let Some(pid) = pid {
        let mut c = if cfg!(windows) { Command::new("taskkill") } else { Command::new("pkill") };
        if cfg!(windows) { c.args(["/PID", &pid.to_string(), "/T", "/F"]); } else { c.args(["-TERM", "-P", &pid.to_string()]); }
        let _ = hidden(&mut c).stdout(Stdio::null()).stderr(Stdio::null()).status().await;
    }
}

pub async fn run(m: &Arc<Manager>, id: &str, stop: &CancellationToken) -> Result<(), DlErr> {
    let it = m.get(id).ok_or_else(|| DlErr::Fail("removed".into()))?;
    let fmt = it.format.clone().unwrap_or(Value::Null);
    let folder = PathBuf::from(m.settings().folder);
    std::fs::create_dir_all(&folder)?;
    m.with(id, |x| x.status = "downloading".into());

    let mut cmd = base_cmd();
    cmd.arg("-f").arg(fmt["format"].as_str().unwrap_or("bv*+ba/b"));
    if let Some(merge) = fmt["merge"].as_str() { cmd.args(["--merge-output-format", merge]); }
    cmd.arg("-o").arg(folder.join("%(title).150B [%(id)s].%(ext)s"));
    cmd.args(["--newline", "--no-colors", "--progress", "--continue", "-N", "8",
              "--progress-template", "download:DMP|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s",
              "--print", "post_process:DMM|", "--print", "after_move:DMF|%(filepath)s"]);
    cmd.arg("--user-agent").arg(it.headers.get("User-Agent").map(String::as_str).unwrap_or(DEFAULT_UA));
    if !it.referer.is_empty() && !crate::engine::is_video_page(&it.url) { cmd.arg("--referer").arg(&it.referer); }
    cmd.arg(&it.url).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| DlErr::Fail(format!("yt-dlp could not be started: {e}")))?;
    let pid = child.id();
    let mut stderr = child.stderr.take().expect("piped");
    let err_task = tokio::spawn(async move {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s).await;
        String::from_utf8_lossy(&s).into_owned()
    });
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    // Merged downloads report each stream separately: `base` holds the finished ones.
    let (mut base, mut last_dl, mut last_total) = (0f64, 0f64, 0f64);
    loop {
        let line = tokio::select! {
            _ = stop.cancelled() => {
                kill_tree(pid).await;
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(DlErr::Stopped);
            }
            l = lines.next_line() => match l { Ok(Some(l)) => l, _ => break },
        };
        if let Some(rest) = line.strip_prefix("DMP|") {
            let v: Vec<f64> = rest.split('|').map(|x| x.trim().parse().unwrap_or(0.0)).collect();
            if v.len() < 5 { continue; }
            let (dl, total) = (v[0], if v[1] > 0.0 { v[1] } else { v[2] });
            if dl + 1.0 < last_dl { base += last_total.max(last_dl); }
            (last_dl, last_total) = (dl, total);
            m.with(id, |x| {
                x.size = x.size.max((base + total) as u64);
                x.downloaded = (base + dl) as u64;
                x.speed = v[3] as u64;
                x.eta = (v[4] > 0.0).then_some(v[4]);
            });
        } else if line.starts_with("DMM|") {
            m.with(id, |x| { x.status = "merging".into(); x.speed = 0; });
        } else if let Some(path) = line.strip_prefix("DMF|") {
            let p = PathBuf::from(path.trim());
            m.with(id, |x| {
                x.file_name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                x.path = p.to_string_lossy().into_owned();
            });
        }
    }
    let status = child.wait().await.map_err(|e| DlErr::Fail(e.to_string()))?;
    let stderr = err_task.await.unwrap_or_default();
    if stop.is_cancelled() { return Err(DlErr::Stopped); }
    if !status.success() { return Err(DlErr::Fail(last_error(&stderr))); }
    m.with(id, |x| {
        if let Ok(meta) = std::fs::metadata(&x.path) {
            x.size = meta.len();
            x.downloaded = meta.len();
        }
    });
    Ok(())
}
