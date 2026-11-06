-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Clock slots linked to the next slot (docs/rotation.md#linked-slots): a
-- jingle that plays only if the next slot's track plays right after it.

ALTER TABLE clock_slots
    ADD COLUMN linked_to_next BOOLEAN NOT NULL DEFAULT FALSE AFTER separation;
