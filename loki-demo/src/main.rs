use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
};
use tracing::{info, info_span, instrument, warn};
use tracing_subscriber::layer::SubscriberExt;
use url::Url;

const LOKI_APP_NAME: &str = "demo";
const OTLP_SERVICE_NAME: &str = "demo";
const TEMPO_URL: &str = "http://127.0.0.1:4317";
const LOKI_URL: &str = "http://127.0.0.1:3100";

// 使用 #[instrument] 宏自动为函数创建一个 span
#[instrument(skip(data), fields(task_id = %user_id))]
async fn task(user_id: u64, data: &str) {
    info!("Starting to process data for user");

    // 模拟一些处理时间
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 手动创建一个嵌套的 span
    let db_span = info_span!("database_operation", table = "users", operation = "update");
    // 进入 span 的上下文
    let _enter = db_span.enter();

    info!("Updating user record in database");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    if data.is_empty() {
        warn!("Received empty data for user");
    } else {
        info!(data_length = data.len(), "Successfully updated user data");
    }

    // 离开 db_span (当 _enter 离开作用域时自动发生)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 读取环境变量中的 IP, TASK_ID
    let ip = std::env::var("IP").unwrap_or_else(|_| "unknown-ip".to_string());
    let task_id = std::env::var("TASK_ID").unwrap_or_else(|_| "unknown-task".to_string());

    // 0. 配置 OpenTelemetry (Tempo)
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(TEMPO_URL)
        .build()?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(
            Resource::builder_empty()
                .with_service_name(OTLP_SERVICE_NAME)
                .with_attribute(KeyValue::new("ip", ip.clone()))
                .with_attribute(KeyValue::new("task", task_id.clone()))
                .build(),
        )
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());
    let tracer = provider.tracer(OTLP_SERVICE_NAME);

    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // 1. 配置 Loki 的 URL
    let loki_url = Url::parse(LOKI_URL).unwrap();

    // 2. 创建 Loki layer
    // 这里我们添加一些默认的 labels，比如 application name 和 machine_id
    let (loki_layer, loki_task) = tracing_loki::builder()
        .label("app", LOKI_APP_NAME)?
        .label("ip", &ip)?
        .label("task", &task_id)?
        .build_url(loki_url)?;

    // 3. 启动一个后台任务来发送日志到 Loki
    // tracing_loki 使用后台任务异步发送日志，避免阻塞主线程
    let loki_handle = tokio::spawn(loki_task);

    // 4. 创建一个控制台输出的 layer (可选，方便在终端也看到日志)
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true);

    // 5. 组合 layers 并初始化全局 subscriber
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info")) // 设置日志级别
        .with(fmt_layer)
        .with(loki_layer)
        .with(telemetry_layer); // 添加 OpenTelemetry layer
    let _guard = tracing::subscriber::set_default(subscriber);

    // 6. 测试发送不同级别的日志
    info!("Application started!");

    task(42, "some important data").await;

    info!("Application shutting down");

    // 关闭 OpenTelemetry tracer provider，确保所有 span 都被发送
    let _ = provider.shutdown();

    // 显式丢弃 guard，这会卸载 subscriber 并关闭 loki_layer 的 channel
    drop(_guard);

    // 等待 loki_task 处理完剩余的日志并退出
    let _ = loki_handle.await;

    Ok(())
}
