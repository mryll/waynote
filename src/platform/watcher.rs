// platform::watcher is not yet wired into the binary entry point. Suppress
// dead-code warnings until main.rs integrates it. Remove once wired up.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::core::content_hash::content_hash;
use crate::core::frontmatter;
use crate::core::reconcile::{self, Change, Known, Scanned};
use crate::platform::paths::Paths;

// ── Shared state ─────────────────────────────────────────────────────────────

/// State shared between the app and the watcher background thread.
///
/// - `known`: the app's current belief about what is on disk (updated by the
///   app after each load/save/reconcile cycle).
/// - `own_writes`: content hashes that the app itself just wrote; the watcher
///   ignores any Scanned file whose hash appears here (own-write loop guard).
///   The watcher removes consumed hashes after each reconcile pass.
pub struct WatchState {
    pub known: Vec<Known>,
    pub own_writes: HashSet<String>,
}

impl WatchState {
    pub fn new(known: Vec<Known>) -> Self {
        WatchState { known, own_writes: HashSet::new() }
    }
}

// ── WatchGuard ────────────────────────────────────────────────────────────────

/// Stops the watcher when dropped.
///
/// Holds the `RecommendedWatcher` (which stops inotify/etc. on drop) and the
/// debounce thread handle (joined on drop to avoid leaked threads in tests).
pub struct WatchGuard {
    _watcher: RecommendedWatcher,
    thread: Option<std::thread::JoinHandle<()>>,
    stop_tx: std::sync::mpsc::SyncSender<()>,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        // Signal the debounce thread to stop, then join it.
        let _ = self.stop_tx.try_send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ── rescan ────────────────────────────────────────────────────────────────────

/// Scan `notes_dir` for `*.md` files and return a `Scanned` entry for each.
///
/// Skips the `attachments/` subdirectory and non-file entries. Each entry
/// contains the id parsed from the file's frontmatter and a SHA-256 hash of
/// the raw file bytes.
pub fn rescan(notes_dir: &Path) -> Vec<Scanned> {
    let entries = match std::fs::read_dir(notes_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for entry in entries.flatten() {
        if let Some(scanned) = scan_entry(&entry) {
            result.push(scanned);
        }
    }
    result
}

/// Try to produce a `Scanned` for one directory entry; return `None` to skip.
fn scan_entry(entry: &std::fs::DirEntry) -> Option<Scanned> {
    let file_type = entry.file_type().ok()?;
    let name = entry.file_name();
    let name_str = name.to_string_lossy();

    if file_type.is_dir() && name_str == "attachments" {
        return None;
    }
    if !file_type.is_file() || !name_str.ends_with(".md") {
        return None;
    }

    let path = entry.path();
    let bytes = std::fs::read(&path).ok()?;
    let id = extract_id(&bytes);
    let hash = content_hash(&bytes);
    Some(Scanned { id, path: path.to_string_lossy().into_owned(), hash })
}

/// Extract the note id from raw file bytes (via frontmatter parse).
/// Returns an empty string if unparseable.
fn extract_id(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    frontmatter::parse(&text).id
}

// ── watch ─────────────────────────────────────────────────────────────────────

/// Spawn a filesystem watcher on `paths.notes_dir()`.
///
/// On any inotify/FSEvents event, a debounce window of `debounce` is applied.
/// After the window closes, the watcher rescans the notes dir, reconciles
/// against `shared_state.known` (filtering out `shared_state.own_writes`),
/// and invokes `on_change(Vec<Change>)` with any detected changes. The
/// own_writes set is cleared after each reconcile pass (hashes consumed).
///
/// **Caller contract**: the watcher reconciles against `shared_state.known` but
/// never updates it. The caller MUST update `shared_state.known` after processing
/// each `Change`, or the next scan will re-emit the same events indefinitely.
///
/// Returns a `WatchGuard`; dropping it stops the watcher and joins the thread.
pub fn watch(
    paths: &Paths,
    debounce: Duration,
    shared_state: Arc<Mutex<WatchState>>,
    on_change: impl Fn(Vec<Change>) + Send + 'static,
) -> notify::Result<WatchGuard> {
    let notes_dir = paths.notes_dir();

    // Channel: notify → debounce thread.
    let (event_tx, event_rx) = std::sync::mpsc::channel::<()>();
    // Channel: WatchGuard::drop → debounce thread.
    let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel::<()>(1);

    let watcher = build_watcher(event_tx, &notes_dir)?;
    let thread = spawn_debounce_thread(event_rx, stop_rx, debounce, notes_dir, shared_state, on_change);

    Ok(WatchGuard { _watcher: watcher, thread: Some(thread), stop_tx })
}

/// Build and start the notify watcher.
fn build_watcher(
    event_tx: std::sync::mpsc::Sender<()>,
    notes_dir: &Path,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |_res: notify::Result<notify::Event>| {
        let _ = event_tx.send(());
    })?;
    watcher.watch(notes_dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// Spawn the debounce-and-reconcile thread; returns its handle.
fn spawn_debounce_thread(
    event_rx: std::sync::mpsc::Receiver<()>,
    stop_rx: std::sync::mpsc::Receiver<()>,
    debounce: Duration,
    notes_dir: PathBuf,
    shared_state: Arc<Mutex<WatchState>>,
    on_change: impl Fn(Vec<Change>) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        run_debounce_loop(&event_rx, &stop_rx, debounce, &notes_dir, &shared_state, &on_change);
    })
}

/// Main loop: wait for an event, drain the debounce window, reconcile.
fn run_debounce_loop(
    event_rx: &std::sync::mpsc::Receiver<()>,
    stop_rx: &std::sync::mpsc::Receiver<()>,
    debounce: Duration,
    notes_dir: &Path,
    shared_state: &Arc<Mutex<WatchState>>,
    on_change: &impl Fn(Vec<Change>),
) {
    loop {
        // Block until either an fs event arrives or a stop signal.
        let got_event = wait_for_event(event_rx, stop_rx);
        if !got_event {
            return; // stop signal received
        }
        drain_debounce_window(event_rx, stop_rx, debounce);
        reconcile_and_dispatch(notes_dir, shared_state, on_change);
    }
}

/// Wait for the first event or a stop signal.
/// Returns `true` if an event arrived, `false` if stop was signalled.
fn wait_for_event(
    event_rx: &std::sync::mpsc::Receiver<()>,
    stop_rx: &std::sync::mpsc::Receiver<()>,
) -> bool {
    // Poll both channels with a short timeout to remain responsive.
    loop {
        if event_rx.recv_timeout(Duration::from_millis(50)).is_ok() {
            return true;
        }
        if stop_rx.try_recv().is_ok() {
            return false;
        }
    }
}

/// Drain remaining events within `debounce` window; stop if stop signal arrives.
fn drain_debounce_window(
    event_rx: &std::sync::mpsc::Receiver<()>,
    stop_rx: &std::sync::mpsc::Receiver<()>,
    debounce: Duration,
) {
    let deadline = std::time::Instant::now() + debounce;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if stop_rx.try_recv().is_ok() {
            break;
        }
        let _ = event_rx.recv_timeout(remaining.min(Duration::from_millis(20)));
    }
}

/// Rescan, reconcile, update own_writes, call on_change.
fn reconcile_and_dispatch(
    notes_dir: &Path,
    shared_state: &Arc<Mutex<WatchState>>,
    on_change: &impl Fn(Vec<Change>),
) {
    let scanned = rescan(notes_dir);

    let mut state = match shared_state.lock() {
        Ok(s) => s,
        Err(_) => return,
    };

    let changes = reconcile::reconcile(&state.known, &scanned, &state.own_writes);
    // Consume ONLY the own-write hashes this scan actually observed. A recorded
    // own-write hash survives until a scan sees it, so an own-write whose fs
    // events split across two debounce cycles is never spuriously reported.
    let consumed: Vec<String> = scanned
        .iter()
        .map(|s| &s.hash)
        .filter(|h| state.own_writes.contains(*h))
        .cloned()
        .collect();
    for h in &consumed {
        state.own_writes.remove(h);
    }
    drop(state);

    if !changes.is_empty() {
        on_change(changes);
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::Paths;
    use std::fs;

    fn make_paths(base: &str) -> Paths {
        let base = base.to_string();
        Paths::from_env(|k| match k {
            "XDG_DATA_HOME" => Some(base.clone()),
            _ => None,
        })
    }

    /// Minimal frontmatter + body with the given id.
    fn note_content(id: &str) -> String {
        format!(
            "---\nid: \"{id}\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# Test Note\n"
        )
    }

    // ── rescan unit tests ────────────────────────────────────────────────────

    #[test]
    fn rescan_returns_scanned_for_each_md_and_skips_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let notes_dir = dir.path();

        let id_a = "01AAAAAAAAAAAAAAAAAAAAAAAAAA";
        let id_b = "01BBBBBBBBBBBBBBBBBBBBBBBBBB";

        fs::write(notes_dir.join("a.md"), note_content(id_a)).unwrap();
        fs::write(notes_dir.join("b.md"), note_content(id_b)).unwrap();

        // attachments/ subdir must be skipped entirely.
        let att = notes_dir.join("attachments");
        fs::create_dir_all(&att).unwrap();
        fs::write(att.join("img.md"), "should be ignored").unwrap();

        let mut result = rescan(notes_dir);
        result.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(result.len(), 2, "must find exactly 2 notes (attachments skipped)");
        assert_eq!(result[0].id, id_a);
        assert_eq!(result[1].id, id_b);
    }

    #[test]
    fn rescan_hashes_match_raw_file_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let notes_dir = dir.path();
        let id = "01CCCCCCCCCCCCCCCCCCCCCCCCCC";
        let content = note_content(id);
        fs::write(notes_dir.join("c.md"), &content).unwrap();

        let result = rescan(notes_dir);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].hash, content_hash(content.as_bytes()));
    }

    // ── watch integration tests ──────────────────────────────────────────────

    /// Drain the channel until `predicate` matches one of the Change batches,
    /// or `timeout` elapses. Returns the matching batch.
    fn await_change(
        rx: &std::sync::mpsc::Receiver<Vec<Change>>,
        timeout: Duration,
        predicate: impl Fn(&Vec<Change>) -> bool,
    ) -> Option<Vec<Change>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
                Ok(changes) if predicate(&changes) => return Some(changes),
                Ok(_) => {} // spurious / coalesced batch — keep waiting
                Err(_) => {}
            }
        }
    }

    #[test]
    fn watch_detects_add_modify_remove_via_real_fs_events() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        let shared = Arc::new(Mutex::new(WatchState::new(vec![])));
        let (tx, rx) = std::sync::mpsc::channel::<Vec<Change>>();

        let guard = watch(
            &paths,
            Duration::from_millis(80),
            Arc::clone(&shared),
            move |changes| { let _ = tx.send(changes); },
        )
        .expect("watcher must start");

        let timeout = Duration::from_secs(8);
        let id = "01DDDDDDDDDDDDDDDDDDDDDDDDDD";
        let note_path = notes_dir.join(format!("{id}-test-note.md"));
        let content = note_content(id);

        // ── Added ──────────────────────────────────────────────────────────
        fs::write(&note_path, &content).unwrap();

        let added = await_change(&rx, timeout, |cs| {
            cs.iter().any(|c| matches!(c, Change::Added(i) if i == id))
        });
        assert!(added.is_some(), "Expected Added({id}) within {timeout:?}");

        // Update known state so the next reconcile sees this note as known.
        {
            let mut state = shared.lock().unwrap();
            state.known.push(Known {
                id: id.to_string(),
                path: note_path.to_string_lossy().into_owned(),
                hash: content_hash(content.as_bytes()),
            });
        }

        // ── Modified ──────────────────────────────────────────────────────
        let modified_content = note_content(id) + "extra line\n";
        fs::write(&note_path, &modified_content).unwrap();

        let modified = await_change(&rx, timeout, |cs| {
            cs.iter().any(|c| matches!(c, Change::Modified(i) if i == id))
        });
        assert!(modified.is_some(), "Expected Modified({id}) within {timeout:?}");

        // Update known hash.
        {
            let mut state = shared.lock().unwrap();
            if let Some(k) = state.known.iter_mut().find(|k| k.id == id) {
                k.hash = content_hash(modified_content.as_bytes());
            }
        }

        // ── Removed ───────────────────────────────────────────────────────
        fs::remove_file(&note_path).unwrap();

        let removed = await_change(&rx, timeout, |cs| {
            cs.iter().any(|c| matches!(c, Change::Removed(i) if i == id))
        });
        assert!(removed.is_some(), "Expected Removed({id}) within {timeout:?}");

        drop(guard); // stop watcher + join thread
    }
}
