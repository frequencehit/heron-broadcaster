// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watching a folder for changes, without reacting to reads.
//!
//! On Linux, `notify` also reports `IN_OPEN`, `IN_ACCESS` and
//! `IN_CLOSE_NOWRITE` as access events. A scan opens every directory, so
//! counting those as changes makes each scan trigger the next one.

use std::path::Path;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::AccessKind};

/// Whether an event means the folder's content changed.
///
/// Reads (open, access, close after read) do not. Closing a file after a
/// write does, and so does any event of a kind we do not know.
#[must_use]
pub fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(mode)) => {
            matches!(mode, notify::event::AccessMode::Write)
        }
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Watches `dir` recursively and calls `on_change` for every event that is
/// a change, see [`is_change`]. Dropping the watcher stops it.
///
/// # Errors
///
/// Returns the error from `notify` when the watcher cannot be created or
/// the folder cannot be watched.
pub fn watch_dir(
    dir: &Path,
    on_change: impl Fn() + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let handler = move |event: notify::Result<Event>| {
        if event.is_ok_and(|event| is_change(&event.kind)) {
            on_change();
        }
    };
    let mut watcher = notify::recommended_watcher(handler)?;
    watcher.watch(dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessMode, CreateKind, DataChange, ModifyKind, RenameMode};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn reads_are_not_changes() {
        assert!(!is_change(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!is_change(&EventKind::Access(AccessKind::Any)));
    }

    #[test]
    fn writes_are_changes() {
        assert!(is_change(&EventKind::Create(CreateKind::File)));
        assert!(is_change(&EventKind::Remove(
            notify::event::RemoveKind::File
        )));
        assert!(is_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(is_change(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
        assert!(is_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(is_change(&EventKind::Other));
        assert!(is_change(&EventKind::Any));
    }

    /// Waits for the watcher to go quiet and counts what it reported.
    fn drain(events: &mpsc::Receiver<()>, wait: Duration) -> usize {
        let mut count = 0;
        while events.recv_timeout(wait).is_ok() {
            count += 1;
        }
        count
    }

    #[test]
    fn reading_a_tree_is_silent_and_changes_are_reported() {
        let root = tempfile::tempdir().unwrap();
        let album = root.path().join("artist/album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("a.mp3"), b"first").unwrap();

        let (tx, events) = mpsc::channel();
        let _watcher = watch_dir(root.path(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        drain(&events, Duration::from_millis(500));

        // What a scan does: list every directory, read every file.
        for _ in 0..3 {
            for entry in walk(root.path()) {
                if entry.is_file() {
                    std::fs::read(&entry).unwrap();
                }
            }
        }
        assert_eq!(drain(&events, Duration::from_millis(500)), 0);

        std::fs::write(album.join("b.mp3"), b"new").unwrap();
        assert!(drain(&events, Duration::from_millis(500)) > 0, "create");

        std::fs::write(album.join("a.mp3"), b"changed").unwrap();
        assert!(drain(&events, Duration::from_millis(500)) > 0, "modify");

        std::fs::rename(album.join("a.mp3"), root.path().join("moved.mp3")).unwrap();
        assert!(drain(&events, Duration::from_millis(500)) > 0, "move");

        std::fs::remove_file(album.join("b.mp3")).unwrap();
        assert!(drain(&events, Duration::from_millis(500)) > 0, "delete");
    }

    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path));
            }
            found.push(path);
        }
        found
    }
}
