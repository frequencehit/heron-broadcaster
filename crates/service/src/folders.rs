// SPDX-License-Identifier: AGPL-3.0-or-later

//! The folders a service writes to, checked: a folder it cannot write to
//! (the wrong owner, a read-only mount) is said at startup and in
//! `/healthz`, with the user the service runs as, rather than found by the
//! first upload (docs/library.md#folders-and-permissions).
//!
//! The check asks the kernel (`access(2)`), and writes nothing: a file
//! created to test the library or the incoming folder would be picked up by
//! their watchers.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::HealthCheck;

/// A folder the service writes to.
#[derive(Clone, Debug)]
pub struct Folder {
    /// What it is, for people: `the buffer zone`.
    pub name: &'static str,
    pub path: PathBuf,
}

impl Folder {
    pub fn new(name: &'static str, path: impl Into<PathBuf>) -> Self {
        Self {
            name,
            path: path.into(),
        }
    }

    /// `None` when the service can write to it; otherwise what to say.
    pub fn problem(&self) -> Option<String> {
        writable(&self.path)
            .err()
            .map(|error| not_writable(self.name, &self.path, &error))
    }
}

/// Whether this process can create files in `path`.
pub fn writable(path: &Path) -> io::Result<()> {
    if !path.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no such folder"));
    }
    rustix::fs::access(
        path,
        rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
    )
    .map_err(io::Error::from)
}

/// The user this process runs as: `uid 997, gid 988`.
pub fn user() -> String {
    format!(
        "uid {}, gid {}",
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw()
    )
}

/// What to say about a folder the service cannot write to (`error`, from
/// [`writable`] or a failed write): which, as whom, and what to do.
pub fn not_writable(name: &str, path: &Path, error: &io::Error) -> String {
    let fix = match error.kind() {
        io::ErrorKind::NotFound => "create it, and give it to HERON_LIBRARIAN_USER",
        io::ErrorKind::ReadOnlyFilesystem => "mount it read-write",
        _ => "give it to HERON_LIBRARIAN_USER (chown -R)",
    };
    format!(
        "{name} ({}) is not writable by {}: {fix} ({error})",
        path.display(),
        user()
    )
}

/// Whether `error`, from writing in a folder, says the folder is not ours
/// to write to, rather than a fault of the moment.
pub fn is_permission(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    )
}

/// The folders a service writes to.
#[derive(Clone, Debug, Default)]
pub struct Folders(Arc<Vec<Folder>>);

impl Folders {
    pub fn new(folders: Vec<Folder>) -> Self {
        Self(Arc::new(folders))
    }

    /// One line for each folder the service cannot write to, checked now.
    pub fn problems(&self) -> Vec<String> {
        self.0.iter().filter_map(Folder::problem).collect()
    }

    /// Logs each folder the service cannot write to, when it starts. It
    /// still starts: the air must not depend on a folder.
    pub fn log_problems(&self) {
        for problem in self.problems() {
            tracing::error!("{problem}");
        }
    }
}

/// A health check whose warnings also list the folders the service cannot
/// write to.
#[derive(Clone)]
pub struct WithFolders<H> {
    pub inner: H,
    pub folders: Folders,
}

impl<H: HealthCheck> HealthCheck for WithFolders<H> {
    async fn check(&self) -> Result<(), String> {
        self.inner.check().await
    }

    async fn warnings(&self) -> Vec<String> {
        let mut warnings = self.folders.problems();
        warnings.extend(self.inner.warnings().await);
        warnings
    }

    async fn details(&self) -> serde_json::Map<String, serde_json::Value> {
        self.inner.details().await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Root writes anywhere: these tests mean nothing as root.
    fn as_root() -> bool {
        rustix::process::geteuid().is_root()
    }

    #[test]
    fn a_folder_we_cannot_write_to_is_said() {
        if as_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        let folders = Folders::new(vec![Folder::new("the buffer zone", &staging)]);
        assert!(folders.problems().is_empty());

        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o555)).unwrap();
        let problems = folders.problems();
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with(&format!(
                "the buffer zone ({}) is not writable by uid ",
                staging.display()
            )),
            "{problems:?}"
        );
        assert!(
            problems[0].contains("give it to HERON_LIBRARIAN_USER"),
            "{problems:?}"
        );

        // Fixed: the warning goes, without a restart.
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(folders.problems().is_empty());
    }

    #[test]
    fn a_missing_folder_is_said() {
        let dir = tempfile::tempdir().unwrap();
        let problems =
            Folders::new(vec![Folder::new("the backups", dir.path().join("none"))]).problems();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("create it"), "{problems:?}");
    }
}
