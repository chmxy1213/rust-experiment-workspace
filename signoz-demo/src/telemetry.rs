use anyhow::Result;
use opentelemetry::{
    Context, KeyValue, global,
    trace::{TraceContextExt as _, TracerProvider as _},
};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;

pub struct TelemetryGuard {
    provider: SdkTracerProvider,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let _ = self.provider.shutdown();
    }
}

pub fn init(service_name: &str, machine_id: &str) -> Result<TelemetryGuard> {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:4317".to_string());

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(
            Resource::builder_empty()
                .with_service_name(service_name.to_string())
                .with_attribute(KeyValue::new("machine.id", machine_id.to_string()))
                .build(),
        )
        .build();

    global::set_text_map_propagator(TraceContextPropagator::new());

    let tracer = provider.tracer(service_name.to_string());
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(telemetry_layer);

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(TelemetryGuard { provider })
}

pub fn inject_current_context(headers: &mut reqwest::header::HeaderMap) {
    let cx = Context::current();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderInjector(headers));
    });
}

pub fn extract_parent_context(headers: &axum::http::HeaderMap) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

pub fn current_trace_id() -> String {
    let cx = tracing::Span::current().context();
    let span = cx.span();
    span.span_context().trace_id().to_string()
}
