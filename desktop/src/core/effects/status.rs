//! What happened when the app last ran each custom shader on this computer,
//! by `shader_id`: whether it ran, at what resolution, or why it didn't
//! (the web's `lib/effects/status.ts`). The renderer writes it, the shader
//! editor reads it.
//!
//! A shader that stopped the GPU (or the whole app) is remembered across
//! restarts: before a shader's first frames it's noted as being tried in the
//! app's settings (`Prefs::shaders_trying`), and the note is cleared once the
//! frames came back in time. A note still there at the next start means they
//! never did, so it isn't run again until it's changed or someone asks to
//! try again.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// How many shaders being tried are remembered.
pub const MAX_TRYING: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub enum ShaderStatus {
    /// Drawing, at this share of the full resolution, its frames taking `ms` on the GPU.
    Running { scale: f32, ms: f32 },
    /// It didn't pass the checks or compile, or the GPU refused it.
    Broken { message: String, line: Option<usize> },
    /// Even its smallest frames take longer than the budget.
    Slow { ms: f32 },
    /// It stopped the GPU, or the app, while it drew.
    Stopped,
}

type Save = Arc<dyn Fn(Vec<String>) + Send + Sync>;
type Changed = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    statuses: HashMap<String, ShaderStatus>,
    trying: Vec<String>,
    save: Option<Save>,
    changed: Option<Changed>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(STATE.lock().get_or_insert_with(State::default))
}

/// Starts with the shaders still being tried when the app last stopped
/// (each of them stopped it), `save` to keep the list, and `changed` to
/// tell the window something changed.
pub fn start(
    trying: Vec<String>,
    save: impl Fn(Vec<String>) + Send + Sync + 'static,
    changed: impl Fn() + Send + Sync + 'static,
) {
    with(|s| {
        for id in &trying {
            s.statuses.insert(id.clone(), ShaderStatus::Stopped);
        }
        s.trying = trying;
        s.save = Some(Arc::new(save));
        s.changed = Some(Arc::new(changed));
    });
}

pub fn status(id: &str) -> Option<ShaderStatus> {
    with(|s| s.statuses.get(id).cloned())
}

pub fn set_status(id: &str, status: Option<ShaderStatus>) {
    tracing::debug!("custom shader {id}: {status:?}");
    let changed = with(|s| {
        let was = match status {
            Some(status) => s.statuses.insert(id.to_owned(), status.clone()) == Some(status),
            None => s.statuses.remove(id).is_none(),
        };
        (!was).then(|| s.changed.clone()).flatten()
    });
    if let Some(changed) = changed {
        changed();
    }
}

fn save_trying(f: impl FnOnce(&mut Vec<String>)) {
    let (list, save) = with(|s| {
        f(&mut s.trying);
        let over = s.trying.len().saturating_sub(MAX_TRYING);
        s.trying.drain(..over);
        (s.trying.clone(), s.save.clone())
    });
    if let Some(save) = save {
        save(list);
    }
}

/// Notes a shader as about to run its first frames.
pub fn start_trying(id: &str) {
    save_trying(|list| {
        list.retain(|t| t != id);
        list.push(id.to_owned());
    });
}

/// Its first frames came back (or it was let go before they did, and nothing went wrong).
pub fn done_trying(id: &str) {
    if with(|s| s.trying.iter().any(|t| t == id)) {
        save_trying(|list| list.retain(|t| t != id));
    }
}

/// Forgets what happened to a shader, so it's tried again.
pub fn retry(id: &str) {
    done_trying(id);
    set_status(id, None);
    crate::core::effects::gpu::forget(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_kept_and_forgotten() {
        set_status("test-a", Some(ShaderStatus::Slow { ms: 50.0 }));
        assert_eq!(status("test-a"), Some(ShaderStatus::Slow { ms: 50.0 }));
        start_trying("test-a");
        assert!(with(|s| s.trying.contains(&"test-a".to_owned())));
        retry("test-a");
        assert_eq!(status("test-a"), None);
        assert!(!with(|s| s.trying.contains(&"test-a".to_owned())));
    }
}
