-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- A match on every track, its tracks from the hour's clock
-- (docs/listener-interaction.md#match): what the clock puts between the
-- vote's track and the winner, and an announcement queued before a vote's
-- track queued already (the winner of the match before), locked with the
-- match's tracks.

ALTER TABLE matches
    ADD COLUMN jingle_entry_id BIGINT UNSIGNED NULL AFTER entry_b_id,
    ADD COLUMN between_entry_ids JSON NULL AFTER jingle_entry_id;
