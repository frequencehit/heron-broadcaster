-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Exact times for clock slots (docs/rotation.md#exact-times): a time of the
-- hour, hard or soft, with the soft window and the medley fill. NULL: none.

ALTER TABLE clock_slots
    ADD COLUMN exact_time JSON NULL AFTER linked_to_next;
