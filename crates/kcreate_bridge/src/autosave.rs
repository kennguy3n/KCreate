//! Project auto-save with crash recovery (Phase 9 Task 26).
//!
//! Spawns a single background thread per process that periodically
//! calls `crate::document::project_save` whenever the project's
//! `modified_at` timestamp has advanced since the previous tick.
//!
//! Crash recovery: in addition to the regular incremental save,
//! every successful tick writes an *autosave marker* to
//! `<.kstudio>/autosave/marker.json` containing the project's
//! `modified_at` and a monotonic counter. On project open the
//! renderer can call `autosave_recovery_available` to detect a
//! marker that's newer than the last clean shutdown, then
//! `autosave_recover` to confirm the recovery (the in-memory state
//! is already restored from the on-disk SQLite — the marker just
//! lets the UI surface "recovered from N minutes ago" banners).
//!
//! Interval is read from
//! [`kcreate_core::config::RuntimeConfig::effective_autosave_interval_secs`]
//! so a hand-edited config can tune frequency without recompiling.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::document::{
    project_save, runtime_slot, with_workspace, with_workspace_mut, DocumentBridgeError, Result,
};

/// JSON payload written next to the project on every successful
/// autosave tick. Stable on-disk format — adding fields is OK,
/// removing is not (the recovery path reads markers written by
/// older builds).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutosaveMarker {
    pub project_path: PathBuf,
    pub modified_at: DateTime<Utc>,
    pub written_at: DateTime<Utc>,
    pub counter: u64,
}

/// Live status of the autosave subsystem for the *currently open*
/// project. Returned to the renderer as the source of truth for
/// the status pill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutosaveStatus {
    pub running: bool,
    pub interval_secs: u32,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub counter: u64,
}

#[derive(Default)]
struct AutosaveSharedState {
    last_attempt_at: Option<DateTime<Utc>>,
    last_success_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    counter: u64,
    /// `modified_at` value at the time of the last successful tick;
    /// used so an idle project doesn't get re-saved every cycle.
    last_saved_modified_at: Option<DateTime<Utc>>,
}

struct AutosaveState {
    running: AtomicBool,
    shared: Mutex<AutosaveSharedState>,
}

fn state() -> &'static Arc<AutosaveState> {
    static STATE: OnceLock<Arc<AutosaveState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Arc::new(AutosaveState {
            running: AtomicBool::new(false),
            shared: Mutex::new(AutosaveSharedState::default()),
        })
    })
}

/// Start the autosave background thread. Idempotent — calling
/// twice is a safe no-op (returns `false` on the second call).
///
/// The thread re-reads the interval at every tick so a config
/// change picks up on the next cycle without a restart.
pub fn autosave_start() -> bool {
    let state = state().clone();
    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    thread::Builder::new()
        .name("kcreate-autosave".to_string())
        .spawn(move || {
            while state.running.load(Ordering::SeqCst) {
                let interval_secs = {
                    let g = runtime_slot().lock();
                    g.effective_autosave_interval_secs()
                };
                thread::sleep(Duration::from_secs(u64::from(interval_secs)));
                if !state.running.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tick(&state);
            }
        })
        .expect("spawn autosave thread");
    true
}

/// Stop the autosave background thread. Returns `true` if it was
/// running.
pub fn autosave_stop() -> bool {
    state().running.swap(false, Ordering::SeqCst)
}

/// Run a single autosave cycle synchronously. Useful for unit
/// tests + for the "Save Now" command in the UI. Reuses the same
/// state and counter the background thread does.
pub fn autosave_force_now() -> Result<bool> {
    tick(state())
}

/// Snapshot of the autosave bookkeeping for the open project.
pub fn autosave_status() -> AutosaveStatus {
    let st = state();
    let shared = st.shared.lock();
    let interval_secs = {
        let g = runtime_slot().lock();
        g.effective_autosave_interval_secs()
    };
    AutosaveStatus {
        running: st.running.load(Ordering::SeqCst),
        interval_secs,
        last_attempt_at: shared.last_attempt_at,
        last_success_at: shared.last_success_at,
        last_error: shared.last_error.clone(),
        counter: shared.counter,
    }
}

/// Return the autosave marker for the *currently open* project if
/// one exists AND its timestamp is newer than the marker written
/// when the project was last cleanly closed. The renderer surfaces
/// the result as the "Recover unsaved work?" dialog.
pub fn autosave_recovery_available() -> Result<Option<AutosaveMarker>> {
    let marker_path = with_workspace(|ws| Ok(marker_path_for(ws.store.project_dir())))?;
    if !marker_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&marker_path).map_err(|e| {
        DocumentBridgeError::Internal(format!(
            "read autosave marker {}: {e}",
            marker_path.display()
        ))
    })?;
    let marker: AutosaveMarker = serde_json::from_slice(&bytes).map_err(|e| {
        DocumentBridgeError::Internal(format!(
            "parse autosave marker {}: {e}",
            marker_path.display()
        ))
    })?;
    // Compare against the project's `modified_at` from SQLite. If
    // the in-memory project is already at or beyond the marker,
    // there's nothing to recover.
    let project_modified = with_workspace(|ws| Ok(ws.project.modified_at))?;
    if marker.modified_at <= project_modified {
        return Ok(None);
    }
    Ok(Some(marker))
}

/// Accept the recovery — i.e. record that the user explicitly
/// opted to keep the autosaved state — and overwrite the marker
/// with the current state so subsequent opens don't keep showing
/// the prompt. The actual recovery has already happened during
/// `project_open` because the SQLite store contains the autosaved
/// rows.
pub fn autosave_recover() -> Result<()> {
    write_marker_for_current_project()?;
    let mut shared = state().shared.lock();
    shared.last_error = None;
    Ok(())
}

/// Discard the autosaved state — delete the marker. The caller is
/// expected to immediately call `project_undo` (or revert from the
/// last clean save) to roll the document back to the user's last
/// confirmed state.
pub fn autosave_dismiss_recovery() -> Result<()> {
    let marker_path = with_workspace(|ws| Ok(marker_path_for(ws.store.project_dir())))?;
    if marker_path.exists() {
        fs::remove_file(&marker_path).map_err(|e| {
            DocumentBridgeError::Internal(format!(
                "remove autosave marker {}: {e}",
                marker_path.display()
            ))
        })?;
    }
    Ok(())
}

fn tick(state: &Arc<AutosaveState>) -> Result<bool> {
    let now = Utc::now();
    {
        let mut shared = state.shared.lock();
        shared.last_attempt_at = Some(now);
    }
    // Pull `modified_at` first — if the project hasn't moved we
    // skip the disk write entirely.
    let pre = match with_workspace(|ws| Ok(ws.project.modified_at)) {
        Ok(m) => m,
        Err(DocumentBridgeError::NoProject) => return Ok(false),
        Err(e) => {
            record_error(state, &e);
            return Err(e);
        }
    };
    {
        let shared = state.shared.lock();
        if let Some(last) = shared.last_saved_modified_at {
            if pre == last {
                return Ok(false);
            }
        }
    }
    if let Err(e) = project_save() {
        record_error(state, &e);
        return Err(e);
    }
    let new_counter = {
        let mut shared = state.shared.lock();
        shared.counter = shared.counter.saturating_add(1);
        shared.last_saved_modified_at = Some(pre);
        shared.last_success_at = Some(Utc::now());
        shared.last_error = None;
        shared.counter
    };
    write_marker_with_counter(new_counter)?;
    Ok(true)
}

fn record_error(state: &Arc<AutosaveState>, e: &DocumentBridgeError) {
    let mut shared = state.shared.lock();
    shared.last_error = Some(e.to_string());
}

fn marker_path_for(project_dir: &Path) -> PathBuf {
    project_dir.join("autosave").join("marker.json")
}

fn write_marker_with_counter(counter: u64) -> Result<()> {
    let (marker, marker_path) = with_workspace_mut(|ws| {
        let dir = ws.store.project_dir().to_path_buf();
        let marker = AutosaveMarker {
            project_path: dir.clone(),
            modified_at: ws.project.modified_at,
            written_at: Utc::now(),
            counter,
        };
        Ok((marker, marker_path_for(&dir)))
    })?;
    write_marker(&marker, &marker_path)
}

fn write_marker_for_current_project() -> Result<()> {
    let counter = state().shared.lock().counter;
    write_marker_with_counter(counter)
}

fn write_marker(marker: &AutosaveMarker, marker_path: &Path) -> Result<()> {
    if let Some(parent) = marker_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            DocumentBridgeError::Internal(format!(
                "create autosave dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    let bytes = serde_json::to_vec_pretty(marker)
        .map_err(|e| DocumentBridgeError::Internal(format!("serialize autosave marker: {e}")))?;
    // Atomic write: tmp + rename so a crash mid-write doesn't
    // leave a truncated marker.
    let mut tmp_path = marker_path.to_path_buf();
    tmp_path.set_extension("json.tmp");
    fs::write(&tmp_path, &bytes).map_err(|e| {
        DocumentBridgeError::Internal(format!(
            "write autosave tmp {}: {e}",
            tmp_path.display()
        ))
    })?;
    fs::rename(&tmp_path, marker_path).map_err(|e| {
        DocumentBridgeError::Internal(format!(
            "rename autosave marker {} -> {}: {e}",
            tmp_path.display(),
            marker_path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn marker_path_lives_under_autosave_subdir() {
        let dir = std::path::Path::new("/tmp/kc/proj.kstudio");
        let p = marker_path_for(dir);
        assert!(p.ends_with("autosave/marker.json"));
    }

    #[test]
    fn autosave_status_defaults_are_safe() {
        let st = autosave_status();
        let json = serde_json::to_string(&st).expect("serialize");
        let _: HashMap<String, serde_json::Value> =
            serde_json::from_str(&json).expect("parse back");
    }

    #[test]
    fn marker_round_trip_preserves_fields() {
        let m = AutosaveMarker {
            project_path: PathBuf::from("/tmp/a.kstudio"),
            modified_at: Utc::now(),
            written_at: Utc::now(),
            counter: 17,
        };
        let s = serde_json::to_string(&m).expect("ser");
        let back: AutosaveMarker = serde_json::from_str(&s).expect("de");
        assert_eq!(back.project_path, m.project_path);
        assert_eq!(back.counter, m.counter);
    }
}
