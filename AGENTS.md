# Anqa agent notes

Anqa is a Rust dashboard that manages Telegram bots as one Docker container per
bot directory. Project root: `~/dev/anqa`. The binary and image are `anqa`;
the public version is `v0.1.2`.

## Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --locked
ANQA_REQUIRE_DOCKER=1 cargo test --test docker_integration -- --test-threads=1
```

Real-Docker tests are serial. Without `ANQA_REQUIRE_DOCKER=1`, tests that need a
daemon skip; the Docker-unavailable API test still runs. On this host, Docker
commands need an active group session; from a shell without one, use
`sg docker -c '...'`.

Build and check the container deployment:

```sh
docker build -t anqa:dev .
docker compose -f compose.yaml config
docker compose -f compose.dev.yaml config
```

Run the local source build:

```sh
cp .env.example .env
mkdir -p dev-bots dev-data
docker compose -f compose.dev.yaml up -d --build
cat dev-data/token
```

## Module map

```text
src/
  main.rs              process entry; healthcheck dispatch; server startup
  lib.rs               module wiring; public API for tests
  config.rs            env parsing, paths, bind address
  auth.rs              token load/generate, constant-time compare, cookies
  state.rs             shared AppState and per-bot operation state
  error.rs             typed errors and JSON error responses
  api.rs               axum router, middleware, handlers
  assets.rs            rust-embed asset serving
  healthcheck.rs       `anqa healthcheck`
  bots/
    mod.rs             re-exports and BotView/status types
    manifest.rs        strict bot.toml, env, ports, volumes, config hash
    slug.rs            directory -> Docker-safe slug
    discovery.rs       top-level scan, errors, duplicate-slug guard
  docker/
    mod.rs             DockerClient wrapper, errors, list/inspect
    status.rs          Docker state/health/uptime -> bot status
    context.rs         .dockerignore-aware tar context
    build.rs           BuildKit image build, pull-when-missing
    lifecycle.rs       ensure/create/start/stop/autostart
    cleanup.rs         sentinel + orphan cleanup
assets/
  index.html           SPA shell
  app.css              tokens, themes, layout, components
  app.js               polling, rendering, actions, settings
  theme.js             pre-paint theme/accent selection
  fonts/JetBrainsMono-Variable.ttf
  fonts/OFL.txt
examples/echo-bot/     stdlib Python echo bot
tests/
  fixtures/            sleeper, volume, and broken build fixtures
  common/mod.rs        test environment and Docker helpers
  docker_integration.rs
  build_spike.rs
Dockerfile
compose.yaml
compose.dev.yaml
.env.example
.dockerignore
```

## Design

- Palette: skill `design` — two accents: Telegram blue (default) and Rust orange; departure from the six-accent default per user request
- Themes: Light / Dark / OLED; first run follows prefers-color-scheme; OLED opt-in
- Type: JetBrains Mono (bundled, OFL)
- Approved: 2026-11-15

## Conventions

- Rust 2024 edition, pinned Rust 1.98.1. Work directly on `main`; this is the
  approved branch for the project.
- Keep the Rust dependencies pinned in `Cargo.lock`. Do not add a frontend
  framework, build step, or runtime dependency for the SPA.
- The UI uses semantic CSS tokens from `assets/app.css`. Components must not
  use raw palette hex values. Themes are Light (default follow), Dark, and
  OLED; accents are Telegram blue and Rust orange.
- `assets/app.js` must remain DOM/fetch based: no `innerHTML`, no eval, no
  framework. Poll every 2 seconds, pause while hidden, and keep session
  generation checks around async work.
- All mutations return `202 Accepted` and run in the background. Per-bot
  operation state is in memory; `GET /api/bots` merges it into `BotView`. Error
  responses use `{"error":{"code":"...","message":"..."}}`.
- Docker owns container state, restart policies, images, health, and named
  volumes. The filesystem owns bot source and manifests. Anqa only persists
  the token and sentinel under `/data`.
- Managed resources are named `anqa-<slug>` (containers), `anqa/<slug>:latest`
  (Dockerfile images), and `anqa-<slug>-<name>` (volumes). Containers are
  labelled `anqa.managed=true`, `anqa.bot=<dir>`, `anqa.config-hash=<hash>`,
  and `anqa.version=<version>`.
- Never delete named volumes as part of lifecycle or orphan cleanup. Bot data
  survives stop, rebuild, recreation, and directory removal.
- Orphan cleanup writes `/data/.anqa-volume` with the bots root `dev:ino` and
  skips the first run with a new root. Do not weaken this guard or write to
  `/bots`.
- `/bots` is read-only inside the container. Bot builds ignore `.env`, `.git`,
  and `.dockerignore` in the build context. Never bake secrets into images.
- API and UI tests use `tests/common/mod.rs`; real-Docker tests must clean up
  containers, images, and volumes they create. The orphan-cleanup test must not
  delete unrelated managed containers; it skips when any already exist.
- No telemetry, analytics, log viewer, bot CRUD, framework detection, or
  Telegram API probes in v1.

## Deployment

Production stack: `/srv/stacks/anqa`.

```text
/srv/stacks/anqa/
  compose.yaml
  .env
  bots/
  data/
    token
```

- Dashboard URL: port **7409** (`0.0.0.0:7409`; Compose maps
  `${ANQA_PORT:-7409}`).
- `anqa: 7409` belongs under `# Personal Projects (7400s)` in
  `/srv/stacks/dashboard/html/config.yaml`.
- The production service uses the pinned GHCR image, runs as
  `${ANQA_UID}:${ANQA_GID}`, adds `${DOCKER_GID}` for the socket, mounts
  `./bots:/bots:ro` and `./data:/data`, and uses `read_only: true`.
- This host has UID/GID `1000:1000` and Docker socket GID `988`; verify with
  `id` and `stat -c '%g' /var/run/docker.sock` before deploying.
- First login token: `cat data/token`. Rotate by deleting the file and
  restarting, or by setting `ANQA_TOKEN`.
- Updates: change the image tag, then `docker compose pull && docker compose
  up -d`. Rollback: pin the previous tag and run the same command. Named bot
  volumes are untouched.

## Docker socket warning

Anqa has full Docker API access, which is root-equivalent on the host. The
`/var/run/docker.sock:ro` mount only protects the socket file; Docker API calls
through it can still create, start, stop, and remove containers. Run Anqa on a
trusted network behind TLS, set `ANQA_SECURE_COOKIE=1` when HTTPS is in front,
and only expose bot directories you trust. Bot images consume host resources
according to their own configuration; Anqa does not add resource limits.

## Screenshot

`docs/screenshot.png` is a real capture of the dashboard in the Light theme,
using sample data: Demo Bot running, Echo Bot and Price Alerts stopped.
It was captured at 1440x320 after logging in and starting the sleeper demo bot.
There is no committed screenshot script. To reproduce the setup, create demo
directories under `dev-bots/`, run `docker compose -f compose.dev.yaml up -d
--build`, log in with `dev-data/token`, start the demo, and capture the
dashboard. Remove `dev-bots/`, `dev-data/`, the demo container, and the demo
image afterward. The published image is not used for this capture.

## State file

This project uses `.ai-state` at the root to record where work stopped. Read it
when starting, rewrite it as one plain line before finishing, and keep it
git-ignored (it already is).
