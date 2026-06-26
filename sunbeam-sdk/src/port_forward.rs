//! Port-forward helper: bind local TCP listeners and proxy to pod ports via
//! the kube-rs API. Used by `sunbeam service port-forward`.
//!
//! Unlike `secrets::port_forward`, which is designed as a background helper
//! with a dynamic local port, this module exposes fixed local-to-remote
//! mappings and blocks until Ctrl-C.

use crate::error::{Result, SunbeamError};
use crate::info;

use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use tokio::net::TcpListener;

/// Serve one or more `(local, remote)` port mappings for the given pod until
/// the user presses Ctrl-C. All mappings bind to 127.0.0.1.
#[tracing::instrument(skip(logger))]
pub async fn serve_port_forward(
    logger: &crate::logger::Logger,
    namespace: String,
    pod_name: String,
    mappings: Vec<(u16, u16)>,
) -> Result<()> {
    let client = crate::kube::get_client().await?;
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    for (local, remote) in mappings {
        let pods: Api<Pod> = Api::namespaced(client.clone(), &namespace);
        let pod_name_cl = pod_name.clone();
        let logger_cl = logger.clone();

        let listener = TcpListener::bind(("127.0.0.1", local))
            .await
            .map_err(|e| SunbeamError::Other(format!("failed to bind 127.0.0.1:{local}: {e}")))?;
        info!(
            logger,
            "Port-forward mapping active",
            local = local,
            remote = remote
        );

        let handle = tokio::spawn(async move {
            loop {
                let (mut client_sock, _peer) = match listener.accept().await {
                    Ok(x) => x,
                    Err(e) => {
                        info!(logger_cl, "accept failed on port", local = local, error = e);
                        break;
                    }
                };
                let pods = pods.clone();
                let pod_name = pod_name_cl.clone();
                let logger_inner = logger_cl.clone();
                tokio::spawn(async move {
                    let mut pf = match pods.portforward(&pod_name, &[remote]).await {
                        Ok(pf) => pf,
                        Err(e) => {
                            info!(
                                logger_inner,
                                "portforward failed",
                                pod = pod_name,
                                remote = remote,
                                error = e
                            );
                            return;
                        }
                    };
                    let mut upstream = match pf.take_stream(remote) {
                        Some(s) => s,
                        None => return,
                    };
                    let _ = tokio::io::copy_bidirectional(&mut client_sock, &mut upstream).await;
                });
            }
        });
        tasks.push(handle);
    }

    info!(logger, "Port-forward active: press Ctrl-C to stop.");
    let _ = tokio::signal::ctrl_c().await;

    for h in &tasks {
        h.abort();
    }
    Ok(())
}
