-- SPDX-License-Identifier: AGPL-3.0-or-later

-- *Recalculate now* on the attempts page (docs/rotation.md#the-attempts-page).
ALTER TABLE exact_calculations
    MODIFY reason ENUM('first', 'restart', 'queue_changed', 'hand_dropped', 'automatic',
                       'by_hand', 'followed') NOT NULL;
