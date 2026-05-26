//! Awk-friendly per-line log formatter.

use std::fmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{
    format::{DefaultFields, Writer},
    FmtContext, FormatEvent, FormatFields,
};
use tracing_subscriber::layer::Layer;
use tracing_subscriber::registry::LookupSpan;

/// Build the per-line formatting layer writing to stderr.
pub fn build<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    build_with_writer(std::io::stderr as fn() -> std::io::Stderr)
}

/// Build the per-line formatting layer with a custom writer.
pub fn build_with_writer<S, W>(make_writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + 'static,
{
    tracing_subscriber::fmt::layer()
        .event_format(LineFormat)
        .fmt_fields(DefaultFields::new())
        .with_ansi(false)
        .with_writer(make_writer)
}

/// Awk-friendly event formatter.
///
/// Output format:
/// ```text
/// LEVEL group=span_name field1=value field2=value "message"
/// ```
pub struct LineFormat;

impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        write!(writer, "{} ", meta.level())?;

        // Print span context: group=NAME and any span fields.
        if let Some(scope) = ctx.event_scope() {
            for span in scope {
                write!(writer, "group={} ", span.name())?;
                if let Some(fields) = span
                    .extensions()
                    .get::<tracing_subscriber::fmt::FormattedFields<N>>()
                {
                    if !fields.is_empty() {
                        write!(writer, "{} ", fields)?;
                    }
                }
            }
        }

        // Print event fields explicitly as key=value pairs.
        let mut visitor = crate::logging::event_fmt::FieldVisitor::new();
        event.record(&mut visitor);

        // Write non-message fields first.
        for (k, v) in &visitor.fields {
            write!(writer, "{}={} ", k, v)?;
        }

        // Write message last, always quoted and sanitized.
        if !visitor.message.is_empty() {
            let sanitized = crate::logging::event_fmt::sanitize_message(&visitor.message);
            write!(writer, "\"{}\"", sanitized)?;
        }

        writeln!(writer)
    }
}
