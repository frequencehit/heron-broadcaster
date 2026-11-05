-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- AGC presets, shared by the stations. A station's chain itself is its
-- `agc` settings group. See docs/audio-engine.md#master-agc-chain.

CREATE TABLE agc_presets (
    id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    name        VARCHAR(100) NOT NULL,
    -- The blocks of the chain (heron_core::AgcSettings without bypass_all).
    settings    JSON NOT NULL,
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY agc_presets_name (name)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci;

-- Starting points, every block written out. All of them bring the level to
-- the station's loudness target with the gated AGC and end on the final
-- stage at -1 dBFS.
-- - Gentle: an open sound; a light two-band glue, nothing else.
-- - Pop: present and even, a little wider, a firmer low end.
-- - Dense: loud; driven into five tight bands, then a dual-band.
-- - Rock: slower attacks let the drums through; rock mixes are wide
--   already, so no widening.
-- - Talk: voices brought to a steady level, less rumble.
INSERT INTO agc_presets (name, settings) VALUES
    ('Gentle', '{
      "gated_agc": {"enabled": true, "gate_lufs": -45, "max_gain_db": 6},
      "stereo_expander": {"enabled": false, "level": 50, "depth": 50, "threshold_db": -40},
      "bass_eq": {"enabled": false, "shape": "shelf", "frequency_hz": 100, "gain_db": 0},
      "five_band": {"enabled": false, "overdrive_db": 0, "crossovers_hz": [100, 400, 1600, 6400], "links": [false, false, false, false], "bands": [
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0}]},
      "dual_band": {"enabled": true, "crossover_hz": 200, "link": true, "bands": [
          {"mode": "compressor", "threshold_db": -18, "ratio": 2, "attack_ms": 30, "release_ms": 400, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -18, "ratio": 2, "attack_ms": 20, "release_ms": 300, "hold_ms": 0, "gain_db": 0}]},
      "final_stage": {"enabled": true, "mode": "limiter", "ceiling_db": -1, "release_ms": 100}}'),
    ('Pop', '{
      "gated_agc": {"enabled": true, "gate_lufs": -45, "max_gain_db": 9},
      "stereo_expander": {"enabled": true, "level": 40, "depth": 40, "threshold_db": -40},
      "bass_eq": {"enabled": true, "shape": "shelf", "frequency_hz": 80, "gain_db": 2},
      "five_band": {"enabled": true, "overdrive_db": 2, "crossovers_hz": [100, 400, 1600, 6400], "links": [true, false, false, false], "bands": [
          {"mode": "compressor", "threshold_db": -22, "ratio": 3, "attack_ms": 30, "release_ms": 300, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -22, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -22, "ratio": 2.5, "attack_ms": 10, "release_ms": 200, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -24, "ratio": 3, "attack_ms": 5, "release_ms": 150, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -26, "ratio": 3, "attack_ms": 2, "release_ms": 100, "hold_ms": 0, "gain_db": 1}]},
      "dual_band": {"enabled": false, "crossover_hz": 200, "link": false, "bands": [
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0}]},
      "final_stage": {"enabled": true, "mode": "limiter", "ceiling_db": -1, "release_ms": 80}}'),
    ('Dense', '{
      "gated_agc": {"enabled": true, "gate_lufs": -45, "max_gain_db": 12},
      "stereo_expander": {"enabled": true, "level": 60, "depth": 60, "threshold_db": -40},
      "bass_eq": {"enabled": true, "shape": "shelf", "frequency_hz": 70, "gain_db": 3},
      "five_band": {"enabled": true, "overdrive_db": 6, "crossovers_hz": [100, 400, 1600, 6400], "links": [true, true, false, false], "bands": [
          {"mode": "compressor", "threshold_db": -26, "ratio": 5, "attack_ms": 15, "release_ms": 200, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -26, "ratio": 5, "attack_ms": 10, "release_ms": 180, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -26, "ratio": 4, "attack_ms": 5, "release_ms": 150, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -28, "ratio": 4, "attack_ms": 3, "release_ms": 120, "hold_ms": 0, "gain_db": 1},
          {"mode": "compressor", "threshold_db": -30, "ratio": 5, "attack_ms": 1, "release_ms": 80, "hold_ms": 0, "gain_db": 1.5}]},
      "dual_band": {"enabled": true, "crossover_hz": 150, "link": false, "bands": [
          {"mode": "compressor", "threshold_db": -12, "ratio": 8, "attack_ms": 10, "release_ms": 150, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -12, "ratio": 8, "attack_ms": 3, "release_ms": 100, "hold_ms": 0, "gain_db": 0}]},
      "final_stage": {"enabled": true, "mode": "limiter", "ceiling_db": -1, "release_ms": 50}}'),
    ('Rock', '{
      "gated_agc": {"enabled": true, "gate_lufs": -45, "max_gain_db": 9},
      "stereo_expander": {"enabled": false, "level": 50, "depth": 50, "threshold_db": -40},
      "bass_eq": {"enabled": true, "shape": "peak", "frequency_hz": 90, "gain_db": 2},
      "five_band": {"enabled": true, "overdrive_db": 3, "crossovers_hz": [120, 500, 2000, 7000], "links": [true, false, false, false], "bands": [
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 40, "release_ms": 350, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 2.5, "attack_ms": 30, "release_ms": 300, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 2, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -22, "ratio": 2.5, "attack_ms": 10, "release_ms": 200, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -24, "ratio": 3, "attack_ms": 5, "release_ms": 150, "hold_ms": 0, "gain_db": 0}]},
      "dual_band": {"enabled": false, "crossover_hz": 200, "link": false, "bands": [
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0}]},
      "final_stage": {"enabled": true, "mode": "limiter", "ceiling_db": -1, "release_ms": 100}}'),
    ('Talk', '{
      "gated_agc": {"enabled": true, "gate_lufs": -40, "max_gain_db": 12},
      "stereo_expander": {"enabled": false, "level": 50, "depth": 50, "threshold_db": -40},
      "bass_eq": {"enabled": true, "shape": "shelf", "frequency_hz": 100, "gain_db": -3},
      "five_band": {"enabled": false, "overdrive_db": 0, "crossovers_hz": [100, 400, 1600, 6400], "links": [false, false, false, false], "bands": [
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -20, "ratio": 3, "attack_ms": 20, "release_ms": 250, "hold_ms": 0, "gain_db": 0}]},
      "dual_band": {"enabled": true, "crossover_hz": 250, "link": false, "bands": [
          {"mode": "compressor", "threshold_db": -24, "ratio": 3, "attack_ms": 20, "release_ms": 300, "hold_ms": 0, "gain_db": 0},
          {"mode": "compressor", "threshold_db": -22, "ratio": 3, "attack_ms": 5, "release_ms": 150, "hold_ms": 0, "gain_db": 0}]},
      "final_stage": {"enabled": true, "mode": "limiter", "ceiling_db": -1, "release_ms": 100}}');
