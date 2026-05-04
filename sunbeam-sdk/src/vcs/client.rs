//! connectrpc client builders for gitserv's repo/ref/mirror surface.
//!
//! Each helper builds a generated gitserv-proto client over the shared
//! bearer-auth connection from [`crate::grpc_auth`]. Callers pick up an
//! access token via [`crate::auth::get_access_token`].

use crate::error::{Result, SunbeamError};
use crate::grpc_auth::connect_with_bearer;
use connectrpc::ConnectError;
use gitserv_proto::pb::{
    AdminServiceClient, RefServiceClient, RepoServiceClient,
    SigningKeyServiceClient,
};

pub use crate::grpc_auth::connect_with_bearer as connect_with_bearer_raw;

/// Authenticated RepoService client.
pub type RepoClient = RepoServiceClient<connectrpc::client::SharedHttp2Connection>;

/// Authenticated RefService client.
pub type RefClient = RefServiceClient<connectrpc::client::SharedHttp2Connection>;

/// Authenticated AdminService client.
pub type AdminClient = AdminServiceClient<connectrpc::client::SharedHttp2Connection>;

/// Authenticated SigningKeyService client.
pub type SigningKeyClient = SigningKeyServiceClient<connectrpc::client::SharedHttp2Connection>;

pub async fn connect_repo_client(endpoint: &str, token: &str) -> Result<RepoClient> {
    let (conn, config) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(RepoServiceClient::new(conn, config))
}

pub async fn connect_ref_client(endpoint: &str, token: &str) -> Result<RefClient> {
    let (conn, config) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(RefServiceClient::new(conn, config))
}

pub async fn connect_admin_client(endpoint: &str, token: &str) -> Result<AdminClient> {
    let (conn, config) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(AdminServiceClient::new(conn, config))
}

pub async fn connect_signing_key_client(endpoint: &str, token: &str) -> Result<SigningKeyClient> {
    let (conn, config) = connect_with_bearer(endpoint, token)
        .await
        .map_err(|e| SunbeamError::network(format!("gitserv connect: {e}")))?;
    Ok(SigningKeyServiceClient::new(conn, config))
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

/// Map a connectrpc [`ConnectError`] to a `SunbeamError` with an appropriate
/// exit-code flavor.
///
/// `NotFound` → Config (65), `PermissionDenied` → Identity (77),
/// `InvalidArgument` → Config (64-ish), `Unavailable` → Network (69),
/// `Unauthenticated` → Identity (77), `Internal`/other → Other (70).
pub fn map_error(err: ConnectError) -> SunbeamError {
    use connectrpc::ErrorCode;
    let msg = err.message.as_deref().unwrap_or("(no message)");
    match err.code {
        ErrorCode::NotFound => SunbeamError::config(format!("not found: {msg}")),
        ErrorCode::PermissionDenied => {
            SunbeamError::identity(format!("permission denied: {msg}"))
        }
        ErrorCode::InvalidArgument => {
            SunbeamError::config(format!("invalid argument: {msg}"))
        }
        ErrorCode::Unavailable => {
            SunbeamError::network(format!("unavailable: {msg}"))
        }
        ErrorCode::Unauthenticated => {
            SunbeamError::identity(format!("unauthenticated: {msg}"))
        }
        _ => SunbeamError::Other(format!("gitserv error: {}: {msg}", err.code.grpc_code())),
    }
}

// Keep old name as an alias for call sites that use `map_status`.
pub use map_error as map_status;
