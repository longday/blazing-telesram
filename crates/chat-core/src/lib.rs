//! Sync core of the read-only client: domain model, offline cache and the `ChatClient` engine.
//! No UI and no protocol code; the network is reached through [`Backend`].

pub mod backend;
pub mod cache;
pub mod client;
pub mod model;
pub mod paths;

#[cfg(test)]
mod fixtures;

pub use backend::{Backend, BackendError, LiveEvent};
pub use client::{ChatClient, ClientConfig};
