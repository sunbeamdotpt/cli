//! Awk-friendly per-line log formatter.
//!
//! Output format (logrus-style):
//! ```text
//! time="2026-05-26T15:42:29.123+01:00" level=INFO msg="message" group=span_name field1=value field2=value
//! ```

use std::fmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{
    FmtContext, FormatEvent, FormatFields,
    format::{DefaultFields, Writer},
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
        // ── Timestamp ──────────────────────────────────────────────────────────
        let now = chrono::Local::now();
        write!(
            writer,
            "time=\"{}\" ",
            now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        )?;

        // ── Level ──────────────────────────────────────────────────────────────
        let meta = event.metadata();
        write!(writer, "level={} ", meta.level())?;

        // ── Event fields (extract message + other fields) ──────────────────────
        let mut visitor = crate::logging::event_fmt::FieldVisitor::new();
        event.record(&mut visitor);

        // Write message as msg="..."
        if !visitor.message.is_empty() {
            let sanitized = crate::logging::event_fmt::sanitize_message(&visitor.message);
            write!(writer, "msg=\"{}\" ", sanitized)?;
        }

        // ── Span context: group=NAME and span fields ───────────────────────────
        if let Some(scope) = ctx.event_scope() {
            for span in scope {
                write!(writer, "group={} ", span.name())?;
                if let Some(fields) = span
                    .extensions()
                    .get::<tracing_subscriber::fmt::FormattedFields<N>>()
                    && !fields.is_empty()
                {
                    write!(writer, "{} ", fields)?;
                }
            }
        }

        // ── Remaining event fields (excluding message which was already emitted) ─
        for (k, v) in &visitor.fields {
            write!(writer, "{}={} ", k, v)?;
        }

        writeln!(writer)
    }
}
