//! OpenTelemetry setup for Huxley (design doc §13.3, ADR-015).
//!
//! - Traces, metrics, and logs are exported over OTLP (gRPC) when an endpoint
//!   is configured. Without one, nothing is exported.
//! - Logs always go to stdout too (JSON by default, or pretty for local dev),
//!   carrying trace and span IDs so they can be correlated.
//! - W3C `traceparent`/`tracestate` is installed as the global propagator.

use anyhow::Context as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use serde::Deserialize;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _};

/// Stdout log format.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Pretty,
}

/// Telemetry configuration (`[telemetry]` in `huxley.toml`, or `HUXLEY_TELEMETRY__*`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// `service.name` resource attribute.
    pub service_name: String,
    /// OTLP gRPC endpoint, e.g. `http://otel-collector:4317`. `None` disables export.
    pub otlp_endpoint: Option<String>,
    /// Ratio of new root traces to sample (parent-based), `0.0..=1.0`.
    pub sample_ratio: f64,
    /// Stdout log format.
    pub log_format: LogFormat,
    /// `EnvFilter` directive, e.g. `info,huxley=debug`. `RUST_LOG` overrides it.
    pub log_filter: String,
    /// `deployment.environment.name` resource attribute.
    pub environment: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            service_name: "huxley".to_owned(),
            otlp_endpoint: None,
            sample_ratio: 1.0,
            log_format: LogFormat::Json,
            log_filter: "info".to_owned(),
            environment: "development".to_owned(),
        }
    }
}

/// Keeps telemetry providers alive. Call [`TelemetryGuard::shutdown`] on exit to
/// flush buffered spans, metrics, and logs.
#[derive(Debug, Default)]
pub struct TelemetryGuard {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
    logger: Option<SdkLoggerProvider>,
}

impl TelemetryGuard {
    pub fn shutdown(self) {
        if let Some(p) = self.tracer
            && let Err(e) = p.shutdown()
        {
            eprintln!("telemetry: tracer shutdown failed: {e}");
        }
        if let Some(p) = self.meter
            && let Err(e) = p.shutdown()
        {
            eprintln!("telemetry: meter shutdown failed: {e}");
        }
        if let Some(p) = self.logger
            && let Err(e) = p.shutdown()
        {
            eprintln!("telemetry: logger shutdown failed: {e}");
        }
    }
}

/// Reads W3C trace context from HTTP headers.
struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl opentelemetry::propagation::Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}

/// Makes `span` a child of the trace in `headers` (`traceparent`), if present.
pub fn set_parent_from_headers(span: &tracing::Span, headers: &http::HeaderMap) {
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    let parent =
        global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)));
    // Fails only when the OpenTelemetry layer is not installed (export disabled).
    let _ = span.set_parent(parent);
}

/// Installs the global tracing subscriber and, if configured, the OTLP exporters.
///
/// `role` is recorded as the `huxley.role` resource attribute (e.g. `api`, `worker`, `all`).
pub fn init(config: &TelemetryConfig, role: &str) -> anyhow::Result<TelemetryGuard> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.log_filter))
        .context("invalid log filter")?;

    let stdout = match config.log_format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer().pretty().boxed(),
    };

    // An empty value (e.g. an unset compose variable) also means "disabled".
    let Some(endpoint) = config
        .otlp_endpoint
        .as_deref()
        .filter(|e| !e.trim().is_empty())
    else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout)
            .try_init()
            .context("failed to install tracing subscriber")?;
        return Ok(TelemetryGuard::default());
    };

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attributes([
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("deployment.environment.name", config.environment.clone()),
            KeyValue::new("huxley.role", role.to_owned()),
            // Tells processes apart (Prometheus' `instance`): the pod or host
            // name, with the process id for several on one host.
            KeyValue::new(
                "service.instance.id",
                format!(
                    "{}-{}",
                    std::env::var("HOSTNAME").unwrap_or_else(|_| "huxley".into()),
                    std::process::id()
                ),
            ),
        ])
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            config.sample_ratio.clamp(0.0, 1.0),
        ))))
        .with_batch_exporter(
            SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()
                .context("failed to build OTLP span exporter")?,
        )
        .build();

    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_periodic_exporter(
            MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()
                .context("failed to build OTLP metric exporter")?,
        )
        .build();

    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(
            LogExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()
                .context("failed to build OTLP log exporter")?,
        )
        .build();

    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());

    let otel_traces = tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("huxley"));
    let otel_logs = OpenTelemetryTracingBridge::new(&logger_provider);

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout)
        .with(otel_traces)
        .with(otel_logs)
        .try_init()
        .context("failed to install tracing subscriber")?;

    Ok(TelemetryGuard {
        tracer: Some(tracer_provider),
        meter: Some(meter_provider),
        logger: Some(logger_provider),
    })
}
