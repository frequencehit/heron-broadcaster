// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-librarian`: library management. New files, uploaded or dropped
//! into the incoming folder, wait in the buffer zone, identified, until
//! someone imports them; they are then tagged and filed into the library.
//! An existing library is reorganized on request, tracks are deleted
//! through a trash, and every change is journaled and can be undone. See
//! `docs/library.md`.
//!
//! It is the only service with write access to the library and the
//! incoming folder.

pub mod covers;
pub mod edit;
pub mod fsops;
pub mod identify;
pub mod import;
pub mod journal;
pub mod medley;
pub mod musicbrainz;
pub mod organize;
pub mod relations;
pub mod restore;
pub mod review;
pub mod staging;
pub mod tasks;
pub mod trash;

/// Where files go in the library, shared with `heron-api`, which checks the settings.
pub use heron_library::layout;
