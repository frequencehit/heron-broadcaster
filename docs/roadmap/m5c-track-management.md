# M5c – Track management

**Depends on:** M5. Builds on the librarian of [M1b](m1b-library-management.md).

Managing the library from the browser: uploading new music, checking it before it goes on air, and removing tracks for good. Today files reach the library only through the incoming folder, imported as soon as they settle, and nothing is ever deleted. With M5c, every new file waits in a **buffer zone** where it is identified and checked, and is imported only when someone confirms it; tracks can be deleted with their files, through a trash that can be restored.

## Scope

### Buffer zone

- A folder of its own (`HERON_STAGING_FOLDER`), outside the library. `heron-api` writes uploads into it; `heron-librarian` reads it and imports from it. The library stays writable by the librarian only.
- Files arrive by upload or from the incoming folder. The files of one upload, or of one incoming folder, form a **group**, so the files of an album still agree on their release.
- For each file, the librarian reads its tags and duration, fingerprints it and identifies it (AcoustID, then MusicBrainz, with the rules and rate limits of [Library management](../library.md#identification)). The buffer shows, before anything is imported:
  - the tags it would write and the cover;
  - the path the layout gives it in the library;
  - the candidate releases when the match is not confident.
- Files are **flagged** when they:
  - duplicate a library track (same content hash or fingerprint), or another file of the buffer;
  - cannot be decoded, or are too short to fingerprint;
  - are not in a supported format.
- On a file or a selection:
  - pick another candidate release, or keep the file's own tags (with [M5b](m5b-track-details-tags.md), edit them);
  - listen to it;
  - **import**: a library task, run as an `import` batch like today, so it can be undone;
  - **discard**: the file is removed from the buffer. It never reached the library.
- Confident matches are selected already, so a clean album is imported in one click.

### Uploads

- Drag files or folders onto the page, several at once, with each file's progress.
- Uploads are sent in **chunks and resume** after a dropped connection, so large FLAC files and slow links work; the `heron-web` proxy and its body size limit must allow them.
- Accepted: the scanner's audio extensions. A size limit per file and a free-space check refuse what would fill the disk (settings).
- A file is written under a temporary name and shows in the buffer only once complete.

### Incoming folder

- Files dropped into the incoming folder are moved into the buffer once they settle (same settle rule, journaled like any move) instead of being imported directly. They wait there like uploads.

### Deleting tracks

- Delete one track or a selection, from the library and from a track's page. Tracks whose file is missing can be deleted too, which cleans up the library after files were removed by hand.
- Deleting is a library task: the librarian moves each file to the **trash**, `.heron-trash/<batch>/…` at the library root (same file system, so a rename; the scanner skips hidden folders), journaled as a `delete` batch.
- A deleted track:
  - disappears from the library, from rotation and from its categories;
  - leaves the queue (the queue manager drops it, as it does for missing files);
  - plays on if it is on air: its file is already open.
- Its analysis, categories, overrides and history are kept until it is purged, so **restoring** it (undoing the batch) brings it back whole. A path taken since is reported, like any undo.
- After `trash_days` (a setting, 30 by default), the librarian **purges** the trash: the files are removed, the tracks deleted, and their plays keep their artist and title (`play_history.track_id` becomes NULL, as already planned for deleted tracks). Admins can also empty the trash at once.
- Files that are not tracks (`cover.jpg`) go to the trash with the last track of their folder; emptied folders are removed.

### Web UI

- An **Import music** page (first named *Buffer*): uploads, groups, identification, flags, import and discard.
- Delete actions on the **Tracks** and **Track** pages, with a confirmation that lists what will go.
- A **Trash** view: deleted tracks, when they will be purged, restore, empty the trash.
- The review on *To organize* stays for tracks already in the library.
- Operators upload, import, discard and delete; admins empty the trash and change the settings.

## Acceptance criteria

- [x] Uploading 100 files from the browser (tagged, badly tagged, untagged, a duplicate, a broken file) puts them in the buffer, identified, with their planned path and flags. Nothing reaches the library before it is imported.

  The 100 files of M1b's acceptance test (tagged, badly tagged, untagged; against a mock of AcoustID and MusicBrainz) go through the buffer zone and wait there, identified, with their planned paths, until imported (`librarian/tests/import.rs`); duplicates in the library and in the buffer, re-tagged copies, broken and too short files are flagged (`librarian/tests/staging.rs`); uploads go through the same pipeline (`api` end-to-end test, the Compose CI job, and uploads from Chrome against a running station). The 100 files were not uploaded through a browser in one go.
- [x] Importing a group files and tags it like an M1b import, as one batch that can be undone. Discarding removes files from the buffer only.
- [x] Files dropped into the incoming folder wait in the buffer instead of being imported.
- [x] An upload interrupted in the middle of a file resumes; an incomplete upload never shows as ready.

  Unit tests of the uploader (a connection dropping mid-chunk), the API end-to-end test, and `scripts/check-buffer.sh` in the Compose CI job.
- [x] Deleting 50 tracks (one on air, some queued, one missing) moves their files to the trash and takes them out of the queue and rotation; the one on air plays to its end. Restoring them brings their files back byte for byte, with their analysis, categories and history.

  51 tracks, one of them missing, deleted and restored byte for byte with their analysis, categories and plays (`librarian/tests/trash.rs`); a track on air (its play ends as `finished`), a queued one and a missing one deleted through the API with the engine running (`api` end-to-end test).
- [x] After the retention period (time faked in tests), the trash is purged; the play history keeps their plays with artist and title.
- [x] Interrupting the librarian (chaos test) while it imports from the buffer, deletes or purges loses no file it should keep, and leaves no track pointing at a missing path.

  `librarian/tests/chaos.rs` crashes and kills the binary while it moves incoming files into the buffer, imports, organizes, undoes, deletes, restores and purges.

## Results

- **Buffer zone** ([Library management](../library.md#the-buffer-zone)): `HERON_STAGING_FOLDER`, written by `heron-api` (uploads) and `heron-librarian` (incoming folder); both run as `HERON_LIBRARIAN_USER`. Files are grouped (one upload, one incoming folder), identified once complete, flagged (in the library, twice here, same recording, unreadable, too short, unsupported), and show the tags and path they would get. A release picked by hand, or the file's own tags, is worked out again by the librarian. Imports are `import` batches; discarding happens in the API.
- **Uploads**: resumable chunks of up to 16 MiB at the offset the server has (`PUT /staging/uploads/{id}?offset=`), through an nginx location that passes them on unbuffered; limits per file and free space to keep are settings (2 GiB, 5 GiB). Abandoned uploads are given up after a day.
- **Trash** ([Library management](../library.md#deleting-tracks-and-the-trash)): `delete` batches move files to `.heron-trash/<batch>/`, and the journal marks a track deleted (and missing) in the same step that records its file there, so deletion and restore are crash-safe. Files that are not tracks follow the last track of their folder. Restoring is undoing the batch. The librarian purges tracks older than `trash_days` once an hour; admins can empty the trash.
- **Web UI**: *Import music* (first named *Buffer*) and *Trash* pages, deletion from *Tracks* and *Track*, upload and trash settings.
- **Answers to the open question**: 2 GiB per file and 5 GiB kept free (no separate quota), 30 days in the trash, operators delete and restore, admins empty the trash, buffer files never expire.
- Deferred: editing tags in the buffer zone (done in [M5e](m5e-buffer-preparation.md)); restoring one track of a deletion rather than the whole batch; resuming an upload after the page is reloaded.

## References

- [Library management](../library.md)
- [Web UI](../web.md)
- [Data model](../data-model.md)
- [Decisions – Library files](../decisions.md#decisions-made-after-the-handoff)
- [Open questions](../open-questions.md)
