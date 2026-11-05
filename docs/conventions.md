# Conventions

## Rust

- `rustfmt`, and `clippy` with warnings denied in CI.
- No `unwrap()` / `expect()` outside tests and startup code.
- Errors: `thiserror` in libraries, `anyhow` (or equivalent) only at binary boundaries.
- Structured logging with `tracing`.
- A `/metrics` endpoint on each binary.

## Licensing

- SPDX header in every source file:

  ```rust
  // SPDX-License-Identifier: AGPL-3.0-or-later
  ```

## Web

- TypeScript strict mode.
- API client types generated from the OpenAPI spec.

## Tests

- Unit tests per crate.
- Web: unit tests for `src/lib`, component tests in jsdom ([Web – Development](web.md#development)).
- Generated audio fixtures for analysis and DSP: sine waves, silence, blips, quiet outros.
- Integration tests against MariaDB in Docker.
- Chaos tests for reliability.

## Commits

- [Conventional Commits](https://www.conventionalcommits.org/), scoped by crate or area: `feat(engine): …`, `fix(analysis): …`.
- Small, reviewable commits.

## Releases

- CI publishes the images it has tested to GHCR, under the repository owner's packages (`ghcr.io/dehy/heron-*`; a fork publishes its own, `ghcr.io/<owner>/heron-*`), for amd64 and arm64. Pull requests never publish. `HERON_REGISTRY` in `.env` picks where a station pulls them from.
- Every push to `main` publishes `edge` and `sha-<commit>`. What goes into `edge` is written in the *Edge* sections of `CHANGELOG.md` and of the release notes in [Upgrading](upgrading.md#release-notes); a release renames them to its version.
- To release `X.Y.Z`: set the version in `Cargo.toml` (`[workspace.package]`) and `web/package.json`, update `Cargo.lock`, `web/package-lock.json` and `web/openapi.json` (`cargo run -p heron-api -- openapi > web/openapi.json`), add the release to `CHANGELOG.md` and its upgrade notes to [Upgrading](upgrading.md#release-notes), merge, then tag `main` with `vX.Y.Z` and push the tag. CI checks that the tag matches both versions and publishes `X.Y.Z`, `X.Y` and `latest`.
