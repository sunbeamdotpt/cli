//! Down workflow steps.

pub mod teardown;

pub use teardown::{
    DeleteNamespaces, DiscoverNamespaces, ForceDeleteStuckNamespaces, WaitForTermination,
};
