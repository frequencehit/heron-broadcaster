-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- M11: the default AAC-LC profile, seeded with the AAC encoder. See
-- docs/outputs.md#default-profiles.
INSERT IGNORE INTO encoder_profiles (name, encoder, bitrate_kbps, sample_rate, channels, options)
VALUES ('AAC 96k', 'aac', 96, 48000, 2, NULL);
