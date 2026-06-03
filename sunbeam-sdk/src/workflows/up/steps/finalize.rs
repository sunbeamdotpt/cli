//! Finalize steps: print URLs.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::workflows::data::UpData;

fn resolve_domain(data: &UpData) -> String {
    if !data.domain.is_empty() {
        return data.domain.clone();
    }
    if let Some(ctx) = &data.ctx
        && !ctx.domain.is_empty()
    {
        return ctx.domain.clone();
    }
    String::new()
}

/// Print service URLs.
#[derive(Default)]
pub struct PrintURLs;

#[async_trait::async_trait]
impl StepBody for PrintURLs {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        let domain = resolve_domain(&data);
        tracing::info!(msg = "Stack is up.", domain = %domain);

        let sep = "\u{2500}".repeat(60);
        println!("\n{sep}");
        println!("  Stack is up.  Domain: {domain}");
        println!("{sep}");

        let urls: &[(&str, String)] = &[
            ("Auth", format!("https://auth.{domain}/")),
            ("Docs", format!("https://docs.{domain}/")),
            ("Meet", format!("https://meet.{domain}/")),
            ("Drive", format!("https://drive.{domain}/")),
            ("Chat", format!("https://chat.{domain}/")),
            ("Mail", format!("https://mail.{domain}/")),
            ("People", format!("https://people.{domain}/")),
        ];

        for (name, url) in urls {
            println!("  {name:<10} {url}");
        }

        println!();
        println!("  OpenBao UI:");
        println!("    kubectl --context=sunbeam -n data port-forward svc/openbao 8200:8200");
        println!("    http://localhost:8200");
        println!(
            "    token: kubectl --context=sunbeam -n data get secret openbao-keys \
             -o jsonpath='{{.data.root-token}}' | base64 -d"
        );
        println!("{sep}\n");

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_urls_is_default() {
        let _ = PrintURLs;
    }

    #[test]
    fn resolve_domain_with_domain_set() {
        let data = UpData {
            domain: "sunbeam.pt".into(),
            ..Default::default()
        };
        assert_eq!(resolve_domain(&data), "sunbeam.pt");
    }
}
