-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Calculate now (docs/rotation.md#the-attempts-page): a calculation made on
-- demand for the upcoming exact time, and who asked for it.

ALTER TABLE exact_calculations
    MODIFY COLUMN reason ENUM('first', 'restart', 'queue_changed', 'hand_dropped', 'automatic',
                              'followed', 'on_demand') NOT NULL,
    ADD COLUMN requested_by BIGINT UNSIGNED NULL AFTER follows_id,
    ADD CONSTRAINT exact_calculations_requested_by FOREIGN KEY (requested_by) REFERENCES users (id) ON DELETE SET NULL;
