-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Daypart separation is set per category only (docs/rotation.md#daypart-separation):
-- a station's `separation.dayparts` goes onto its categories of the Song type
-- (type 1, whatever its name now) that have no value of their own, then
-- leaves the station's `rotation` settings. Jingles, ads, the station's own
-- types and untyped categories get none: they must be able to play at any
-- hour. A category shared by stations with different values takes the
-- highest. Nothing changes where no station set it.
--
-- A configuration backup of an older schema runs this too when restored.

UPDATE categories c
JOIN (
    SELECT cs.category_id,
           MAX(CAST(JSON_VALUE(ss.value, '$.separation.dayparts') AS UNSIGNED)) AS dayparts
    FROM category_stations cs
    JOIN station_settings ss ON ss.station_id = cs.station_id AND ss.name = 'rotation'
    GROUP BY cs.category_id
) station ON station.category_id = c.id
SET c.separation = JSON_SET(COALESCE(c.separation, '{}'), '$.dayparts', station.dayparts)
WHERE c.type_id = 1
  AND station.dayparts > 0
  AND JSON_VALUE(c.separation, '$.dayparts') IS NULL;

UPDATE station_settings
SET value = JSON_REMOVE(value, '$.separation.dayparts')
WHERE name = 'rotation'
  AND JSON_EXTRACT(value, '$.separation.dayparts') IS NOT NULL;
