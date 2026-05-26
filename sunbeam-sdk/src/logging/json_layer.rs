//! NDJSON structured log layer.

use tracing::Subscriber;
use tracing_subscriber::layer::Layer;

/// Build the JSON formatting layer writing to stderr.
pub fn build<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    build_with_writer(std::io::stderr as fn() -> std::io::Stderr)
}

/// Build the JSON formatting layer with a custom writer.
pub fn build_with_writer<S, W>(make_writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + 'static,
{
    tracing_subscriber::fmt::layer()
        .json()
        .with_writer(make_writer)
}
