-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The web UI's width for each user: the centred column (normal), 80 % of
-- the window (wide) or the whole window (full), instead of full_width.

ALTER TABLE users
    ADD COLUMN width ENUM('normal', 'wide', 'full') NOT NULL DEFAULT 'normal' AFTER language;

UPDATE users SET width = 'full' WHERE full_width;

ALTER TABLE users DROP COLUMN full_width;
