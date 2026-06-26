use std::fs;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

use crate::core::content_hash::content_hash;
use crate::core::frontmatter;
use crate::core::id_policy::{IdGen, RawId, resolve};
use crate::core::note::Note;
use crate::platform::paths::Paths;

/// Geometry keys that belong in the layout file, not in the note's frontmatter.
const GEOMETRY_KEYS: &[&str] = &["x", "y", "w", "h", "output", "output_desc", "logical"];

/// A note that has been loaded from disk, with its resolved path and content hash.
pub struct LoadedNote {
    pub path: PathBuf,
    pub note: Note,
    pub disk_hash: String,
}

/// Read all `*.md` files (non-recursive, skipping `attachments/`) from notes_dir,
/// parse, resolve ids, and for any file whose id was assigned or whose filename
/// differs from `<id>-<slug>.md`, rewrite atomically to the correct path and
/// remove the old one. Returns the loaded notes with their final disk hashes.
pub fn load_all(paths: &Paths, gen: &mut impl IdGen) -> std::io::Result<Vec<LoadedNote>> {
    let notes_dir = paths.notes_dir();
    fs::create_dir_all(&notes_dir)?;

    // Clean up any leftover tmp-del files from interrupted renames.
    for entry in fs::read_dir(&notes_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("tmp-del") {
            let _ = fs::remove_file(&path); // best-effort
        }
    }

    // Collect (path, contents, original_hash) for all *.md files, skipping attachments/ and non-files.
    let mut files: Vec<(PathBuf, String, String)> = Vec::new();
    for entry in fs::read_dir(&notes_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip `attachments/` subdirectory entirely.
        if file_type.is_dir() && name_str == "attachments" {
            continue;
        }
        // Only process .md files directly in notes_dir.
        if !file_type.is_file() || !name_str.ends_with(".md") {
            continue;
        }
        let path = entry.path();
        let raw = fs::read(&path)?;
        let original_hash = content_hash(&raw);
        let contents = String::from_utf8_lossy(&raw).into_owned();
        files.push((path, contents, original_hash));
    }

    // Sort for deterministic ordering.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    // Parse all files and strip geometry keys from extra.
    let notes: Vec<Note> = files
        .iter()
        .map(|(_, c, _)| strip_geometry(frontmatter::parse(c)))
        .collect();

    // Build RawId slice — borrow from local strings to satisfy lifetimes.
    let path_keys: Vec<String> = files
        .iter()
        .map(|(p, _, _)| p.to_str().unwrap_or("").to_string())
        .collect();
    let fm_ids: Vec<String> = notes.iter().map(|n| n.id.clone()).collect();
    let filename_ids: Vec<Option<String>> = files
        .iter()
        .map(|(p, _, _)| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|s| filename_id_from(s))
                .map(|s| s.to_string())
        })
        .collect();

    let raw_ids: Vec<RawId> = (0..files.len())
        .map(|i| RawId {
            path_key: &path_keys[i],
            fm_id: &fm_ids[i],
            filename_id: filename_ids[i].as_deref(),
        })
        .collect();

    let resolved = resolve(&raw_ids, gen);

    // Build LoadedNotes, rewriting files that need it.
    let mut results = Vec::with_capacity(files.len());
    for (i, (old_path, _, original_hash)) in files.iter().enumerate() {
        let res = &resolved[i];
        let mut note = notes[i].clone();
        note.id = res.id.clone();

        let expected_filename = format!("{}-{}.md", note.id, note.slug());
        let expected_path = notes_dir.join(&expected_filename);

        // Rewrite + rename ONLY when the id was freshly assigned/deduped (first
        // touch): the file then needs its new id persisted and an id-prefixed
        // name. Do NOT rename just because the slug (derived from the H1) no
        // longer matches the filename — the spec forbids auto-rename on title
        // edits in v1 (avoids churn + broken external links / git history), so a
        // note whose title changed keeps its existing filename (frozen path).
        let (final_path, disk_hash) = if res.assigned {
            let bytes = frontmatter::serialize(&note).into_bytes();
            // Write-new-first ordering for crash safety. When the filename changed,
            // `old_path != expected_path` is guaranteed, so `expected_path` does not
            // exist yet and writing it never clobbers the old file. A crash after the
            // write but before removing the old file leaves both on disk briefly; the
            // old file is renamed to `.tmp-del` and the startup cleanup loop deletes
            // any stray `.tmp-del` on the next load. The note is never lost.
            atomic_write(&expected_path, &bytes)?;
            if old_path != &expected_path {
                let tmp = old_path.with_extension("tmp-del");
                fs::rename(old_path, &tmp)?;
                let _ = fs::remove_file(&tmp);
            }
            (expected_path, content_hash(&bytes))
        } else {
            // id is stable → keep the on-disk filename as-is (no auto-rename).
            (old_path.clone(), original_hash.clone())
        };

        results.push(LoadedNote {
            path: final_path,
            note,
            disk_hash,
        });
    }

    Ok(results)
}

/// Strip layout-only geometry keys from a note's extra map.
fn strip_geometry(mut note: Note) -> Note {
    for key in GEOMETRY_KEYS {
        note.extra.remove(*key);
    }
    note
}

/// Extract the potential id prefix from a filename `<id>-<slug>.md` or `<id>.md`.
///
/// Returns the segment before the first `-`, or the full stem if no `-` present.
/// Returns `None` when the stem is empty.
fn filename_id_from(filename: &str) -> Option<&str> {
    let stem = filename.strip_suffix(".md")?;
    let candidate = match stem.find('-') {
        Some(pos) => &stem[..pos],
        None => stem,
    };
    if candidate.is_empty() { None } else { Some(candidate) }
}

/// Outcome of a conflict-aware save operation.
pub enum SaveOutcome {
    /// The file was saved normally; contains the content hash written.
    Saved(String),
    /// A concurrent external change was detected; a conflict copy was written
    /// to the returned path and the original file was left untouched.
    Conflict(PathBuf),
}

/// Returns the content hash of `path` if it exists, or `None` if absent.
fn read_hash_if_exists(path: &Path) -> std::io::Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(content_hash(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Returns `true` when the on-disk state differs from what the caller expected.
///
/// `base_hash == ""` asserts absence: any present file is a conflict. Otherwise
/// a present file whose hash differs from `base_hash` is a conflict.
fn is_conflict(base_hash: &str, disk_hash: &Option<String>) -> bool {
    let Some(h) = disk_hash else {
        return false;
    };
    if base_hash.is_empty() {
        return true;
    }
    h != base_hash
}

/// Serialize `note` and write atomically to `target` (the note's frozen loaded
/// path), regardless of the note's current slug. Returns the content hash written.
pub fn save_to(target: &Path, note: &Note) -> std::io::Result<String> {
    let bytes = frontmatter::serialize(note).into_bytes();
    atomic_write(target, &bytes)?;
    Ok(content_hash(&bytes))
}

/// Path-aware optimistic-concurrency save: writes to `target` if the on-disk
/// file at `target` still hashes to `base_hash`; otherwise writes a conflict
/// copy next to `target` (`<stem>.conflict-<ts>.md`) and returns `Conflict`.
/// `base_hash == ""` asserts the target does not yet exist (present file → Conflict).
pub fn save_checked_to(
    target: &Path,
    note: &Note,
    base_hash: &str,
    ts: &str,
) -> std::io::Result<SaveOutcome> {
    let disk_hash = read_hash_if_exists(target)?;
    if is_conflict(base_hash, &disk_hash) {
        let conflict_path = write_conflict_next_to(target, note, ts)?;
        return Ok(SaveOutcome::Conflict(conflict_path));
    }
    let bytes = frontmatter::serialize(note).into_bytes();
    atomic_write(target, &bytes)?;
    Ok(SaveOutcome::Saved(content_hash(&bytes)))
}

/// Write a conflict copy alongside `target`: `<stem>.conflict-<ts>.md`.
fn write_conflict_next_to(target: &Path, note: &Note, ts: &str) -> std::io::Result<PathBuf> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("note");
    let conflict_path = parent.join(format!("{stem}.conflict-{ts}.md"));
    let bytes = frontmatter::serialize(note).into_bytes();
    atomic_write(&conflict_path, &bytes)?;
    Ok(conflict_path)
}

/// Atomic primitive: temp file in same dir → write → sync → rename → sync dir.
///
/// The temp file is placed in the same directory as `target` so the rename is
/// always within the same filesystem (required for atomicity on Linux). The temp
/// name is `<target-filename>.tmp-<pid>` so it remains human-readable on crash.
/// After the rename, the parent directory is fsynced to make the directory entry
/// durable. Platform errors on directory fsync are silently ignored (best-effort).
pub fn atomic_write(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path has no parent directory",
        )
    })?;

    // Build a temp path in the same directory — same filesystem, so rename is atomic.
    let file_name = target
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no filename"))?
        .to_string_lossy();
    let tmp_name = format!("{}.tmp-{}", file_name, std::process::id());
    let tmp_path = parent.join(&tmp_name);

    // Write, sync, then atomically rename into place.
    let mut f = fs::File::create(&tmp_path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp_path, target)?;

    // Best-effort: fsync the parent directory so the directory entry is durable.
    let _ = fs::File::open(parent).and_then(|f| f.sync_all());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::id_policy::IdGen;
    use crate::core::note::{Layer, Note};
    use crate::platform::paths::Paths;
    use std::collections::BTreeMap;

    struct SeqIdGen {
        n: u32,
    }
    impl SeqIdGen {
        fn new() -> Self {
            SeqIdGen { n: 0 }
        }
    }
    impl IdGen for SeqIdGen {
        fn new_id(&mut self) -> String {
            self.n += 1;
            format!("GEN{}", self.n)
        }
    }

    fn make_paths(base: &str) -> Paths {
        let base = base.to_string();
        Paths::from_env(|k| match k {
            "XDG_DATA_HOME" => Some(base.clone()),
            _ => None,
        })
    }

    fn make_note(id: &str) -> Note {
        let mut extra = BTreeMap::new();
        extra.insert(
            "custom_key".to_string(),
            serde_yaml_ng::Value::String("custom_val".to_string()),
        );
        Note {
            id: id.to_string(),
            color: "blue".to_string(),
            pinned: true,
            locked: false,
            layer: Layer::Desktop,
            tags: vec!["inbox".to_string(), "work".to_string()],
            extra,
            body: "# My Note\nsome body\n".to_string(),
        }
    }

    const VALID_ID: &str = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QK";

    #[test]
    fn atomic_write_replaces_existing_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.md");

        atomic_write(&target, b"first").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "first");

        atomic_write(&target, b"second").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "second");

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "only the final file, no temps: {entries:?}");
    }

    #[test]
    fn load_all_assigns_id_to_file_missing_id_and_rewrites_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        // File with empty id in frontmatter.
        let content = "---\nid: \"\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# No Id\n";
        fs::write(notes_dir.join("no-id.md"), content).unwrap();

        let mut gen = SeqIdGen::new();
        let loaded = load_all(&paths, &mut gen).unwrap();
        assert_eq!(loaded.len(), 1);
        let assigned_id = loaded[0].note.id.clone();
        assert!(!assigned_id.is_empty(), "id must be assigned");

        // Second load must find the same id (stable, not re-assigned).
        let mut gen2 = SeqIdGen::new();
        let loaded2 = load_all(&paths, &mut gen2).unwrap();
        assert_eq!(loaded2.len(), 1);
        assert_eq!(loaded2[0].note.id, assigned_id, "id must be stable on second load");
    }

    #[test]
    fn load_all_skips_attachments_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        let note = make_note(VALID_ID);
        let target = notes_dir.join(format!("{}-{}.md", VALID_ID, note.slug()));
        save_to(&target, &note).unwrap();

        // Plant a .md inside attachments/ — must not be loaded.
        let att_dir = notes_dir.join("attachments");
        fs::create_dir_all(&att_dir).unwrap();
        fs::write(att_dir.join("attachment.md"), "should not be loaded").unwrap();

        let mut gen = SeqIdGen::new();
        let loaded = load_all(&paths, &mut gen).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].note.id, VALID_ID);
    }

    #[test]
    fn disk_hash_is_original_file_bytes_not_reserialized() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        // Write a file manually with a valid ULID in both filename and frontmatter,
        // but with non-canonical YAML formatting (extra spaces, non-standard key order).
        // Re-serializing this note would produce different bytes.
        let non_canonical = "---\nid:   \"01JZ9P6S0R8ZX0G8N3Z4V7Y8QK\"\ncolor:  \"yellow\"\npinned:  false\nlayer:  \"front\"\ntags:  []\n---\n# Non-canonical\n";
        let filename = format!("{}-non-canonical.md", VALID_ID);
        fs::write(notes_dir.join(&filename), non_canonical).unwrap();

        let mut gen = SeqIdGen::new();
        let loaded = load_all(&paths, &mut gen).unwrap();
        assert_eq!(loaded.len(), 1);
        let ln = &loaded[0];

        let original_bytes = non_canonical.as_bytes();
        let expected_hash = content_hash(original_bytes);

        // disk_hash must equal hash of original file bytes (the no-rewrite path).
        assert_eq!(
            ln.disk_hash, expected_hash,
            "disk_hash must be hash of original file bytes, not re-serialized form"
        );

        // Confirm the re-serialized form would produce a different hash (the bug we fixed).
        let reserialized = frontmatter::serialize(&ln.note).into_bytes();
        let reserialized_hash = content_hash(&reserialized);
        assert_ne!(
            ln.disk_hash, reserialized_hash,
            "original bytes and re-serialized bytes must differ (proving the fix exercises the correct path)"
        );
    }

    #[test]
    fn load_all_no_duplicate_after_id_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        // File with missing id — load_all will assign one and rename it.
        let content = "---\nid: \"\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# Rename Test\n";
        fs::write(notes_dir.join("no-id.md"), content).unwrap();

        let mut gen = SeqIdGen::new();
        load_all(&paths, &mut gen).unwrap();

        // After the rewrite, exactly one .md file must exist (no ghost copy).
        let md_files: Vec<_> = fs::read_dir(&notes_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    == Some("md")
            })
            .collect();

        assert_eq!(
            md_files.len(),
            1,
            "expected exactly one .md file after id rewrite, found: {:?}",
            md_files.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );

        // The note content must survive the rename (write-new-first ordering): a
        // second load finds the body intact and the id now stable.
        let mut gen2 = SeqIdGen::new();
        let reloaded = load_all(&paths, &mut gen2).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].note.body, "# Rename Test\n");
        assert!(!reloaded[0].note.id.is_empty());

        // No leftover `.tmp-del` files in either pass.
        let stray: Vec<_> = fs::read_dir(&notes_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|x| x.to_str()) == Some("tmp-del")
            })
            .collect();
        assert!(stray.is_empty(), "no .tmp-del files should remain");
    }

    #[test]
    fn load_all_keeps_filename_when_only_the_slug_changed() {
        // Spec §4.3: no auto-rename on title edits in v1. A note with a valid,
        // stable id whose H1 (and thus slug) no longer matches its filename MUST
        // keep its on-disk filename — load_all must not rename it.
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        let id = "01HZX5K3P8QR7V9WMN2T4Y6BCD"; // valid ULID
        let content = format!(
            "---\nid: {id}\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# New Title\n"
        );
        let old_name = format!("{id}-old-slug.md");
        fs::write(notes_dir.join(&old_name), &content).unwrap();

        let mut gen = SeqIdGen::new();
        let loaded = load_all(&paths, &mut gen).unwrap();

        assert_eq!(loaded.len(), 1);
        assert!(
            notes_dir.join(&old_name).exists(),
            "original filename must be preserved (no auto-rename on title edit)"
        );
        assert!(
            !notes_dir.join(format!("{id}-new-title.md")).exists(),
            "must NOT rename the file to the new slug"
        );
        assert_eq!(
            loaded[0].path,
            notes_dir.join(&old_name),
            "LoadedNote.path keeps the on-disk filename"
        );
        assert_eq!(loaded[0].note.id, id, "id stays stable");
    }

    // ── save_to / save_checked_to tests ──────────────────────────────────────

    #[test]
    fn save_to_writes_to_given_path_even_when_slug_changed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(format!("{VALID_ID}-original-title.md"));

        let mut note = make_note(VALID_ID);
        note.body = "# A Totally Different Title\nbody\n".to_string();
        // slug() now differs from the filename — save_to must IGNORE the slug.
        assert_ne!(note.slug(), "original-title");

        let hash = save_to(&target, &note).unwrap();
        assert!(target.exists(), "must write to the frozen path, not a slug-derived one");

        // Exactly one .md file in the dir — no orphan was created.
        let md_count = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .count();
        assert_eq!(md_count, 1, "no orphan file from a slug-derived path");
        assert_eq!(hash, content_hash(&fs::read(&target).unwrap()));
    }

    #[test]
    fn save_checked_to_no_concurrent_change_overwrites_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(format!("{VALID_ID}-frozen.md"));
        let note = make_note(VALID_ID);
        let base = save_to(&target, &note).unwrap();

        let mut updated = make_note(VALID_ID);
        updated.body = "# Frozen\nnew body\n".to_string();
        let outcome = save_checked_to(&target, &updated, &base, "2026-01-01T000000").unwrap();
        assert!(matches!(outcome, SaveOutcome::Saved(_)));
        assert!(fs::read_to_string(&target).unwrap().contains("new body"));
        let conflicts = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".conflict-"))
            .count();
        assert_eq!(conflicts, 0);
    }

    #[test]
    fn save_checked_to_concurrent_change_writes_conflict_next_to_target_and_leaves_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(format!("{VALID_ID}-frozen.md"));
        let note = make_note(VALID_ID);
        let base = save_to(&target, &note).unwrap();

        // External edit on disk.
        fs::write(&target, "---\nid: \"01JZ9P6S0R8ZX0G8N3Z4V7Y8QK\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# External\n").unwrap();

        let mut in_mem = make_note(VALID_ID);
        in_mem.body = "# Frozen\nin-memory body\n".to_string();
        let outcome = save_checked_to(&target, &in_mem, &base, "2026-06-24T120000").unwrap();
        let conflict = match outcome {
            SaveOutcome::Conflict(p) => p,
            _ => panic!("expected Conflict"),
        };
        assert!(conflict.exists());
        assert!(conflict.file_name().unwrap().to_string_lossy().contains(".conflict-"));
        assert!(fs::read_to_string(&conflict).unwrap().contains("in-memory body"));
        assert!(
            fs::read_to_string(&target).unwrap().contains("External"),
            "original untouched"
        );
    }

    #[test]
    fn save_checked_to_conflict_filename_derives_from_target_stem() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(format!("{VALID_ID}-frozen.md"));
        let note = make_note(VALID_ID);
        let base = save_to(&target, &note).unwrap();

        // External edit to force conflict.
        fs::write(&target, "---\nid: \"01JZ9P6S0R8ZX0G8N3Z4V7Y8QK\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# External\n").unwrap();

        let mut in_mem = make_note(VALID_ID);
        in_mem.body = "# Changed Title\nbody\n".to_string();
        let outcome = save_checked_to(&target, &in_mem, &base, "2026-06-24T120000").unwrap();
        let conflict = match outcome {
            SaveOutcome::Conflict(p) => p,
            _ => panic!("expected Conflict"),
        };
        let conflict_name = conflict.file_name().unwrap().to_string_lossy().into_owned();
        // Must derive from the target stem ("…-frozen"), NOT from note.slug() ("changed-title").
        assert!(
            conflict_name.starts_with(&format!("{VALID_ID}-frozen")),
            "conflict filename must start with the target stem, got: {conflict_name}"
        );
        assert!(
            !conflict_name.contains("changed-title"),
            "conflict filename must NOT use slug(), got: {conflict_name}"
        );
    }

    #[test]
    fn save_checked_to_missing_target_with_empty_base_hash_returns_saved() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(format!("{VALID_ID}-frozen.md"));
        let note = make_note(VALID_ID);

        let outcome = save_checked_to(&target, &note, "", "2026-06-24T120000").unwrap();
        assert!(matches!(outcome, SaveOutcome::Saved(_)));
        assert!(target.exists());
    }

    #[test]
    fn geometry_keys_absent_from_loaded_note_extra() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        // File with every layout/geometry key in frontmatter (as might come from an
        // external editor or an older write). None of these belong in the `.md`.
        let content = "---\nid: \"\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\nx: 100\ny: 200\nw: 300\nh: 400\noutput: HDMI-1\noutput_desc: Some Monitor\nlogical: [0, 0, 1920, 1080]\n---\n# Geo Test\n";
        fs::write(notes_dir.join("geo-test.md"), content).unwrap();

        let mut gen = SeqIdGen::new();
        let loaded = load_all(&paths, &mut gen).unwrap();
        assert_eq!(loaded.len(), 1);
        let extra = &loaded[0].note.extra;

        for key in &["x", "y", "w", "h", "output", "output_desc", "logical"] {
            assert!(
                !extra.contains_key(*key),
                "geometry key '{key}' must not be in note.extra after load"
            );
        }
    }
}
