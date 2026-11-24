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
const CACHE_DAY_MS: i64 = 24 * 60 * 60 * 1_000;
const CHART_TAIL_START: u64 = 1 << 20;
const CHART_TAIL_LIMIT: u64 = 32 << 20;

pub(super) async fn serve(s: Arc<Server>, addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route("/api/state", get(state))
        .route("/api/nodes", get(nodes))
        .route("/api/events", get(events))
        .route("/api/jobs/{id}/output", get(output))
        .route("/api/jobs/{id}/output-tail", get(output_tail))
        .route("/metrics", get(metrics))
        .route("/api/history", get(history))
        .route("/api/chart-history", get(chart_history))
        .route("/api/cache-history", get(cache_history))
        .route("/api/cache-day", get(cache_day))
        .route("/api/limits", get(limits_get).put(limits_put))
        .route("/api/usage", get(usage))
        .route("/api/priority", get(priority_get).put(priority_put))
        .route("/api/run-limit", get(run_limit_get).put(run_limit_put))
        .route(
            "/api/job-parallel",
            get(job_parallel_get).put(job_parallel_put),
        )
        .route(
            "/api/evict-below",
            get(evict_below_get).put(evict_below_put),
        )
        .route("/api/cpu-limit", get(cpu_limit_get).put(cpu_limit_put))
        .route("/api/admission", get(admission_get).put(admission_put))
        .fallback(fallback)
        .with_state(s);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn state(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.state())
}

/// `GET /api/nodes`: every machine jobs run on.
pub(super) async fn nodes(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.nodes())
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

pub(super) async fn job_parallel_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.job_parallel_view())
}

pub(super) async fn job_parallel_put(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: crate::job_parallel::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    match s.set_job_parallel(&req) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}

pub(super) async fn evict_below_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.evict_below_view())
}

pub(super) async fn evict_below_put(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: crate::evict::BelowRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    match s.set_evict_below(&req) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}

pub(super) async fn cpu_limit_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.cpu_limit_view())
}

pub(super) async fn cpu_limit_put(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: crate::cpu_limit::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    match s.set_cpu_limit(&req) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}

pub(super) async fn admission_get(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.admission_view())
}

pub(super) async fn admission_put(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let port = s.cfg.http.map_or(0, |a| a.port());
    if let Some(r) = refuse_change(&headers, port) {
        return r.into_response();
    }
    let req: crate::shares::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("slot: {e}\n")).into_response(),
    };
    match s.set_admission(&req) {
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

async fn cache_day(State(s): State<Arc<Server>>) -> Result<axum::Json<CacheMinute>, StatusCode> {
    let path = s.cfg.state_dir.join("cache.jsonl");
    let day = tokio::task::spawn_blocking(move || {
        read_cache_day(&path, super::now_ms(), CHART_TAIL_START)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(axum::Json(day))
}

/// The compiler calls per minute of the chart window, from the counters at
/// the end of `cache.jsonl` ([`cache_minutes`]).
fn read_cache_history(path: &FsPath, now: i64, start: u64) -> io::Result<Vec<CacheMinute>> {
    read_cache_minutes(path, now.saturating_sub(CHART_WINDOW_MS), now, start)
}

/// The compiler calls of the last 24 hours, summed into one element at `now`:
/// the minutes of [`read_cache_minutes`] over that window. The read is
/// bounded like the chart's, so a log of more than [`CHART_TAIL_LIMIT`]
/// bytes in a day gives the calls of its last bytes.
fn read_cache_day(path: &FsPath, now: i64, start: u64) -> io::Result<CacheMinute> {
    let minutes = read_cache_minutes(path, now.saturating_sub(CACHE_DAY_MS), now, start)?;
    let mut day = CacheMinute {
        t: now,
        ..CacheMinute::default()
    };
    for m in minutes {
        day.hits += m.hits;
        day.misses += m.misses;
        day.not_cached += m.not_cached;
        for (reason, n) in m.reasons {
            *day.reasons.entry(reason).or_default() += n;
        }
    }
    Ok(day)
}

/// The compiler calls per minute from `first` to `now`, from the counters
/// at the end of `cache.jsonl` ([`cache_minutes`]). The reading before the
/// window is what the first one in it is compared with; a port that has
/// none in the tail starts counting at its first reading.
fn read_cache_minutes(
    path: &FsPath,
    first: i64,
    now: i64,
    start: u64,
) -> io::Result<Vec<CacheMinute>> {
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

/// `?node=<name>` of `GET /api/jobs/{id}/output`, `?from=<n>` of
/// `GET /api/jobs/{id}/output-tail`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OutputQuery {
    node: Option<String>,
    from: u64,
}

/// `GET /api/jobs/{id}/output-tail?from=<n>`: the kept output of a job of
/// this daemon from chunk `n` on, as one JSON object; what a node's
/// `output` step prints for its endpoint.
async fn output_tail(
    State(s): State<Arc<Server>>,
    Path(id): Path<i64>,
    axum::extract::Query(q): axum::extract::Query<OutputQuery>,
) -> Response {
    match s.output(id) {
        super::Output::NoJob => StatusCode::NOT_FOUND.into_response(),
        super::Output::Dropped => axum::Json(crate::types::OutputTail {
            ended: true,
            ..Default::default()
        })
        .into_response(),
        super::Output::Kept(r) => axum::Json(r.since(q.from)).into_response(),
    }
}

/// `GET /api/jobs/{id}/output`: the output of a job as server-sent
/// events, its kept tail and then what it writes, and an `end` event. With
/// `?node=<name>` of a remote node the job is that node's: the endpoint
/// reads its output from the node with the node's `output` step.
async fn output(
    State(s): State<Arc<Server>>,
    Path(id): Path<i64>,
    axum::extract::Query(q): axum::extract::Query<OutputQuery>,
) -> Response {
    if let Some(node) = q.node.filter(|n| n != crate::types::LOCAL_NODE) {
        let Some(conf) = s.remotes.conf(&node) else {
            return (StatusCode::NOT_FOUND, "slot: no such node\n").into_response();
        };
        let relay = super::nodes::relay_output(
            move |from| conf.output_command(id, from),
            id,
            node,
            super::nodes::OUTPUT_EVERY,
        );
        let events = relay.map(|r| {
            Ok::<_, Infallible>(match r {
                super::nodes::Relay::Chunk(c) => {
                    SseEvent::default().data(serde_json::to_string(&c).unwrap_or_default())
                }
                super::nodes::Relay::End(msg) if msg.is_empty() => {
                    SseEvent::default().event("end").data("{}")
                }
                super::nodes::Relay::End(msg) => SseEvent::default()
                    .event("end")
                    .data(serde_json::json!({ "msg": msg }).to_string()),
            })
        });
        return Sse::new(events)
            .keep_alive(KeepAlive::default())
            .into_response();
    }
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

/// Every path no route has: an unknown `/api/` path answers 404 whatever
/// the method, so a newer client can tell an older daemon from a failed
/// call (`PUT /api/admission` on a daemon without it); other paths serve
/// the UI to `GET` and `HEAD`.
async fn fallback(method: axum::http::Method, uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "slot: no such API path\n").into_response();
    }
    if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    asset(&uri)
}

fn asset(uri: &axum::http::Uri) -> Response {
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
    fn cache_day_sums_the_calls_of_the_last_24_hours() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.jsonl");
        let empty = read_cache_day(&path, NOW, 4096).unwrap();
        assert_eq!(
            empty,
            CacheMinute {
                t: NOW,
                ..CacheMinute::default()
            },
            "no log yet: no calls"
        );

        let reading = |t: i64, port: u16, hits: u64, misses: u64, reasons: &[(&str, u64)]| {
            let c = Counters {
                t,
                port,
                rust_hits: hits,
                rust_misses: misses,
                not_cached: reasons.iter().map(|(r, n)| (r.to_string(), *n)).collect(),
                ..Counters::default()
            };
            serde_json::to_string(&c).unwrap() + "\n"
        };
        // A second server: one reading 30 h ago, one an hour ago.
        let mut text = reading(NOW - 30 * 60 * MIN, 4303, 1_000, 0, &[]);
        // A reading every 10 minutes from 25 h ago to now; the server starts
        // again at k = 100, so its counters fall and count from 0.
        for k in 0..=150_i64 {
            let n = if k < 100 { k } else { k - 100 } as u64;
            let t = NOW - (150 - k) * 10 * MIN;
            text += &reading(t, 4226, 3 * n, n, &[("incremental", 2 * n)]);
            if k == 144 {
                text += &reading(NOW - 60 * MIN, 4303, 1_500, 0, &[("crate-type", 7)]);
            }
        }
        // A reading after `now` (a clock set back) is left out.
        text += &reading(NOW + MIN, 4226, 1_000_000, 0, &[]);
        std::fs::write(&path, &text).unwrap();

        let day = read_cache_day(&path, NOW, 4096).unwrap();
        // k = 6 is 24 h ago and is compared with k = 5; the restart at
        // k = 100 gives no difference: 144 differences of 3, 1 and 2.
        assert_eq!(day.t, NOW);
        assert_eq!(day.hits, 144 * 3 + 500, "{day:?}");
        assert_eq!(day.misses, 144);
        assert_eq!(day.not_cached, 144 * 2 + 7);
        assert_eq!(
            day.reasons,
            [
                ("crate-type".to_string(), 7),
                ("incremental".to_string(), 288)
            ]
            .into()
        );
        let history: u64 = read_cache_history(&path, NOW, 4096)
            .unwrap()
            .iter()
            .map(|m| m.hits)
            .sum();
        assert!(history < day.hits, "the day reaches past the chart window");
        assert_eq!(
            serde_json::to_value(&day).unwrap(),
            serde_json::json!({
                "t": NOW, "hits": 932, "misses": 144, "not_cached": 295,
                "reasons": {"crate-type": 7, "incremental": 288},
            }),
            "the shape of one element of /api/cache-history"
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
