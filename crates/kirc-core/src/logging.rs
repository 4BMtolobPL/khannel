use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[cfg(debug_assertions)]
pub(super) fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // 콘솔 로그
    let console_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_target(false)
        .with_level(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_span_events(FmtSpan::CLOSE)
        .pretty();

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .init();
}

#[cfg(not(debug_assertions))]
pub(super) fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // 파일 로그
    // + log lotation 추가해서 파일 로그까지
    /*let file_appender = tracing_appender::rolling::daily("logs", "app.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .json()
        .with_target(true);*/

    // 콘솔 로그
    // fmt::layer().json().with_target(true).with_thread_ids(true).with_file(true).with_line_number(true)
    let console_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_target(false)
        .with_level(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_span_events(FmtSpan::CLOSE)
        .json();

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .init();
}
