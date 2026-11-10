-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Each user's status bar settings (docs/web.md#status-bar), as JSON: the
-- parts shown, the time on the decks, the countdowns. NULL: the defaults.

ALTER TABLE users
    ADD COLUMN status_bar JSON NULL AFTER on_air_whole_queue;
