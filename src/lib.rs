// SPDX-License-Identifier: MPL-2.0
mod commands;
mod device;
mod firmware;
mod live;
mod protocol;
mod session;
pub use commands::*;
pub use device::*;
pub use firmware::*;
pub use live::*;
pub use protocol::*;
pub use session::*;
uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{detail}")]
    Invalid { detail: String },
    #[error("device error {code}")]
    Device { code: u16 },
    #[error("{detail}")]
    Incompatible { detail: String },
    #[error("{detail}")]
    Busy { detail: String },
    #[error("request timed out")]
    Timeout,
    #[error("transport disconnected")]
    Disconnected,
    #[error("operation cancelled")]
    Cancelled,
    #[error("receive queue overflow")]
    Overflow,
}
pub(crate) fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid {
        detail: message.into(),
    }
}
pub(crate) type Result<T> = std::result::Result<T, CoreError>;
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}
