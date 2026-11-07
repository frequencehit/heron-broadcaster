-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Groups the Icecast outputs still holding their server's settings (their
-- `config` has a `host`) into stream servers (docs/upgrading.md#edge):
--
-- - one server per station for each kind, address, port, source login and
--   password and admin credentials, named after its address (`host:port`,
--   numbered when the name is taken), with the public address of its first
--   output;
-- - a station with no station details yet gets the details its outputs send
--   most (each field on its own, the first output's on a tie);
-- - each output keeps as its own overrides the details that differ from the
--   station's (an empty text where it sent none), and only its mount in
--   `config`.
--
-- Nothing changes on air: the API builds the same settings for the engine.
-- A configuration backup of an older schema runs this too when restored;
-- outputs already on a server are left as they are.

-- The servers.
INSERT INTO stream_servers
    (station_id, name, host, port, source_username, source_password, admin_username,
     admin_password, public_url)
WITH legacy AS (
    SELECT o.id, o.station_id,
           JSON_VALUE(o.config, '$.host') AS host,
           COALESCE(CAST(JSON_VALUE(o.config, '$.port') AS UNSIGNED), 8000) AS port,
           COALESCE(NULLIF(JSON_VALUE(o.config, '$.username'), ''), 'source') AS source_username,
           JSON_VALUE(o.config, '$.password') AS source_password,
           IF(NULLIF(JSON_VALUE(o.statistics, '$.admin_password'), '') IS NULL, NULL,
              COALESCE(NULLIF(JSON_VALUE(o.statistics, '$.admin_username'), ''), 'admin')) AS admin_username,
           NULLIF(JSON_VALUE(o.statistics, '$.admin_password'), '') AS admin_password,
           NULLIF(JSON_VALUE(o.config, '$.public_url'), '') AS public_url
    FROM outputs o
    WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL
),
servers AS (
    SELECT station_id, host, port, source_username, source_password, admin_username,
           admin_password, MIN(id) AS first_id
    FROM legacy
    GROUP BY station_id, host, port, source_username, source_password, admin_username,
             admin_password
),
numbered AS (
    SELECT s.*,
           ROW_NUMBER() OVER (PARTITION BY s.station_id, s.host, s.port ORDER BY s.first_id) AS n
    FROM servers s
)
SELECT n.station_id,
       IF(n.n = 1, CONCAT(n.host, ':', n.port), CONCAT(n.host, ':', n.port, ' (', n.n, ')')),
       n.host, n.port, n.source_username, n.source_password, n.admin_username, n.admin_password,
       (SELECT l.public_url FROM legacy l WHERE l.id = n.first_id)
FROM numbered n
ORDER BY n.station_id, n.first_id;

-- The station details: the most sent value of each field.
INSERT INTO station_settings (station_id, name, value)
SELECT st.station_id, 'stream_details', JSON_OBJECT(
    'name', (SELECT NULLIF(JSON_VALUE(o.config, '$.name'), '') AS v FROM outputs o
             WHERE o.station_id = st.station_id AND o.kind = 'icecast' AND o.server_id IS NULL
               AND JSON_VALUE(o.config, '$.host') IS NOT NULL
             GROUP BY v ORDER BY COUNT(*) DESC, MIN(o.id) LIMIT 1),
    'description', (SELECT NULLIF(JSON_VALUE(o.config, '$.description'), '') AS v FROM outputs o
             WHERE o.station_id = st.station_id AND o.kind = 'icecast' AND o.server_id IS NULL
               AND JSON_VALUE(o.config, '$.host') IS NOT NULL
             GROUP BY v ORDER BY COUNT(*) DESC, MIN(o.id) LIMIT 1),
    'genre', (SELECT NULLIF(JSON_VALUE(o.config, '$.genre'), '') AS v FROM outputs o
             WHERE o.station_id = st.station_id AND o.kind = 'icecast' AND o.server_id IS NULL
               AND JSON_VALUE(o.config, '$.host') IS NOT NULL
             GROUP BY v ORDER BY COUNT(*) DESC, MIN(o.id) LIMIT 1),
    'url', (SELECT NULLIF(JSON_VALUE(o.config, '$.url'), '') AS v FROM outputs o
             WHERE o.station_id = st.station_id AND o.kind = 'icecast' AND o.server_id IS NULL
               AND JSON_VALUE(o.config, '$.host') IS NOT NULL
             GROUP BY v ORDER BY COUNT(*) DESC, MIN(o.id) LIMIT 1),
    'public', IF((SELECT COALESCE(JSON_EXTRACT(o.config, '$.public') = 'true', FALSE) AS v FROM outputs o
             WHERE o.station_id = st.station_id AND o.kind = 'icecast' AND o.server_id IS NULL
               AND JSON_VALUE(o.config, '$.host') IS NOT NULL
             GROUP BY v ORDER BY COUNT(*) DESC, MIN(o.id) LIMIT 1),
             JSON_EXTRACT('true', '$'), JSON_EXTRACT('false', '$')))
FROM (SELECT DISTINCT station_id FROM outputs
      WHERE kind = 'icecast' AND server_id IS NULL AND JSON_VALUE(config, '$.host') IS NOT NULL) st
WHERE NOT EXISTS (SELECT 1 FROM station_settings ss
                  WHERE ss.station_id = st.station_id AND ss.name = 'stream_details');

-- The details each output overrides: those that differ from the station's.
UPDATE outputs o
JOIN station_settings ss ON ss.station_id = o.station_id AND ss.name = 'stream_details'
SET o.details = JSON_SET(COALESCE(o.details, '{}'), '$.name',
                         COALESCE(NULLIF(JSON_VALUE(o.config, '$.name'), ''), ''))
WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL
  AND NOT (NULLIF(JSON_VALUE(o.config, '$.name'), '') <=> NULLIF(JSON_VALUE(ss.value, '$.name'), ''));

UPDATE outputs o
JOIN station_settings ss ON ss.station_id = o.station_id AND ss.name = 'stream_details'
SET o.details = JSON_SET(COALESCE(o.details, '{}'), '$.description',
                         COALESCE(NULLIF(JSON_VALUE(o.config, '$.description'), ''), ''))
WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL
  AND NOT (NULLIF(JSON_VALUE(o.config, '$.description'), '')
           <=> NULLIF(JSON_VALUE(ss.value, '$.description'), ''));

UPDATE outputs o
JOIN station_settings ss ON ss.station_id = o.station_id AND ss.name = 'stream_details'
SET o.details = JSON_SET(COALESCE(o.details, '{}'), '$.genre',
                         COALESCE(NULLIF(JSON_VALUE(o.config, '$.genre'), ''), ''))
WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL
  AND NOT (NULLIF(JSON_VALUE(o.config, '$.genre'), '') <=> NULLIF(JSON_VALUE(ss.value, '$.genre'), ''));

UPDATE outputs o
JOIN station_settings ss ON ss.station_id = o.station_id AND ss.name = 'stream_details'
SET o.details = JSON_SET(COALESCE(o.details, '{}'), '$.url',
                         COALESCE(NULLIF(JSON_VALUE(o.config, '$.url'), ''), ''))
WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL
  AND NOT (NULLIF(JSON_VALUE(o.config, '$.url'), '') <=> NULLIF(JSON_VALUE(ss.value, '$.url'), ''));

UPDATE outputs o
JOIN station_settings ss ON ss.station_id = o.station_id AND ss.name = 'stream_details'
SET o.details = JSON_SET(COALESCE(o.details, '{}'), '$.public',
                         IF(COALESCE(JSON_EXTRACT(o.config, '$.public') = 'true', FALSE),
                            JSON_EXTRACT('true', '$'), JSON_EXTRACT('false', '$')))
WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL
  AND COALESCE(JSON_EXTRACT(o.config, '$.public') = 'true', FALSE)
      <> COALESCE(JSON_EXTRACT(ss.value, '$.public') = 'true', FALSE);

-- Each output on its server.
UPDATE outputs o
JOIN stream_servers s
  ON s.station_id = o.station_id AND s.relay_of IS NULL
 AND s.host = JSON_VALUE(o.config, '$.host')
 AND s.port = COALESCE(CAST(JSON_VALUE(o.config, '$.port') AS UNSIGNED), 8000)
 AND s.source_username <=> COALESCE(NULLIF(JSON_VALUE(o.config, '$.username'), ''), 'source')
 AND s.source_password <=> JSON_VALUE(o.config, '$.password')
 AND s.admin_password <=> NULLIF(JSON_VALUE(o.statistics, '$.admin_password'), '')
 AND s.admin_username <=> IF(NULLIF(JSON_VALUE(o.statistics, '$.admin_password'), '') IS NULL, NULL,
        COALESCE(NULLIF(JSON_VALUE(o.statistics, '$.admin_username'), ''), 'admin'))
SET o.server_id = s.id
WHERE o.kind = 'icecast' AND o.server_id IS NULL AND JSON_VALUE(o.config, '$.host') IS NOT NULL;

-- Only the mount stays on the output.
UPDATE outputs
SET config = JSON_OBJECT('mount', JSON_VALUE(config, '$.mount')), statistics = NULL
WHERE kind = 'icecast' AND server_id IS NOT NULL AND JSON_VALUE(config, '$.host') IS NOT NULL;

-- The samples and sessions so far were read from the output's server.
UPDATE listener_samples s
JOIN outputs o ON o.id = s.output_id
SET s.server_id = o.server_id
WHERE s.server_id = 0 AND o.server_id IS NOT NULL;

UPDATE listener_sessions s
JOIN outputs o ON o.id = s.output_id
SET s.server_id = o.server_id
WHERE s.server_id = 0 AND o.server_id IS NOT NULL;
