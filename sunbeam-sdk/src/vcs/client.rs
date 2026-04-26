//! Tonic client builders for gitserv's repo/ref/mirror surface.
//!
//! Each helper layers a generated gitserv-proto client over the shared
//! bearer-auth interceptor from [`crate::grpc_auth`]. Callers pick up an
//! access token via [`crate::auth::get_access_token`].

use crate::error::{Result, ResultExt, SunbeamError};
use crate::grpc_auth::{BearerAuth, connect_with_bearer};
use tonic::codegen::InterceptedService;
use tonic::transport::Channel;

use gitserv_proto::pb::{
    ref_service_client::RefServiceClient, repo_service_client::RepoServiceClient,
};

/// Authenticated RepoService client (includes mirror RPCs per proto).
pub type RepoClient = RepoServiceClient<InterceptedService<Channel, BearerAuth>>;

/// Authenticated RefService client.
pub type RefClient = RefServiceClient<InterceptedService<Channel, BearerAuth>>;

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

/// Resolve an SSO access token from the sunbeam auth cache.
///
/// Mirrors the `wfectl/mod.rs::resolve_token` precedent — the on-disk
/// format is `~/.sunbeam/auth/<domain>.json` with an `access_token` field.
pub fn resolve_token(domain: &str) -> Result<String> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(format!(".sunbeam/auth/{domain}.json"));
    let bytes = std::fs::read(&path)
        .ctx("not logged in — run `sunbeam auth sso` first")?;
    let token: serde_json::Value = serde_json::from_slice(&bytes)?;
    token["access_token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| SunbeamError::identity("token cache is corrupt — run `sunbeam auth sso`"))
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
