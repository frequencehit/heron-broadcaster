# Roadmap

Work one milestone at a time, in order. Do not start a milestone before the previous one meets its acceptance criteria, unless the dependency column allows it. See [Workflow](../workflow.md).

Progress is tracked here: the status column below and the acceptance checklists in each milestone file.

| # | Milestone | Depends on | Status |
|---|---|---|---|
| M0 | [Foundations](m0-foundations.md) | — | Done ([#1](https://github.com/dehy/heron-broadcaster/pull/1)) |
| M1 | [Import & analysis](m1-import-analysis.md) | M0 | Done ([#3](https://github.com/dehy/heron-broadcaster/pull/3)) |
| M1b | [Library management](m1b-library-management.md) | M1 | Done ([#7](https://github.com/dehy/heron-broadcaster/pull/7)) |
| M2 | [Engine core](m2-engine-core.md) | M1 | Done ([#5](https://github.com/dehy/heron-broadcaster/pull/5)) |
| M3 | [24/7 reliability](m3-reliability.md) | M2 | Done ([#16](https://github.com/dehy/heron-broadcaster/pull/16)) |
| M4 | [API & control plane](m4-api-control-plane.md) | M2 | Done ([#8](https://github.com/dehy/heron-broadcaster/pull/8)) |
| M5 | [Web UI](m5-web-ui.md) | M4 | Done ([#9](https://github.com/dehy/heron-broadcaster/pull/9)) |
| M5b | [Track details, labels & tags](m5b-track-details-tags.md) | M5 | Done ([#19](https://github.com/dehy/heron-broadcaster/pull/19)) |
| M5c | [Track management](m5c-track-management.md) | M5 | Done ([#18](https://github.com/dehy/heron-broadcaster/pull/18)) |
| M5d | [Web UI translations](m5d-web-i18n.md) | M5 | Done ([#17](https://github.com/dehy/heron-broadcaster/pull/17)) |
| M5e | [Preparing tracks in the buffer zone](m5e-buffer-preparation.md) | M5b, M5c | Done ([#22](https://github.com/dehy/heron-broadcaster/pull/22)) |
| M6 | [Rotation & clockwheels](m6-rotation-clockwheels.md) | M4 | Done ([#11](https://github.com/dehy/heron-broadcaster/pull/11)) |
| M7 | [Webhooks & output API](m7-webhooks-output-api.md) | M4 | Done |
| M7b | [Listener statistics](m7b-listener-statistics.md) | M4 | Done ([#34](https://github.com/dehy/heron-broadcaster/pull/34)) |
| M8 | [Hot engine upgrades](m8-hot-engine-upgrade.md) | M3 | Done ([#32](https://github.com/dehy/heron-broadcaster/pull/32)) |
| M9 | [Deck EQ](m9-deck-eq.md) | M2 | Done ([#37](https://github.com/dehy/heron-broadcaster/pull/37)) |
| M10 | [AGC & multiband processing](m10-agc-multiband.md) | M9 | Not started |
| M11 | [More encoders, sinks & live DJ](m11-sinks-live-dj.md) | M3 | Not started |
| M12 | [Backups](m12-config-backups.md) | M4, M13, M14 | In progress |
| M13 | [Several stations](m13-several-stations.md) | M6, M7, M7b, M8 | In progress |
| M14 | [Several schedules](m14-schedules.md) | M6, M13 (the schedule per station) | Done ([#102](https://github.com/dehy/heron-broadcaster/pull/102)) |

## Dependency graph

```
M0 ─► M1 ─┬─► M1b
          └─► M2 ─┬─► M3 ─┬─► M8
                  │       └─► M11
                  ├─► M4 ─┬─► M5 ─┬─► M5b
                  │       │       ├─► M5c ─► M5e
                  │       │       └─► M5d
                  │       ├─► M6
                  │       ├─► M7
                  │       ├─► M7b
                  │       ├─► M12
                  │       └─► M13 (also after M6, M7, M7b, M8) ─► M14
                  └─► M9 ─► M10
```
