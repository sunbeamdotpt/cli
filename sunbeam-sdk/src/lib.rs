#[macro_use]
pub mod error;

pub mod client;

pub mod auth;
pub mod checks;
pub mod cluster;
pub mod config;
pub mod constants;
pub mod gitea;
pub mod images;
pub mod kube;
pub mod manifests;
pub mod openbao;
pub mod output;
pub mod pm;
pub mod secrets;
pub mod services;
pub mod update;
pub mod users;

// Feature-gated service client modules
#[cfg(feature = "identity")]
pub mod identity;
#[cfg(feature = "matrix")]
pub mod matrix;
#[cfg(feature = "opensearch")]
pub mod search;
#[cfg(feature = "s3")]
pub mod storage;
#[cfg(feature = "livekit")]
pub mod media;
#[cfg(feature = "monitoring")]
pub mod monitoring;
#[cfg(feature = "lasuite")]
pub mod lasuite;
#[cfg(feature = "build")]
pub mod build;
