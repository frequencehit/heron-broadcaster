-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The listener's country (ISO 3166-1 alpha-2), looked up in the local GeoIP
-- country database when the session started (docs/statistics.md#countries).
-- NULL: not found, or no database. The address itself is never stored.

ALTER TABLE listener_sessions
    ADD COLUMN country CHAR(2) NULL AFTER user_agent;
