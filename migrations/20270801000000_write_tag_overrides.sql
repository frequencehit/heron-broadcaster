-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Writing the tags kept in Heron (track_tag_overrides) into the files, as
-- a library task (docs/library.md#keeping-tags-in-heron-instead-of-the-files).

ALTER TABLE library_tasks
    MODIFY COLUMN kind ENUM('organize_plan', 'organize_apply', 'undo', 'review_apply',
                            'artist_relations', 'edit_tags', 'musicbrainz_details',
                            'staging_import', 'delete', 'empty_trash', 'render_medley',
                            'preview_medley', 'fetch_covers', 'apply_cover',
                            'write_tag_overrides') NOT NULL;
