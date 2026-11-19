//! Read-only client for the Yandex Telemost web messenger protocol.

pub mod bootstrap;
pub mod client;
pub mod error;
pub mod frame;
pub mod log;
pub mod media;
mod net;
pub mod registry;
pub mod request;
pub mod runtime;
pub mod session;
#[cfg(test)]
mod test_log;
pub mod wire;
pub mod ws;
pub mod xiva;

pub use client::{Client, ClientConfig};
pub use error::ProtoError;
pub use net::HttpSettings;
