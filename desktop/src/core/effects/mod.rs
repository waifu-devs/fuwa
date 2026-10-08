//! The backdrop's moving effects as WGSL, the same text the web app runs
//! (`web/src/lib/effects`): the built-in shaders (`shaders.rs`), the custom
//! shaders people write (`custom.rs`), what happened when each one ran
//! (`status.rs`), and the renderer that draws them offscreen with wgpu and
//! hands the frames to the window (`gpu.rs`).

pub mod custom;
pub mod gpu;
pub mod shaders;
pub mod status;
