-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Daypart separation (docs/rotation.md#daypart-separation) is relaxed at
-- steps 3 and 4, between album and artist separation: the steps after album
-- separation moved two places (artist 5 and 6, title 7 and 8, track 9 and
-- 10, the fallback category 11 and 12). The picks already logged follow, so
-- the log reads the same. The settings and the categories' overrides are
-- JSON: nothing to add there.

-- `explanation` first: it reads `step` before it changes.
UPDATE rotation_picks
SET explanation = JSON_SET(explanation, '$.step', step + 2),
    step = step + 2
WHERE step >= 3;

UPDATE play_history
SET relaxation_applied = JSON_SET(
        relaxation_applied, '$.step',
        CAST(JSON_VALUE(relaxation_applied, '$.step') AS UNSIGNED) + 2)
WHERE relaxation_applied IS NOT NULL
  AND CAST(JSON_VALUE(relaxation_applied, '$.step') AS UNSIGNED) >= 3;
