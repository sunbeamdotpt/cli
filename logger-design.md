# Logger API Design — with inheritance

## 1. Two layers

| Layer | What it is | Who implements it |
|-------|-----------|-------------------|
| `Logger` | Concrete, cloneable handle. Carries inherited fields. | Us (one struct) |
| `Sink` | Trait. Receives merged fields + message. | You (tracing, stdout, test recorder, noop) |

## 2. `Sink` — the backend trait

```rust
use std::fmt;

pub trait Sink: Send + Sync {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]);
}
```

Only implement `Sink` if you are adding a new output target (tracing, file, test vec, etc.).

## 3. `Logger` — the user-facing handle

```rust
#[derive(Clone)]
pub struct Logger {
    sink: Arc<dyn Sink>,
    inherited: Arc<Vec<(String, String)>>, // pre-formatted at bind time
}

impl Logger {
    /// Log at each level. `fields` are merged on top of inherited ones.
    pub fn trace(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]);
    pub fn debug(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]);
    pub fn info (&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]);
    pub fn warn (&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]);
    pub fn error(&self, msg: &str, fields: &[(&str, &dyn fmt::Display)]);

    /// Return a child logger that carries `fields` on every future log.
    /// Cheap: clones two `Arc`s and appends to a `Vec`.
    pub fn with_fields(&self, fields: &[(&str, &dyn fmt::Display)]) -> Self;

    /// Convenience: single-field `with_fields`.
    pub fn with_field(&self, key: &str, value: &dyn fmt::Display) -> Self {
        self.with_fields(&[(key, value)])
    }
}
```

### Inheritance in action

```rust
let base = Logger::new(TracingSink);
let ns_logger = base.with_field("namespace", &"production");
let deploy_logger = ns_logger.with_fields(&[
    ("deployment", &"nginx"),
    ("revision", &"v3"),
]);

deploy_logger.info("Rolling out", &[("replicas", &3)]);
// Sink receives:
//   namespace=production, deployment=nginx, revision=v3, replicas=3
```

## 4. No global — injected at construction

No `get_logger()`, no `OnceLock`, no static state. Every struct that logs owns a `Logger`.

```rust
pub struct ApplyManifest {
    logger: Logger,
}

impl ApplyManifest {
    pub fn new(logger: Logger) -> Self {
        Self { logger }
    }

    pub async fn execute(&self) {
        // Pass a child logger down the call chain
        let doc_logger = self.logger.with_field("document", &"deployment.yaml");
        info!(doc_logger, "Applying manifest");

        // ...
        // ...
    }
}
```

The CLI entry point creates the root logger and hands it to the top-level command:

```rust
// src/main.rs or cli dispatch
let root = Logger::new(TracingSink);
let cmd_logger = root.with_field("verb", &"up");
UpCommand::new(cmd_logger).run().await;
```

A `Logger::new(sink)` constructor is provided:

```rust
impl Logger {
    pub fn new(sink: impl Sink + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
            inherited: Arc::new(Vec::new()),
        }
    }
}
```

## 6. Macro surface

```rust
/// info!(logger, "Applying", summary = %summary, attempt = %attempt);
/// % → Display, ? → Debug. No sigil defaults to Display.
#[macro_export]
macro_rules! info {
    ($logger:expr, $msg:expr $(, $key:ident $(= $fmt:tt)? $val:expr)* $(,)?) => {{
        $logger.info($msg, &[
            $(($crate::logger::_key!($key), &$crate::logger::_val!($fmt, $val)),)*
        ]);
    }};
}
// Same for trace!, debug!, warn!, error!
```

## 7. Example backends

### TracingSink
```rust
pub struct TracingSink;

impl Sink for TracingSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        // dispatch to tracing::Event with all fields
    }
}
```

### TestSink
```rust
pub struct TestSink(Mutex<Vec<RecordedEvent>>);

impl Sink for TestSink {
    fn log(&self, level: Level, msg: &str, fields: &[(&str, &dyn fmt::Display)]) {
        let kvs: Vec<(String, String)> = fields.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.0.lock().unwrap().push(RecordedEvent { level, msg: msg.into(), fields: kvs });
    }


}
```

## 8. Migration of existing call sites

**Before:**
```rust
tracing::info!(msg = "Applying CRD", summary = %summary);
```

**After — passed through constructors:**
```rust
// Parent creates child with inherited context
let logger = parent_logger.with_field("namespace", &ns);
info!(logger, "Applying CRD", summary = %summary);
```

**No global fallback.** If something needs to log and nobody gave it a logger, that's a compile error — which is the point.

## 9. Open questions

| Question | Default | Can change to |
|----------|---------|---------------|
| Child logger shadows parent key with same name? | Yes (child wins) | Error on duplicate? |
| `with_fields` pre-formats to `String` | Yes (simple, no lifetime issues) | Store closures (lazy, complex) |
| `Logger` vs `&Logger` in APIs | Owned `Logger` (cheap Clone) | `&Logger` |


---

**What do you want changed?**
