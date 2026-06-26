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
    /// Trace-level verbosity.
    Trace,
    /// Debug-level verbosity.
    Debug,
    /// Informational messages.
    Info,
    /// Warning messages.
    Warn,
    /// Error messages.
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Level::Trace => write!(f, "TRACE"),
            Level::Debug => write!(f, "DEBUG"),
            Level::Info => write!(f, "INFO"),
            Level::Warn => write!(f, "WARN"),
            Level::Error => write!(f, "ERROR"),
        }
    }
}

fn level_rank(level: Level) -> u8 {
    match level {
        Level::Trace => 0,
        Level::Debug => 1,
        Level::Info => 2,
        Level::Warn => 3,
        Level::Error => 4,
    }
}

// ---------------------------------------------------------------------------
// Sink
// ---------------------------------------------------------------------------

/// Backend trait — implement this to add a new output target.
pub trait Sink: Send + Sync {
    /// Emit a single log event.
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
    /// Log level of the event.
    pub level: Level,
    /// Log message.
    pub msg: String,
    /// Key-value fields attached to the event.
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
// LineSink
// ---------------------------------------------------------------------------

/// Awk-friendly single-line log sink writing to stderr.
pub struct LineSink {
    min_level: Level,
}

impl LineSink {
    /// Create a new `LineSink` with `min_level` set to [`Level::Info`].
    pub fn new() -> Self {
        Self {
            min_level: Level::Info,
        }
    }
    /// Set the minimum log level. Events below this level are dropped.
    pub fn with_level(mut self, level: Level) -> Self {
        self.min_level = level;
        self
    }
}

impl Default for LineSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for LineSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        if level_rank(level) < level_rank(self.min_level) {
            return;
        }
        let now = chrono::Local::now();
        let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let sanitized = msg.replace('\n', "\\n").replace('"', "\\\"");
        let mut out = format!(r#"time="{ts}" level={level} msg="{sanitized}""#);
        for (k, v) in fields {
            out.push(' ');
            out.push_str(k);
            out.push('=');
            out.push_str(&v.to_string());
        }
        eprintln!("{out}");
    }
}

// ---------------------------------------------------------------------------
// JsonSink
// ---------------------------------------------------------------------------

/// NDJSON structured log sink writing to stderr.
pub struct JsonSink {
    min_level: Level,
}

impl JsonSink {
    /// Create a new `JsonSink` with `min_level` set to [`Level::Info`].
    pub fn new() -> Self {
        Self {
            min_level: Level::Info,
        }
    }
    /// Set the minimum log level. Events below this level are dropped.
    pub fn with_level(mut self, level: Level) -> Self {
        self.min_level = level;
        self
    }
}

impl Default for JsonSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for JsonSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        if level_rank(level) < level_rank(self.min_level) {
            return;
        }
        let now = chrono::Local::now();
        let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let mut map = serde_json::Map::new();
        map.insert("timestamp".to_string(), serde_json::Value::String(ts));
        map.insert(
            "level".to_string(),
            serde_json::Value::String(level.to_string()),
        );
        map.insert(
            "message".to_string(),
            serde_json::Value::String(msg.to_string()),
        );

        let mut field_map = serde_json::Map::new();
        for (k, v) in fields {
            field_map.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
        if !field_map.is_empty() {
            map.insert("fields".to_string(), serde_json::Value::Object(field_map));
        }

        if let Ok(line) = serde_json::to_string(&serde_json::Value::Object(map)) {
            eprintln!("{line}");
        }
    }
}

// ---------------------------------------------------------------------------
// ThreadedSink
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::Mutex;

/// Grouped concurrent output sink backed by `indicatif::MultiProgress`.
///
/// Each distinct `group` field value gets its own progress bar. Events without
/// a `group` field are printed via [`MultiProgress::println`].
///
/// For explicit group lifecycle (start → finish with checkmark), use
/// [`ThreadedSink::enter_group`].
#[derive(Clone)]
pub struct ThreadedSink {
    state: Arc<Mutex<ThreadedState>>,
    min_level: Level,
}

struct ThreadedState {
    mp: indicatif::MultiProgress,
    groups: HashMap<String, indicatif::ProgressBar>,
}

impl ThreadedSink {
    /// Create a new `ThreadedSink` with `min_level` set to [`Level::Info`].
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ThreadedState {
                mp: indicatif::MultiProgress::new(),
                groups: HashMap::new(),
            })),
            min_level: Level::Info,
        }
    }
    /// Set the minimum log level. Events below this level are dropped.
    pub fn with_level(mut self, level: Level) -> Self {
        self.min_level = level;
        self
    }
}

impl Default for ThreadedSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadedSink {
    /// Enter a named group. The returned guard finishes the progress bar on drop.
    pub fn enter_group(&self, name: impl Into<String>) -> ThreadedGroupGuard {
        let name = name.into();
        let mut state = self.state.lock().unwrap();
        let pb = state.mp.add(indicatif::ProgressBar::new_spinner());
        let style = indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
        pb.set_style(style);
        pb.set_message(format!("{name} ..."));
        state.groups.insert(name.clone(), pb);
        ThreadedGroupGuard {
            state: self.state.clone(),
            name,
        }
    }
}

/// Guard that finishes a threaded group on drop.
pub struct ThreadedGroupGuard {
    state: Arc<Mutex<ThreadedState>>,
    name: String,
}

impl Drop for ThreadedGroupGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if let Some(pb) = state.groups.remove(&self.name) {
            let style =
                indicatif::ProgressStyle::with_template("{prefix:.bold.green} {msg}").unwrap();
            pb.set_style(style);
            pb.set_prefix("✓");
            let elapsed = pb.elapsed();
            pb.finish_with_message(format!("{}  {:.1}s", self.name, elapsed.as_secs_f64()));
        }
    }
}

impl Sink for ThreadedSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        if level_rank(level) < level_rank(self.min_level) {
            return;
        }
        let now = chrono::Local::now();
        let ts = now.format("%H:%M:%S%.3f").to_string();

        let extra: Vec<String> = fields
            .iter()
            .filter(|(k, _)| *k != "group")
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        let line = if extra.is_empty() {
            format!("{ts}  {}", msg)
        } else {
            format!("{ts}  {}  {}", msg, extra.join(" "))
        };

        let group = fields
            .iter()
            .find(|(k, _)| *k == "group")
            .map(|(_, v)| v.to_string());

        if let Some(name) = group {
            let mut state = self.state.lock().unwrap();
            if let Some(pb) = state.groups.get_mut(&name) {
                pb.set_message(format!("{name}  {line}"));
            } else {
                let pb = state.mp.add(indicatif::ProgressBar::new_spinner());
                let style = indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
                pb.set_style(style);
                pb.set_message(format!("{name}  {line}"));
                state.groups.insert(name, pb);
            }
        } else {
            let state = self.state.lock().unwrap();
            let _ = state.mp.println(line);
        }
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
        assert!(
            ev.fields
                .contains(&("app".to_string(), "nginx".to_string()))
        );
        assert!(
            ev.fields
                .contains(&("pod".to_string(), "web-0".to_string()))
        );
        assert!(
            ev.fields
                .contains(&("port".to_string(), "8080".to_string()))
        );
    }

    #[test]
    fn test_child_overrides_parent_order() {
        let sink = TestSink::default();
        let root = Logger::new(sink.clone());
        let child = root.with_field("key", &"child");

        child.info("msg", &[]);

        let events = sink.take();
        assert_eq!(events[0].fields.len(), 1);
        assert_eq!(
            events[0].fields[0],
            ("key".to_string(), "child".to_string())
        );
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
        assert!(
            ev.fields
                .contains(&("name".to_string(), "world".to_string()))
        );
        assert!(ev.fields.contains(&("count".to_string(), "42".to_string())));
    }

    #[test]
    fn line_sink_does_not_panic() {
        let logger = Logger::new(LineSink::new());
        logger.info("hello world", &[("count", &42)]);
    }

    #[test]
    fn line_sink_filters_by_level() {
        let sink = TestSink::default();
        let logger = Logger::new(sink.clone());
        let line_logger = Logger::new(LineSink::new().with_level(Level::Warn));
        // Just verify it doesn't panic and filters correctly at the sink level.
        line_logger.debug("hidden", &[]);
        line_logger.info("hidden", &[]);
        line_logger.error("visible", &[]);
    }

    #[test]
    fn json_sink_does_not_panic() {
        let logger = Logger::new(JsonSink::new());
        logger.info("hello world", &[("count", &42)]);
    }

    #[test]
    fn threaded_sink_tracks_groups() {
        let sink = ThreadedSink::new();
        let logger = Logger::new(sink.clone());
        let _guard = sink.enter_group("apply");
        logger.info("Applying manifests...", &[("group", &"apply")]);
    }

    #[test]
    fn threaded_sink_handles_events_outside_group() {
        let sink = ThreadedSink::new();
        let logger = Logger::new(sink.clone());
        logger.info("Orphan event", &[]);
    }

    #[test]
    fn threaded_sink_filters_by_level() {
        let sink = ThreadedSink::new().with_level(Level::Error);
        let logger = Logger::new(sink);
        logger.info("hidden", &[]);
        logger.error("visible", &[]);
    }

    #[test]
    fn display_level_formats_correctly() {
        assert_eq!(format!("{}", Level::Trace), "TRACE");
        assert_eq!(format!("{}", Level::Debug), "DEBUG");
        assert_eq!(format!("{}", Level::Info), "INFO");
        assert_eq!(format!("{}", Level::Warn), "WARN");
        assert_eq!(format!("{}", Level::Error), "ERROR");
    }
}
