//! Structured logger with inherited fields.
//!
//! Production code passes a [`Logger`] through constructors. Child loggers
//! inherit fields from their parents via [`Logger::with_fields`].
//!
//! ```ignore
//! let root = Logger::new(TracingSink);
//! let ns_logger = root.with_field("namespace", &"production");
//! info!(ns_logger, "Applying", kind = %kind);
//! ```

use std::fmt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Level
// ---------------------------------------------------------------------------

/// Log severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// ---------------------------------------------------------------------------
// Sink
// ---------------------------------------------------------------------------

/// Backend trait — implement this to add a new output target.
pub trait Sink: Send + Sync {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]);
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

/// User-facing logger handle. Clone is cheap (two `Arc`s).
///
/// Fields added via [`Logger::with_fields`] are prepended to every log call
/// made through this handle (or any of its clones/children).
#[derive(Clone)]
pub struct Logger {
    sink: Arc<dyn Sink>,
    inherited: Arc<Vec<(String, String)>>,
}

impl Logger {
    /// Create a root logger backed by `sink`.
    pub fn new(sink: impl Sink + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
            inherited: Arc::new(Vec::new()),
        }
    }

    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        let mut merged: Vec<(&str, &dyn fmt::Display)> =
            Vec::with_capacity(self.inherited.len() + fields.len());
        for (k, v) in self.inherited.iter() {
            merged.push((k.as_str(), v as &dyn fmt::Display));
        }
        for (k, v) in fields {
            merged.push((*k, *v));
        }
        self.sink.log(level, msg, &merged);
    }

    /// Log at TRACE level.
    pub fn trace(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        self.log(Level::Trace, msg, fields);
    }

    /// Log at DEBUG level.
    pub fn debug(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        self.log(Level::Debug, msg, fields);
    }

    /// Log at INFO level.
    pub fn info(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        self.log(Level::Info, msg, fields);
    }

    /// Log at ERROR level.
    pub fn error(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        self.log(Level::Error, msg, fields);
    }

    /// Return a child logger that carries `fields` on every future log.
    ///
    /// Cheap: clones two `Arc`s and appends to a `Vec`.
    pub fn with_fields(&self, fields: &[(&str, &dyn fmt::Display)]) -> Self {
        let mut inherited = (*self.inherited).clone();
        for (k, v) in fields {
            inherited.push((k.to_string(), v.to_string()));
        }
        Self {
            sink: self.sink.clone(),
            inherited: Arc::new(inherited),
        }
    }

    /// Convenience: single-field [`Logger::with_fields`].
    pub fn with_field(&self, key: &str, value: &dyn fmt::Display) -> Self {
        self.with_fields(&[(key, value)])
    }
}

// ---------------------------------------------------------------------------
// Backends
// ---------------------------------------------------------------------------

/// Forwards to the `tracing` crate.
pub struct TracingSink;

impl Sink for TracingSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        if fields.is_empty() {
            match level {
                Level::Trace => tracing::trace!(msg),
                Level::Debug => tracing::debug!(msg),
                Level::Info => tracing::info!(msg),
                Level::Warn => tracing::warn!(msg),
                Level::Error => tracing::error!(msg),
            }
        } else {
            // Build a single formatted field string for tracing.
            // We keep msg as the primary message and attach fields separately.
            let mut buf = String::new();
            for (i, (k, v)) in fields.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                buf.push_str(k);
                buf.push('=');
                buf.push_str(&v.to_string());
            }
            match level {
                Level::Trace => tracing::trace!(msg, fields = %buf),
                Level::Debug => tracing::debug!(msg, fields = %buf),
                Level::Info => tracing::info!(msg, fields = %buf),
                Level::Warn => tracing::warn!(msg, fields = %buf),
                Level::Error => tracing::error!(msg, fields = %buf),
            }
        }
    }
}

/// Swallows every log line. Zero-cost for benchmarks or tests that don't care.
pub struct NoopSink;

impl Sink for NoopSink {
    fn log(&self, _level: Level, _msg: &str, _fields: &[(&str, &dyn fmt::Display)]) {}
}

/// Records every event into a `Vec` for test assertions.
#[derive(Default, Clone)]
pub struct TestSink {
    events: Arc<std::sync::Mutex<Vec<RecordedEvent>>>,
}

/// A single captured log event.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    pub level: Level,
    pub msg: String,
    pub fields: Vec<(String, String)>,
}

impl TestSink {
    /// Drain all captured events.
    pub fn take(&self) -> Vec<RecordedEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

impl Sink for TestSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        let kvs: Vec<(String, String)> = fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.events.lock().unwrap().push(RecordedEvent {
            level,
            msg: msg.to_string(),
            fields: kvs,
        });
    }
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

/// `trace!(logger, "msg", key = value)`
///
/// Values are formatted with `Display`. Use `format!("{:?}", val)` if you need
/// `Debug`.
#[macro_export]
macro_rules! trace {
    ($logger:expr, $msg:expr $(, $key:ident = $val:expr)* $(,)?) => {{
        $logger.trace($msg, &[
            $((stringify!($key), &$val as &dyn std::fmt::Display),)*
        ]);
    }};
}

/// `debug!(logger, "msg", key = value)`
#[macro_export]
macro_rules! debug {
    ($logger:expr, $msg:expr $(, $key:ident = $val:expr)* $(,)?) => {{
        $logger.debug($msg, &[
            $((stringify!($key), &$val as &dyn std::fmt::Display),)*
        ]);
    }};
}

/// `info!(logger, "msg", key = value)`
#[macro_export]
macro_rules! info {
    ($logger:expr, $msg:expr $(, $key:ident = $val:expr)* $(,)?) => {{
        $logger.info($msg, &[
            $((stringify!($key), &$val as &dyn std::fmt::Display),)*
        ]);
    }};
}

/// `error!(logger, "msg", key = value)`
#[macro_export]
macro_rules! error {
    ($logger:expr, $msg:expr $(, $key:ident = $val:expr)* $(,)?) => {{
        $logger.error($msg, &[
            $((stringify!($key), &$val as &dyn std::fmt::Display),)*
        ]);
    }};
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logger_inheritance() {
        let sink = TestSink::default();
        let root = Logger::new(sink.clone());
        let child = root.with_fields(&[("ns", &"prod"), ("app", &"nginx")]);
        let grandchild = child.with_field("pod", &"web-0");

        grandchild.info("started", &[("port", &8080)]);

        let events = sink.take();
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.msg, "started");
        assert_eq!(ev.level, Level::Info);
        assert!(ev.fields.contains(&("ns".to_string(), "prod".to_string())));
        assert!(ev.fields.contains(&("app".to_string(), "nginx".to_string())));
        assert!(ev.fields.contains(&("pod".to_string(), "web-0".to_string())));
        assert!(ev.fields.contains(&("port".to_string(), "8080".to_string())));
    }

    #[test]
    fn test_child_overrides_parent_order() {
        let sink = TestSink::default();
        let root = Logger::new(sink.clone());
        let child = root.with_field("key", &"child");

        child.info("msg", &[]);

        let events = sink.take();
        assert_eq!(events[0].fields.len(), 1);
        assert_eq!(events[0].fields[0], ("key".to_string(), "child".to_string()));
    }

    #[test]
    fn test_noop_sink() {
        let logger = Logger::new(NoopSink);
        logger.error("should not panic", &[]);
    }

    #[test]
    fn test_macro_info() {
        let sink = TestSink::default();
        let logger = Logger::new(sink.clone());
        info!(logger, "hello", name = "world", count = 42);
        let ev = sink.take().pop().unwrap();
        assert_eq!(ev.msg, "hello");
        assert!(ev.fields.contains(&("name".to_string(), "world".to_string())));
        assert!(ev.fields.contains(&("count".to_string(), "42".to_string())));
    }
}
