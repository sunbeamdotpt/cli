//! Unified tracing-based logging subsystem for the Sunbeam CLI.
//!
//! Supports three output modes:
//! - `line`   — awk-friendly, parseable single-line output (default).
//! - `json`   — NDJSON structured logs.
//! - `threaded` — grouped concurrent output with per-task scrollback (BuildKit-style).

use crate::error::Result;
use clap::ValueEnum;
use std::io::IsTerminal;
use tracing::Subscriber;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub mod event_fmt;
pub mod json_layer;
pub mod line_layer;
pub mod span_state;
pub mod threaded_layer;

/// Output mode for the CLI logger.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum LogMode {
    /// Awk-friendly single-line output.
    #[default]
    Line,
    /// NDJSON structured output.
    Json,
    /// Grouped concurrent output with per-task scrollback.
    Threaded,
}

/// Initialize the global tracing subscriber for the given mode.
///
/// Must be called once before any spans or events are emitted.
///
/// `level_override` takes precedence over the default filter but loses to
/// the `RUST_LOG` environment variable.
pub fn init_subscriber(mode: LogMode, level_override: Option<&str>) -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            let filter = level_override.unwrap_or(
                "sunbeam=info,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,warn",
            );
            tracing_subscriber::EnvFilter::new(filter)
        });

    match mode {
        LogMode::Line => {
            let layer = line_layer::build();
            tracing_subscriber::registry()
                .with(env_filter)
                .with(layer)
                .init();
        }
        LogMode::Json => {
            let layer = json_layer::build();
            tracing_subscriber::registry()
                .with(env_filter)
                .with(layer)
                .init();
        }
        LogMode::Threaded => {
            if std::io::stderr().is_terminal() {
                let layer = threaded_layer::build();
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(layer)
                    .init();
            } else {
                let layer = line_layer::build();
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(layer)
                    .init();
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    /// A writer that buffers output into a shared `Vec<u8>` for test assertions.
    #[derive(Clone, Default)]
    pub struct TestWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl TestWriter {
        pub fn get_string(&self) -> String {
            let buf = self.buf.lock().unwrap();
            String::from_utf8_lossy(&buf).to_string()
        }
    }

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestWriter {
        type Writer = TestWriter;
        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn line_format_includes_level() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(line_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("hello world");
        });
        let output = writer.get_string();
        assert!(output.starts_with("INFO "), "expected INFO level, got: {output}");
    }

    #[test]
    fn line_format_quotes_message() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(line_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("hello world");
        });
        let output = writer.get_string();
        assert!(output.contains("\"hello world\""), "expected quoted message, got: {output}");
    }

    #[test]
    fn line_format_includes_group_from_span() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(line_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("apply", namespace = "ory");
            let _guard = span.enter();
            tracing::info!("Applying manifests...");
        });
        let output = writer.get_string();
        assert!(output.contains("group=apply"), "expected group=apply, got: {output}");
        assert!(output.contains("namespace=\"ory\""), "expected namespace field, got: {output}");
    }

    #[test]
    fn line_format_event_fields_before_message() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(line_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(count = 17, "Found services");
        });
        let output = writer.get_string();
        // Event fields should appear before the quoted message.
        let msg_idx = output.find("\"Found services\"").expect("message not found");
        let count_idx = output.find("count=17").expect("count field not found");
        assert!(
            count_idx < msg_idx,
            "event fields should precede message, got: {output}"
        );
    }

    #[test]
    fn line_format_sanitizes_newlines() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(line_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("line one\nline two");
        });
        let output = writer.get_string();
        assert!(
            output.contains("line one\\nline two"),
            "expected escaped newline, got: {output}"
        );
        assert!(!output.contains("line one\nline two"), "raw newline should be stripped, got: {output}");
    }

    #[test]
    fn line_format_escapes_quotes() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(line_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(r#"say "hello""#);
        });
        let output = writer.get_string();
        assert!(
            output.contains("say \\\"hello\\\""),
            "expected escaped quotes, got: {output}"
        );
    }

    #[test]
    fn json_format_produces_valid_json() {
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::registry().with(json_layer::build_with_writer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("apply", namespace = "ory");
            let _guard = span.enter();
            tracing::info!("Applying manifests...");
        });
        let output = writer.get_string();
        let line = output.lines().next().expect("no output");
        let parsed: serde_json::Value = serde_json::from_str(line).expect("invalid JSON");
        assert_eq!(parsed["level"], "INFO");
        assert_eq!(parsed["fields"]["message"], "Applying manifests...");
    }

    #[test]
    fn event_fmt_visitor_extracts_message() {
        use tracing::field::Visit;
        let mut visitor = event_fmt::FieldVisitor::new();
        // Create a span with a message field so we can grab its metadata.
        let span = tracing::info_span!("test", message = tracing::field::Empty);
        let meta = span.metadata().unwrap();
        let field = meta.fields().field("message").unwrap();
        visitor.record_debug(&field, &"hello");
        assert_eq!(visitor.message, "\"hello\"");
    }

    #[test]
    fn event_fmt_visitor_collects_other_fields() {
        use tracing::field::Visit;
        let mut visitor = event_fmt::FieldVisitor::new();
        let span = tracing::info_span!("test", count = tracing::field::Empty);
        let meta = span.metadata().unwrap();
        let field = meta.fields().field("count").unwrap();
        visitor.record_debug(&field, &42);
        assert_eq!(visitor.fields, vec![("count".to_string(), "42".to_string())]);
    }

    #[test]
    fn sanitize_message_strips_newlines() {
        assert_eq!(event_fmt::sanitize_message("a\nb"), "a\\nb");
    }

    #[test]
    fn sanitize_message_escapes_quotes() {
        assert_eq!(event_fmt::sanitize_message(r#"a"b"#), r#"a\"b"#);
    }

    #[test]
    fn log_mode_default_is_line() {
        assert!(matches!(LogMode::default(), LogMode::Line));
    }
}
