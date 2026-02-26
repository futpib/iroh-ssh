#[cfg(feature = "cli")]
mod cli;
#[cfg(feature = "cli")]
mod service;
mod ssh;

use ed25519_dalek::{PUBLIC_KEY_LENGTH, SECRET_KEY_LENGTH};
use iroh::{Endpoint, RelayUrl, protocol::Router};

#[cfg(feature = "cli")]
pub mod api;

#[cfg(feature = "flutter")]
pub mod bridge;

#[cfg(feature = "cli")]
pub use cli::*;
#[cfg(feature = "cli")]
pub use service::Service;
#[cfg(feature = "cli")]
pub use service::ServiceParams;
#[cfg(feature = "cli")]
pub use service::{install_service, run_service, uninstall_service};
#[cfg(feature = "cli")]
pub use ssh::dot_ssh;

#[derive(Debug, Clone)]
pub struct IrohSsh {
    #[allow(dead_code)]
    pub(crate) secret_key: [u8; SECRET_KEY_LENGTH],
    #[allow(dead_code)]
    pub(crate) public_key: [u8; PUBLIC_KEY_LENGTH],
    pub(crate) inner: Option<Inner>,
    pub(crate) ssh_port: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct Inner {
    pub endpoint: Endpoint,
    #[allow(dead_code)]
    pub router: Router,
}

#[derive(Debug, Clone)]
pub struct Builder {
    secret_key: [u8; SECRET_KEY_LENGTH],
    accept_incoming: bool,
    accept_port: Option<u16>,
    relay_urls: Vec<RelayUrl>,
    extra_relay_urls: Vec<RelayUrl>,
    max_remote_nat_traversal_addresses: Option<u8>,
}
