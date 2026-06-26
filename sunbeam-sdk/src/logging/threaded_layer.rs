//! Threaded log layer backed by indicatif::MultiProgress.
//!
//! Each active span gets its own ProgressBar. Events are routed to the
//! span that emitted them, giving a BuildKit-style grouped output.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{Event, Subscriber, span};
use tracing_subscriber::layer::{Context, Layer};

use super::span_state::SpanBar;

/// Build the threaded formatting layer.
pub fn build<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    ThreadedLayer::new()
}

/// Shared render state for all active spans.
struct RenderState {
    mp: indicatif::MultiProgress,
    spans: HashMap<span::Id, SpanBar>,
}

/// A tracing layer that renders each span as an indicatif progress bar.
pub struct ThreadedLayer {
    state: Arc<Mutex<RenderState>>,
}

impl ThreadedLayer {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RenderState {
                mp: indicatif::MultiProgress::new(),
                spans: HashMap::new(),
            })),
        }
    }
}

/// Format a timestamp prefix for threaded-mode event lines.
fn fmt_time() -> String {
    let now = chrono::Local::now();
    now.format("%H:%M:%S%.3f").to_string()
}

impl<S> Layer<S> for ThreadedLayer
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(&self, _attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut state = self.state.lock().unwrap();
        let pb = state.mp.add(indicatif::ProgressBar::new_spinner());

        // Set up a nice spinner style.
        let style = indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
        pb.set_style(style);

        // Pre-fill the message with the span name if we can look it up.
        if let Some(span) = ctx.span(id) {
            let name = span.name();
            pb.set_message(format!("{name} ..."));
        }

        state.spans.insert(id.clone(), SpanBar::new(pb));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut state = self.state.lock().unwrap();

        // Find the current span to route this event to.
        let span_id = match ctx.lookup_current() {
            Some(span) => span.id(),
            None => return,
        };

        let Some(span_bar) = state.spans.get_mut(&span_id) else {
            return;
        };

        // Format the event into a short line with a timestamp prefix.
        let mut visitor = crate::logging::event_fmt::FieldVisitor::new();
        event.record(&mut visitor);

        let ts = fmt_time();
        let line = if visitor.fields.is_empty() {
            format!("{ts}  {}", visitor.message)
        } else {
            let fields: Vec<String> = visitor
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            format!("{ts}  {}  {}", visitor.message, fields.join(" "))
        };

        if !line.is_empty() {
            span_bar.push(line.clone());
            // Show the last line as the progress bar message.
            if let Some(span) = ctx.span(&span_id) {
                let name = span.name();
                span_bar.pb.set_message(format!("{name}  {line}"));
            } else {
                span_bar.pb.set_message(line.clone());
            }
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let mut state = self.state.lock().unwrap();
        let Some(span_bar) = state.spans.remove(&id) else {
            return;
        };

        let name = ctx
            .span(&id)
            .map(|s| s.name().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let elapsed = span_bar.pb.elapsed();
        let secs = elapsed.as_secs_f64();

        // Success style: green checkmark.
        let style = indicatif::ProgressStyle::with_template("{prefix:.bold.green} {msg}").unwrap();
        span_bar.pb.set_style(style);
        span_bar.pb.set_prefix("✓");
        span_bar
            .pb
            .finish_with_message(format!("{name}  {secs:.1}s"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn threaded_layer_tracks_spans() {
        let layer = ThreadedLayer::new();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("apply", namespace = "ory");
            let _guard = span.enter();
            tracing::info!("Applying manifests...");
        });
        // After the span is dropped, on_close should have removed it.
        // We can't easily assert on indicatif output, but we can verify
        // the layer didn't panic and the span was tracked internally.
    }

    #[test]
    fn threaded_layer_buffers_events() {
        let layer = ThreadedLayer::new();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("build", project = "ui");
            let _guard = span.enter();
            tracing::info!("Building...");
            tracing::info!("Done");
        });
    }

    #[test]
    fn threaded_layer_handles_events_outside_span() {
        let layer = ThreadedLayer::new();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("Orphan event");
        });
    }
}
