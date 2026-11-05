-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Each admin's backup key checks: when they last typed the key back, the
-- days until the next check, and "later". Not in backups, and kept by a
-- restore. See docs/backups.md#checking-the-key.

ALTER TABLE users
    ADD COLUMN backup_key_checked_at    DATETIME(6) NULL AFTER full_width,
    ADD COLUMN backup_key_interval_days SMALLINT UNSIGNED NULL AFTER backup_key_checked_at,
    ADD COLUMN backup_key_later_until   DATETIME(6) NULL AFTER backup_key_interval_days;
