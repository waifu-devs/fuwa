//! fuwa for the desktop: a native app for any fuwa instance, drawn with GPUI
//! Kit.
//!
//! `core` is everything that isn't drawing: talking to instances, keeping
//! their state, encrypted direct messages, settings. Every action is a
//! method on [`core::Core`], so the main window today and the tray or game
//! overlay later are only views of it. `ui` draws the main window.

pub mod core;
pub mod ui;

/// The protocol, generated from `proto/fuwa/v1` (client side only).
#[allow(clippy::all, clippy::pedantic)]
pub mod pb {
    tonic::include_proto!("fuwa.v1");
}
