//! PURE mapping from a reconcile `Change` to a Controller mutation intent.
//!
//! Task 7 (watcher bridge): the GTK-thread apply function calls `intent_for` to
//! decide what action is needed for each incoming `Change`, then executes the
//! action on the Controller. Keeping the decision logic pure makes it testable
//! without a display or GTK.

use crate::core::reconcile::Change;

use super::note_entry::NoteId;

/// The intended mutation for a single reconcile `Change`.
///
/// The Controller's apply function dispatches on this intent:
/// - `Reload`  → re-read the file at the known path and update/insert the entry.
/// - `Drop`    → remove the entry + its widget from the surface.
/// - `Repath`  → update the entry's frozen path (no re-render needed).
// used in controller.rs apply_change + apply_change_to; wired Plan 5 Task 7
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub enum ApplyIntent {
    /// Added or Modified: (re)load the note from disk into an entry.
    Reload(NoteId),
    /// Removed: drop the entry and its widget.
    Drop(NoteId),
    /// Renamed: update the entry's frozen path; the content is unchanged.
    Repath { id: NoteId, new_path: String },
}

/// PURE: map one `Change` to the `ApplyIntent` the Controller should execute.
// used in controller.rs apply_change + apply_change_to (Plan 5 Task 7)
#[allow(dead_code)]
pub fn intent_for(change: &Change) -> ApplyIntent {
    match change {
        Change::Added(id) | Change::Modified(id) => ApplyIntent::Reload(id.clone()),
        Change::Removed(id) => ApplyIntent::Drop(id.clone()),
        Change::Renamed { id, new_path } => ApplyIntent::Repath {
            id: id.clone(),
            new_path: new_path.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::reconcile::Change;

    #[test]
    fn added_and_modified_both_reload() {
        assert_eq!(
            intent_for(&Change::Added("a".into())),
            ApplyIntent::Reload("a".into())
        );
        assert_eq!(
            intent_for(&Change::Modified("a".into())),
            ApplyIntent::Reload("a".into())
        );
    }

    #[test]
    fn removed_drops() {
        assert_eq!(
            intent_for(&Change::Removed("a".into())),
            ApplyIntent::Drop("a".into())
        );
    }

    #[test]
    fn renamed_repaths() {
        let c = Change::Renamed { id: "a".into(), new_path: "/x/a-new.md".into() };
        assert_eq!(
            intent_for(&c),
            ApplyIntent::Repath { id: "a".into(), new_path: "/x/a-new.md".into() }
        );
    }
}
