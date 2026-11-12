//! Web UI and its JSON/SSE API.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
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
        .fallback(get(asset))
        .with_state(s);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn state(State(s): State<Arc<Server>>) -> impl IntoResponse {
    axum::Json(s.state())
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
