//! Authenticated wfe-server gRPC client.
//!
//! Bearer-auth interceptor and channel builder now live in
//! [`crate::grpc_auth`]; this module layers the generated `WfeClient`
//! on top.

use anyhow::Result;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

pub use crate::grpc_auth::{BearerAuth, connect};

use wfe_server_protos::wfe::v1::wfe_client::WfeClient as GeneratedWfeClient;

/// Type alias for the fully-instantiated wfe client with auth interceptor.
pub type AuthClient = GeneratedWfeClient<InterceptedService<Channel, BearerAuth>>;

/// Build an authenticated wfe client.
pub async fn build(server: &str, token: &str) -> Result<AuthClient> {
    let (channel, auth) = crate::grpc_auth::connect_with_bearer(server, token).await?;
    Ok(GeneratedWfeClient::with_interceptor(channel, auth))
}
