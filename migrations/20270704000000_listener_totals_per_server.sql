-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Listener totals per server read (the master or a relay), as well as for
-- every server (server_id NULL), so that the statistics can be read for
-- one server (docs/statistics.md#relay-servers). Not a foreign key: the
-- totals stay when a relay is deleted. heron-api recomputes the days still
-- covered by the sessions and samples kept when it first starts.

ALTER TABLE listener_totals
    ADD COLUMN server_id BIGINT UNSIGNED NULL AFTER output_id,
    ADD COLUMN server_key BIGINT UNSIGNED AS (COALESCE(server_id, 0)) STORED AFTER output_key,
    DROP KEY listener_totals_row,
    ADD UNIQUE KEY listener_totals_row (station_id, day, output_key, server_key, hour_key);
