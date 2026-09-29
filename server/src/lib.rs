//! The fuwa server: a self-hostable, Discord-like chat server.
//!
//! One running server is an *instance* (called a node in the API). It keeps its
//! accounts in `node.db` and hosts any number of community servers, each in its
//! own Turso database under `servers/`. Clients speak gRPC or gRPC-Web.

pub mod api;
pub mod app;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod hub;
pub mod id;
pub mod node;
pub mod servers;
pub mod telemetry;

pub mod proto {
    pub mod fuwa {
        pub mod v1 {
            tonic::include_proto!("fuwa.v1");
        }
    }

    /// Every fuwa.v1 descriptor, for gRPC reflection.
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("fuwa_descriptor");
}

pub use proto::fuwa::v1 as pb;

/// This build's version, reported to clients and in the usage signal.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
