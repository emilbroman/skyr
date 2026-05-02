//! IAS — Identity and Access Service.
//!
//! Per-region service that owns the region's identity-token signing key
//! and challenge salt, and fronts the regional UDB. Every API edge is a
//! gRPC client of every region's IAS.
//!
//! See `proto/ias.proto` for the protocol surface.

pub mod auth;
pub mod challenge;
pub mod service;
pub mod webauthn;

pub mod proto {
    tonic::include_proto!("ias.v1");
}

use tonic::transport::Channel;

/// Convenience alias for the generated gRPC client.
pub type IdentityAndAccessClient =
    proto::identity_and_access_client::IdentityAndAccessClient<Channel>;

pub use proto::identity_and_access_server::{IdentityAndAccess, IdentityAndAccessServer};
