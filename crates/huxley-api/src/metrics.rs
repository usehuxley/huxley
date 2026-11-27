//! HTTP and rate limit metrics (OpenTelemetry; ADR-091). No-ops unless
//! telemetry export is on. Names follow the OpenTelemetry semantic
//! conventions where there is one; Prometheus sees `.` as `_`, with units
//! and `_total` appended.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};

/// Seconds: the semantic conventions' boundaries for HTTP durations, plus
/// 30s for slow exports and synchronous webhooks.
const DURATION_BOUNDARIES: [f64; 15] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 30.0,
];

fn request_duration() -> &'static Histogram<f64> {
    static H: OnceLock<Histogram<f64>> = OnceLock::new();
    H.get_or_init(|| {
        global::meter("huxley")
            .f64_histogram("http.server.request.duration")
            .with_unit("s")
            .with_description("How long API requests took, by route and status")
            .with_boundaries(DURATION_BOUNDARIES.to_vec())
            .build()
    })
}

fn rejections() -> &'static Counter<u64> {
    static C: OnceLock<Counter<u64>> = OnceLock::new();
    C.get_or_init(|| {
        global::meter("huxley")
            .u64_counter("huxley.rate_limit.rejected")
            .with_description("Requests refused by a rate limit, by scope")
            .build()
    })
}

/// Records each request's duration with its route template (never the raw
/// path, which would make a series per id).
pub async fn track(request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let method = request.method().as_str().to_owned();
    let start = Instant::now();
    let response = next.run(request).await;
    request_duration().record(
        start.elapsed().as_secs_f64(),
        &[
            KeyValue::new("http.request.method", method),
            KeyValue::new("http.route", route),
            KeyValue::new(
                "http.response.status_code",
                i64::from(response.status().as_u16()),
            ),
        ],
    );
    response
}

/// A request refused by the `scope` rate limit.
pub fn rate_limited(scope: &'static str) {
    rejections().add(1, &[KeyValue::new("scope", scope)]);
}
