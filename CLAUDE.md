# Important Rules for AI Agents

1. **Ask, don't assume.** If something is unclear, ask before writing a single line. Never make silent assumptions about intent, architecture, or requirements. When running unattended, pick the most reasonable interpretation, proceed, and record the assumption rather than blocking.
2. **Implement the simplest solution for simple problems, better solutions for harder problems.** Do not over-engineer or add flexibility that isn't needed yet.
3. **Don't touch unrelated code**, but do surface bad code or design smells you discover so we can address them as a separate issue.
4. **Flag uncertainty explicitly.** If you're unsure about something, see point 1 above. If it makes sense to do so, conduct a small, localised and low-risk experiment and bring the hypothesis and results to me to discuss. Confidence without certainty causes more damage than admitting a gap.
5. **Suggest better ways.** I'm always open to ideas on better ways to do things. Don't hesitate to suggest a better way, or one that has long-lasting impact over a tactical change.

## Read first

- [docs/README.md](docs/README.md): what Heron is and where everything is documented.
- [docs/workflow.md](docs/workflow.md): how to work on a milestone and what to do when it ends.
- [docs/conventions.md](docs/conventions.md): code, test, commit and release conventions.
- [docs/decisions.md](docs/decisions.md): locked decisions. Ask before going against one.

## Project rules

- **Docs first.** A new feature or milestone starts with its documentation: the roadmap entry and milestone file with acceptance criteria, the design sections in `docs/`, user-facing procedures. Code and tests come after, and the docs commit is the first one of the branch.
- **Docs stay generic.** No specific station, host name, reverse proxy or deployment in `docs/`, the README or PR descriptions. Deployment instructions use Docker's own tools only (`docker compose`, `docker stack deploy`, `docker service`).
- **One worktree per milestone.** Work for another milestone or a side change goes in its own `git worktree`, not in the main checkout.
- **Isolate a worktree's Compose stack.** `docker-compose.yml` sets `name: heron` and tags images `heron-*:${HERON_VERSION:-dev}`, so a stack started from a worktree would take over the main one. Before any build or up, set `COMPOSE_PROJECT_NAME=heron-<milestone>`, `HERON_VERSION=<milestone>` and other host ports in the worktree's `.env`.
- **PR screenshots** go on the `pr-assets` branch, in a folder per change, and are linked from the PR body by commit SHA: `https://raw.githubusercontent.com/dehy/heron-broadcaster/<sha>/<folder>/<file>.png`.
- **Delete with literal paths.** `rm` only full literal paths, never a variable that could be empty.
- **Never share Claude sessions.** This repository is public: no Claude session link (`claude.ai/code/session_…`) and no `Claude-Session:` trailer in commits, pull requests, issues, comments or docs. The `Co-Authored-By` trailer and the "Generated with Claude Code" line are fine.

## Before pushing

Run what CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
scripts/check-spdx.sh

# The AAC-LC encoder (`aac` feature) needs FFmpeg 7.1's development libraries:
# CI runs these in a Debian trixie container (.github/workflows/ci.yml, rust-aac)
cargo clippy -p heron-outputs --features aac --all-targets --locked -- -D warnings
cargo test -p heron-outputs --features aac --locked

cd web
npm run typecheck
npm run coverage
npm run generate-api && git diff --exit-code src/api/schema.ts
```
