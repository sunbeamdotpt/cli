//! Tonic client builders for gitserv's repo/ref/mirror surface.
//!
//! Each helper layers a generated gitserv-proto client over the shared
//! bearer-auth interceptor from [`crate::grpc_auth`]. Callers pick up an
//! access token via [`crate::auth::get_access_token`].

use crate::error::{Result, SunbeamError};
use crate::grpc_auth::{BearerAuth, connect_with_bearer};
use tonic::codegen::InterceptedService;
use tonic::transport::Channel;

use gitserv_proto::pb::{
    admin_service_client::AdminServiceClient,
    ref_service_client::RefServiceClient,
    repo_service_client::RepoServiceClient,
    signing_key_service_client::SigningKeyServiceClient,
};

/// Authenticated RepoService client (includes mirror RPCs per proto).
pub type RepoClient = RepoServiceClient<InterceptedService<Channel, BearerAuth>>;

/// Authenticated RefService client.
pub type RefClient = RefServiceClient<InterceptedService<Channel, BearerAuth>>;

/// Authenticated AdminService client.
pub type AdminClient = AdminServiceClient<InterceptedService<Channel, BearerAuth>>;

/// Authenticated SigningKeyService client.
pub type SigningKeyClient = SigningKeyServiceClient<InterceptedService<Channel, BearerAuth>>;

pub async fn connect_repo_client(endpoint: &str, token: &str) -> Result<RepoClient> {
    let (channel, auth) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(RepoServiceClient::with_interceptor(channel, auth))
}

pub async fn connect_ref_client(endpoint: &str, token: &str) -> Result<RefClient> {
    let (channel, auth) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(RefServiceClient::with_interceptor(channel, auth))
}

pub async fn connect_admin_client(endpoint: &str, token: &str) -> Result<AdminClient> {
    let (channel, auth) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(AdminServiceClient::with_interceptor(channel, auth))
}

pub async fn connect_signing_key_client(endpoint: &str, token: &str) -> Result<SigningKeyClient> {
    let (channel, auth) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(SigningKeyServiceClient::with_interceptor(channel, auth))
}

/// Resolve a valid SSO access token, refreshing it automatically if expired.
///
/// Delegates to [`crate::auth::get_token`] which reads from the per-domain
/// cache at `~/.sunbeam/auth/<domain>.json` and performs a silent refresh
/// when the cached token has less than 60 s of validity remaining.
/// The `_domain` argument is kept for call-site compatibility but is unused —
/// the active domain is derived from the CLI config context.
pub async fn resolve_token(_domain: &str) -> Result<String> {
    crate::auth::get_token().await
}

/// Map a tonic `Status` to a `SunbeamError` with an appropriate exit-code
/// flavor. `NotFound` → Config (65), `PermissionDenied` → Identity (77),
/// `InvalidArgument` → Config (64-ish), `Unavailable` → Network (69),
/// `Internal`/other → Other (70).
///
/// The caller wraps the gRPC call and converts errors eagerly so exit
/// codes land correctly.
pub fn map_status(status: tonic::Status) -> SunbeamError {
    use tonic::Code;
    match status.code() {
        Code::NotFound => SunbeamError::config(format!("not found: {}", status.message())),
        Code::PermissionDenied => {
            SunbeamError::identity(format!("permission denied: {}", status.message()))
        }
        Code::InvalidArgument => {
            SunbeamError::config(format!("invalid argument: {}", status.message()))
        }
        Code::Unavailable => SunbeamError::network(format!("unavailable: {}", status.message())),
        Code::Unauthenticated => {
            SunbeamError::identity(format!("unauthenticated: {}", status.message()))
        }
        _ => SunbeamError::Other(format!("gitserv error: {status}")),
    }
}
