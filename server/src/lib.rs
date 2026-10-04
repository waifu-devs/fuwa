//! The fuwa server: a self-hostable, Discord-like chat server.
//!
//! One running server is an *instance* (called a node in the API). It keeps its
//! accounts in `node.db` and hosts any number of community servers, each in its
//! own Turso database under `servers/`. Direct messages, end-to-end encrypted,
//! go through `dms.db`. Clients speak gRPC or gRPC-Web.

pub mod api;
pub mod app;
pub mod attachments;
pub mod auth;
pub mod automod;
pub mod cluster;
pub mod config;
pub mod db;
pub mod dms;
pub mod error;
pub mod federation;
pub mod gifs;
pub mod hub;
pub mod id;
pub mod linked;
pub mod mcp;
pub mod media;
pub mod node;
pub mod outside;
pub mod permissions;
pub mod presence;
pub mod probes;
pub mod recordings;
pub mod replica;
pub mod reports;
pub mod rtc;
pub mod servers;
pub mod settings;
pub mod sso;
pub mod telemetry;
pub mod twofactor;
pub mod voice;
pub mod web;
pub mod webhooks;

pub mod proto {
    pub mod fuwa {
        pub mod v1 {
            tonic::include_proto!("fuwa.v1");
        }

        pub mod cluster {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/cluster/fuwa.cluster.v1.rs"));
            }
        }

        pub mod federation {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/federation/fuwa.federation.v1.rs"));
            }
        }
    }

    /// Every fuwa.v1 descriptor, for gRPC reflection.
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("fuwa_descriptor");
}

/// The internal protocol between the parts of a split instance.
pub use proto::fuwa::cluster::v1 as cpb;
/// How instances talk to each other (docs/federation.md).
pub use proto::fuwa::federation::v1 as fpb;
pub use proto::fuwa::v1 as pb;

/// This build's version, reported to clients and in the usage signal.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The git commit this binary was built from (see `build.rs`), or empty.
pub const COMMIT: &str = env!("FUWA_COMMIT");

/// Where this build's source lives.
pub const SOURCE: &str = env!("CARGO_PKG_REPOSITORY");
