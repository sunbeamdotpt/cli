//! Shared event formatting utilities.

use tracing::field::{Field, Visit};

/// Visitor that extracts the message and remaining fields into strings.
pub struct FieldVisitor {
    pub message: String,
    pub fields: Vec<(String, String)>,
}

impl FieldVisitor {
    pub fn new() -> Self {
        Self {
            message: String::new(),
            fields: Vec::new(),
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let val = format!("{:?}", value);
        if field.name() == "message" {
            self.message = val;
        } else {
            self.fields.push((field.name().to_string(), val));
        }
    }
}

/// Sanitize a log message for single-line output:
/// - Strip actual newlines, replace with literal `\n`.
/// - Escape interior double quotes as `\"`.
pub fn sanitize_message(msg: &str) -> String {
    msg.replace('\n', "\\n").replace('"', "\\\"")
}
