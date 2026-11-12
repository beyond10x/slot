//! Web UI and its JSON/SSE API.

use std::convert::Infallible;
use std::net::SocketAddr;
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
use tokio_stream::wrappers::{BroadcastStream, UnboundedReceiverStream};

use super::Server;

static UI: Dir<'_> = include_dir!("$OUT_DIR/ui");

pub(super) async fn serve(s: Arc<Server>, addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route("/api/state", get(state))
        .route("/api/events", get(events))
        .route("/api/jobs/{id}/output", get(output))
        .route("/metrics", get(metrics))
        .route("/api/history", get(history))
        .route("/api/limits", get(limits_get).put(limits_put))
        .fallback(get(asset))
        .with_state(s);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn state(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.state())
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
    if req.headroom_percent > super::limits::MAX_HEADROOM {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "slot: headroom_percent must be 0 to {}\n",
                super::limits::MAX_HEADROOM
            ),
        )
            .into_response();
    }
    axum::Json(s.set_headroom(req.headroom_percent).await).into_response()
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
