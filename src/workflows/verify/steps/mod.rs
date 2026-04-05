//! Verify workflow steps — VSO ↔ OpenBao end-to-end verification.

mod verify;

pub use verify::{
    FindOpenBaoPod,
    GetRootToken,
    WriteSentinel,
    ApplyVaultAuth,
    ApplyVaultStaticSecret,
    WaitForSync,
    CheckSecretValue,
    Cleanup,
    PrintResult,
};
