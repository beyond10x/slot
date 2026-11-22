//! Web UI and its JSON/SSE API.

use std::convert::Infallible;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::Path as FsPath;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::{self, Stream, StreamExt};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::{BroadcastStream, UnboundedReceiverStream};

use super::Server;
use crate::sccache::{CacheMinute, Counters, cache_minutes};

static UI: Dir<'_> = include_dir!("$OUT_DIR/ui");

const CHART_WINDOW_MS: i64 = 6 * 60 * 60 * 1_000;
const CHART_TAIL_START: u64 = 1 << 20;
const CHART_TAIL_LIMIT: u64 = 32 << 20;

pub(super) async fn serve(s: Arc<Server>, addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route("/api/state", get(state))
        .route("/api/events", get(events))
        .route("/api/jobs/{id}/output", get(output))
        .route("/metrics", get(metrics))
        .route("/api/history", get(history))
        .route("/api/chart-history", get(chart_history))
        .route("/api/cache-history", get(cache_history))
        .route("/api/limits", get(limits_get).put(limits_put))
        .route("/api/usage", get(usage))
        .route("/api/priority", get(priority_get).put(priority_put))
        .route("/api/run-limit", get(run_limit_get).put(run_limit_put))
        .fallback(get(asset))
        .with_state(s);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn state(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.state())
}

async fn usage(
    State(s): State<Arc<Server>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let (from, to, by) = match crate::usage::query(&params, super::now_ms()) {
        Ok(q) => q,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    let path = s.cfg.state_dir.join("jobs.jsonl");
    let titles = s.inner.lock().expect("server lock").titles.clone();
    let built = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path).ok();
        let mut resolve = |cwd: &str| crate::project::resolve(std::path::Path::new(cwd));
        let mut u = match file {
            Some(f) => {
                crate::usage::aggregate(std::io::BufReader::new(f), from, to, by, &mut resolve)
            }
            None => crate::usage::Usage {
                from_ms: from,
                to_ms: to,
                by,
                ..Default::default()
            },
        };
        if by == crate::usage::By::Session {
            for r in &mut u.rows {
                r.title = titles.get(&r.key).cloned().unwrap_or_default();
            }
        }
        u
    })
    .await;
    match built {
        Ok(u) => axum::Json(u).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("slot: {e}\n")).into_response(),
    }
}

async fn limits_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.limits())
}

/// Why a settings change is refused, if it is: a browser page from another
/// origin, or a body that is not JSON (a form or `text/plain` post needs no
/// CORS preflight, so it must not be accepted).
pub(super) fn refuse_change(headers: &HeaderMap, port: u16) -> Option<(StatusCode, &'static str)> {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !super::limits::origin_allowed(origin, port) {
        return Some((StatusCode::FORBIDDEN, "slot: request from another origin\n"));
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().starts_with("application/json"));
    if !json {
        return Some((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "slot: Content-Type must be application/json\n",
        ));
    }
    None
}

async fn limits_put(State(s): State<Arc<Server>>, headers: HeaderMap, body: Bytes) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: super::limits::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    let max = super::limits::MAX_HEADROOM;
    for (name, v) in [
        ("headroom_percent", req.headroom_percent),
        ("call_headroom_percent", req.call_headroom_percent),
    ] {
        if v.is_some_and(|v| v > max) {
            return (
                StatusCode::BAD_REQUEST,
                format!("slot: {name} must be 0 to {max}\n"),
            )
                .into_response();
        }
    }
    axum::Json(s.update_limits(req).await).into_response()
}

pub(super) async fn priority_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.priority_view())
}

pub(super) async fn priority_put(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: super::priority::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    match s.set_priority(&req) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}

pub(super) async fn run_limit_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.run_limit_view())
}

pub(super) async fn run_limit_put(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: crate::run_limit::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    match s.set_run_limit(&req) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}

async fn metrics(State(s): State<Arc<Server>>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        s.metrics_text(),
    )
}

async fn history(State(s): State<Arc<Server>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        s.history_json(),
    )
}

#[derive(Deserialize, Serialize)]
struct ChartSample {
    t: i64,
    host_cores: f64,
    waiting: f64,
    n: u32,
}

async fn chart_history(
    State(s): State<Arc<Server>>,
) -> Result<axum::Json<Vec<ChartSample>>, StatusCode> {
    let path = s.cfg.state_dir.join("census.jsonl");
    let samples = tokio::task::spawn_blocking(move || read_chart_history(&path, super::now_ms()))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(axum::Json(samples))
}

fn read_chart_history(path: &FsPath, now: i64) -> io::Result<Vec<ChartSample>> {
    let first = now.saturating_sub(CHART_WINDOW_MS);
    let mut samples = read_tail(path, first, CHART_TAIL_START, |s: &ChartSample| {
        (s.n > 0 && s.t > 0 && s.host_cores >= 0.0 && s.waiting >= 0.0).then_some(s.t)
    })?;
    samples.retain(|s| (first..=now).contains(&s.t));
    samples.sort_by_key(|s| s.t);
    Ok(samples)
}

/// The complete lines at the end of the log at `path` that read as a `T`
/// with a time (`time`), in the file's order; none when there is no file.
///
/// Most of a log is outside the chart: the last `start` bytes are read, and
/// twice as many (up to [`CHART_TAIL_LIMIT`]) only while they hold no record
/// older than `first`.
fn read_tail<T: serde::de::DeserializeOwned>(
    path: &FsPath,
    first: i64,
    start: u64,
    time: impl Fn(&T) -> Option<i64>,
) -> io::Result<Vec<T>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let mut amount = start.max(1).min(len);
    loop {
        let offset = len - amount;
        file.seek(SeekFrom::Start(offset))?;
        let mut tail = Vec::new();
        (&mut file).take(amount).read_to_end(&mut tail)?;
        // The first line of a tail that starts inside the file is cut.
        let begin = if offset == 0 {
            0
        } else {
            tail.iter()
                .position(|&b| b == b'\n')
                .map_or(tail.len(), |p| p + 1)
        };
        let mut earlier = false;
        let records: Vec<T> = tail[begin..]
            .split_inclusive(|&b| b == b'\n')
            .filter_map(|line| {
                // The writer ends each record with a newline.
                if line.last() != Some(&b'\n') {
                    return None;
                }
                let record: T = serde_json::from_slice(line).ok()?;
                earlier |= time(&record)? < first;
                Some(record)
            })
            .collect();
        if earlier || offset == 0 || amount >= CHART_TAIL_LIMIT {
            return Ok(records);
        }
        amount = (amount * 2).min(len).min(CHART_TAIL_LIMIT);
    }
}

async fn cache_history(
    State(s): State<Arc<Server>>,
) -> Result<axum::Json<Vec<CacheMinute>>, StatusCode> {
    let path = s.cfg.state_dir.join("cache.jsonl");
    let minutes = tokio::task::spawn_blocking(move || {
        read_cache_history(&path, super::now_ms(), CHART_TAIL_START)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(axum::Json(minutes))
}

/// The compiler calls per minute of the chart window, from the counters at
/// the end of `cache.jsonl` ([`cache_minutes`]). The reading before the
/// window is what the first one in it is compared with; a port that has
/// none in the tail starts counting at its first reading.
fn read_cache_history(path: &FsPath, now: i64, start: u64) -> io::Result<Vec<CacheMinute>> {
    let first = now.saturating_sub(CHART_WINDOW_MS);
    let mut records = read_tail(path, first, start, |c: &Counters| {
        (c.t > 0 && c.port != 0).then_some(c.t)
    })?;
    records.sort_by_key(|c| c.t);
    let mut minutes = cache_minutes(&records);
    minutes.retain(|m| (first..=now).contains(&m.t));
    Ok(minutes)
}

async fn events(
    State(s): State<Arc<Server>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = BroadcastStream::new(s.subscribe());
    let stream = rx.filter_map(|m| async move { m.ok().map(|d| Ok(SseEvent::default().data(d))) });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn output(State(s): State<Arc<Server>>, Path(id): Path<i64>) -> Response {
    let ring = match s.output(id) {
        super::Output::NoJob => return StatusCode::NOT_FOUND.into_response(),
        super::Output::Dropped => {
            let end = SseEvent::default()
                .event("end")
                .data(r#"{"msg":"output not kept for this job"}"#);
            return Sse::new(stream::iter([Ok::<_, Infallible>(end)])).into_response();
        }
        super::Output::Kept(r) => r,
    };
    let (tail, rx) = ring.follow();
    let to_event = |c: crate::types::Chunk| {
        Ok::<_, Infallible>(SseEvent::default().data(serde_json::to_string(&c).unwrap_or_default()))
    };
    let head = stream::iter(tail.into_iter().map(to_event));
    let live = UnboundedReceiverStream::new(rx).map(to_event);
    let end = stream::iter([Ok(SseEvent::default().event("end").data("{}"))]);
    Sse::new(head.chain(live).chain(end))
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn asset(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // An unknown API path is a 404, not the UI page, so a newer client can
    // tell that this daemon lacks it.
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "slot: no such API path\n").into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(f) = UI.get_file(path).or_else(|| UI.get_file("index.html")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = match f.path().extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], f.contents()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;

    fn line(t: i64, port: u16, hits: u64, misses: u64) -> String {
        let c = Counters {
            t,
            port,
            rust_hits: hits,
            rust_misses: misses,
            ..Counters::default()
        };
        serde_json::to_string(&c).unwrap() + "\n"
    }

    #[test]
    fn cache_history_is_read_from_the_tail_of_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.jsonl");
        assert!(
            read_cache_history(&path, NOW, 4096).unwrap().is_empty(),
            "no log yet"
        );

        // A second server's only earlier reading stands at the start of the
        // file, far before the tail.
        let mut text = line(NOW - 12 * 60 * MIN, 4303, 1_000, 0);
        // 200 readings from before the window, the last one 7 h ago.
        for k in 1..=200 {
            text += &line(
                NOW - 7 * 60 * MIN - (200 - k) * MIN,
                4226,
                k as u64 * 3,
                k as u64,
            );
        }
        // 40 readings in the window, the last one now: more than 4096 bytes,
        // so the first tail holds no reading from before the window.
        for i in 1..=40 {
            let n = 200 + i as u64;
            text += &line(NOW - (40 - i) * MIN, 4226, n * 3, n);
            if i == 20 {
                text += "not json\n";
                text += &line(NOW - 20 * MIN, 4303, 1_500, 0);
            }
        }
        let in_window = 40 * line(NOW, 4226, 720, 240).len() as u64;
        assert!(in_window > 4096 && in_window < 8192, "{in_window}");
        // A line the writer has not finished is no record.
        text += r#"{"t":1800000000000,"port":4226,"rust_hits":999999"#;
        std::fs::write(&path, &text).unwrap();
        assert!(
            text.len() as u64 > 2 * 8192,
            "the tail is a part of the file"
        );

        let tail = read_cache_history(&path, NOW, 4096).unwrap();
        assert_eq!(
            tail.len(),
            40,
            "the first one is compared with the reading before the window"
        );
        for (i, m) in tail.iter().enumerate() {
            assert_eq!(m.t, NOW - (39 - i as i64) * MIN);
            assert_eq!((m.hits, m.misses, m.not_cached), (3, 1, 0), "{m:?}");
        }

        // The whole file: the second server's reading in the window has one
        // before it now.
        let whole = read_cache_history(&path, NOW, CHART_TAIL_START).unwrap();
        assert_eq!(whole.len(), 40);
        let hits: Vec<u64> = whole.iter().map(|m| m.hits).collect();
        assert_eq!(hits[19], 3 + 500);
        assert_eq!(hits.iter().sum::<u64>(), 40 * 3 + 500);

        // A reading after `now` (a clock set back) is left out.
        assert_eq!(
            read_cache_history(&path, NOW - MIN, 4096).unwrap().len(),
            39
        );
    }

    #[test]
    fn chart_history_keeps_the_valid_samples_of_the_window_in_time_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("census.jsonl");
        assert!(read_chart_history(&path, NOW).unwrap().is_empty());
        let sample = |t: i64, n: u32| {
            format!("{{\"t\":{t},\"host_cores\":2.5,\"waiting\":1.0,\"n\":{n}}}\n")
        };
        let text = [
            sample(NOW - 7 * 60 * MIN, 10),
            sample(NOW - 20_000, 10),
            sample(NOW - 30_000, 10),
            sample(NOW - 10_000, 0),
            "{\"t\":1,\"host_cores\":-1.0,\"waiting\":0.0,\"n\":10}\n".into(),
            sample(NOW + 10_000, 10),
            format!("{{\"t\":{NOW},\"host_cores\":1.0"),
        ]
        .concat();
        std::fs::write(&path, text).unwrap();
        let got: Vec<i64> = read_chart_history(&path, NOW)
            .unwrap()
            .iter()
            .map(|s| s.t)
            .collect();
        assert_eq!(got, [NOW - 30_000, NOW - 20_000]);
    }
}
