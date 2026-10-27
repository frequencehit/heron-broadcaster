-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Types whose tracks send no metadata: while one plays, the stream servers
-- and the public now-playing keep the last track that does (the song before
-- a jingle). Off for Jingle and Ad. See docs/outputs.md#metadata.

ALTER TABLE category_types
    ADD COLUMN sends_metadata BOOLEAN NOT NULL DEFAULT TRUE AFTER no_two_in_a_row;

UPDATE category_types SET sends_metadata = FALSE WHERE name IN ('Jingle', 'Ad');
