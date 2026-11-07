-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Whether the web UI's On air page shows the whole queue, editable there,
-- for each user (off: the first tracks and a link to the Queue page).

ALTER TABLE users
    ADD COLUMN on_air_whole_queue BOOLEAN NOT NULL DEFAULT FALSE AFTER width;
