# Anqa — Telegram Bot Dashboard — Design Spec

**Status:** Approved
**Date:** 2026-11-15
**Repo:** `~/dev/anqa`
**Remote:** `git@github.com:benyamin-git/Anqa.git` (empty repo to be created by the user)
**First release:** `v0.1.0`
**License:** MIT

## Objective / why

Anqa is a small, self-hosted Docker app that manages Telegram bots as **one container per bot**. Each bot lives in its own directory inside a volume; dropping in a `Dockerfile` (plus an optional `bot.toml` and `.env`) is enough for the dashboard to list it, show whether it is live, start and stop it, and toggle whether it starts on boot.

The project exists because running a handful of bots should not require per-framework glue, a general-purpose container UI, or keeping a supervisor in sync with each bot's language and dependencies. Docker already knows how to build, run, isolate, and restart containers. Anqa gives that machinery a focused Telegram-bot control surface with no framework detection and no code generation.

Anqa is a public project for self-hosters. It is not a bot framework, a deployment platform, or a Compose wrapper.

## Success criteria / definition of done

1. `docker compose up -d` starts Anqa on port 7409; a first-run token is generated and stored in `data/token`; the browser login accepts it.
2. A bot directory containing a `Dockerfile` and `.env` appears in the dashboard without a restart; **Start** builds the image and starts exactly one container; the card shows **Live**; **Stop** stops it.
3. A bot with autostart on is started by Docker after a host/daemon restart even if the dashboard was down; a bot with autostart off is not started.
4. Bot data stored in declared named volumes survives stop, start, image rebuild, and container recreation.
5. Editing bot files and pressing **Start** on a stopped bot applies the new code; a build failure is shown on the card and does not crash the dashboard.
6. A deleted bot directory causes its managed container to be removed while its named volumes remain; a missing or empty bots volume never wipes every bot.
7. The UI ships Light, Dark, and OLED themes with Telegram-blue and Rust-orange accents, follows the OS theme on first run, persists choices, and meets the accessibility floor on desktop and mobile.
8. `cargo fmt --check`, `cargo clippy -- -D warnings`, unit tests, and Docker integration tests pass in CI; a tagged release publishes a multi-arch `linux/amd64` + `linux/arm64` image to GHCR.
9. A new self-hoster can follow the README from an empty directory to a running dashboard and a sample bot without reading source code.
10. The public repo contains the approved spec, the approved plan, README, AGENTS.md, LICENSE, and the JetBrains Mono OFL license.

## Scope

### In scope

- A Rust single-binary dashboard served over HTTP.
- Docker Engine API over `/var/run/docker.sock`.
- Directory discovery in the mounted bots volume.
- Dockerfile builds with an optional `bot.toml` run manifest.
- One container per bot, named and labelled by Anqa.
- Start, stop, status, health, uptime, and autostart controls.
- Docker restart-policy-backed autostart.
- Generated-token authentication with an HttpOnly cookie.
- Named Docker volumes for bot data, with Anqa prefixes.
- Optional port publishing and command override via the manifest.
- Orphaned-container cleanup with a safety guard.
- Embedded static SPA (HTML/CSS/JS) and JSON API.
- Responsive card-grid UI with terminal-inspired house styling.
- Light, Dark, and OLED themes; Telegram-blue and Rust-orange accents.
- A Python stdlib Telegram echo-bot example and a sleeper fixture for tests.
- `compose.yaml` for the published image and `compose.dev.yaml` for local builds.
- GitHub Actions CI, multi-arch GHCR publishing, Dependabot.
- Public documentation, MIT license, deployment notes, and security warnings.

### Non-goals (explicitly out)

- A log viewer, log persistence, or log streaming in the UI.
- Creating, editing, renaming, or deleting bot directories from the UI.
- Migrating the existing `drbm-bot` or `dailypoker` systemd services.
- Controlling host systemd units or host processes.
- A Docker Compose project per bot, or parsing Compose files.
- Framework detection, language detection, dependency installation, or code generation.
- Telegram `getMe` liveness probes or any Telegram API call from the dashboard.
- Multiple users, roles, accounts, or per-user permissions.
- A socket-proxy sidecar or a host agent.
- Docker networks, CPU/memory limits, user overrides, labels, or healthcheck fields in the manifest.
- Host bind mounts for bot data; named volumes only.
- Private-registry credential management beyond the Docker daemon's own defaults.
- Windows or macOS native builds; the deliverable is a Linux container image.
- ARMv7 or other architectures beyond `linux/amd64` and `linux/arm64`.
- Internationalisation; the UI is English.
- Telemetry, analytics, update checks, audit logs, or metrics.
- A restart button; use Stop then Start.
- Automatic updates of prebuilt bot images.
- A separate API version (`/api/v1`); the internal API is unversioned.

## Decision log

### Core decisions

| # | Decision | Rationale | Status | Source |
| --- | --- | --- | --- | --- |
| D1 | Build Anqa as one public, self-hosted project: dashboard plus bot supervisor. | The requested scope is one coherent product, not several independent systems. | decided | user |
| D2 | v1 consists of listing bot directories, status, start/stop, and the autostart toggle. No logs or bot CRUD. | Keeps the first release small and honest. | decided | user |
| D3 | Run one Docker container per bot. | Isolation, framework independence, and Docker-native lifecycle and boot persistence. | decided | user |
| D4 | A bot directory is defined by a `Dockerfile` and an optional `bot.toml`; no Compose and no framework detection. | Anqa delegates runtime details to the image; the manifest only describes run configuration. | decided | user |
| D5 | Rebuild a Dockerfile bot on every Start; rely on Docker layer caching. | Code changes apply automatically and the behaviour is simple to reason about. | decided | user |
| D6 | The optional manifest supports a display name, a prebuilt image or a Dockerfile, an env file, an argv command, published ports, and named volumes. | The smallest set that makes real Telegram bots usable without becoming a mini-Compose. | decided | user |
| D7 | Autostart is a Docker restart policy: `always` when on, `no` when off. No systemd integration. | Docker already provides boot persistence and works for every self-hoster. | decided | user |
| D8 | Toggling autostart on a never-started bot builds if needed and creates the container without starting it. | The toggle is honest immediately; Docker stays the source of truth. | decided | user |
| D9 | If a bot directory disappears, stop and remove its managed container and keep its named volumes. | Prevents invisible orphan bots while preserving data. | decided | user |
| D10 | Bot data lives in named Docker volumes, never in host binds. | Portable, no host-path configuration, and compatible with a read-only bots volume. | decided | user |
| D11 | Connect directly to `/var/run/docker.sock`. | Simplest reliable path for builds and container lifecycle; documented as root-equivalent access. | decided | user |
| D12 | Generate a first-run token, store it in `/data`, and carry it in an HttpOnly, SameSite=Strict cookie. | Safe default with no compose secret setup; no JS-readable token. | decided | user |
| D13 | Run the dashboard as the host UID/GID and add the Docker socket GID via `group_add`. | Non-root, can read the user's bot files and write `./data`, and still reach the socket. | decided | user |
| D14 | Status means Docker container state plus Docker health when the bot image defines it. No Telegram probes. | Framework-agnostic and honest about what Docker can know. | decided | user |
| D15 | Target general self-hosters; start greenfield and do not migrate existing systemd bots. | Keeps the public contract generic and the first release unblocked by production cutovers. | decided | user |
| D16 | Implement the dashboard in Rust with axum, tokio, bollard, and rust-embed. | Small static binary, direct Engine API, no frontend runtime or build step. | decided | user |
| D17 | Serve an embedded static SPA that polls a JSON API every 2 seconds. | Minimal moving parts, testable API, no template engine or frontend framework. | decided | user |
| D18 | Use a terminal-inspired design with Light/Dark/OLED themes, JetBrains Mono, and two accents: Telegram blue (default) and Rust orange. | The requested identity, with a deliberate two-accent departure from the house six-accent default. | decided | user |
| D19 | Name the project **Anqa**; repo `Anqa`; crate, binary, and image `anqa`. | Anqa is the Persian mythical bird: a guardian presence without a literal Telegram or bot label. | decided | user |
| D20 | License MIT; first release `v0.1.0`. | Matches the user's other projects and honestly signals pre-1.0. | decided | user |
| D21 | Project lives at `~/dev/anqa`; remote is `benyamin-git/Anqa`; the user creates the empty GitHub repo. | Standard location for the user's projects; `gh` is not installed. | decided | user |
| D22 | Run on port 7409 and deploy from `/srv/stacks/anqa`; register `anqa: 7409` in the dashboard config. | The next free 7400s port and the host's established stack layout. | decided | user |
| D23 | Publish a GHCR image and provide `compose.yaml` for self-hosters plus `compose.dev.yaml` for local builds. | Clear install path; no accidental local builds in the public quickstart. | decided | user |
| D24 | Publish multi-arch `linux/amd64` + `linux/arm64` images with GitHub Actions CI. | Covers common self-hosted servers and gives the public project a repeatable release. | decided | user |
| D25 | Test with Rust unit tests, real-Docker integration tests, and a manual browser smoke test. | Proves the core lifecycle without adding a browser toolchain to CI. | decided | user |
| D26 | Include a Python stdlib long-polling echo bot example and a tiny Alpine sleeper fixture. | A real, dependency-light example that proves framework independence, plus a deterministic test bot. | decided | user |
| D27 | Write and commit this spec and the implementation plan in the repo. | The user asked for the explicit planning flow and a visible record. | decided | user |
| D28 | The user adds their account to the `docker` group and re-logs in before implementation. | Local Docker and integration tests need daemon access; the agent cannot run the sudo command. | decided | user |
| D29 | Do not manage host systemd services; Docker restart policies are the only boot mechanism. | Portable and consistent with the container-per-bot model. | decided | user |
| D30 | Do not add a socket proxy, host agent, or Compose-per-bot mode in v1. | Keeps the architecture small; each adds a second component or a second lifecycle model. | decided | user |

### Assumed implementation details

These are recommendations made because the user did not express a preference. They are flagged here for review, not silently chosen.

| # | Decision | Rationale | Status | Source |
| --- | --- | --- | --- | --- |
| A1 | Display name `Anqa`; repo `Anqa`; crate/binary/image `anqa`; image `ghcr.io/benyamin-git/anqa`. | Consistent with the user's capitalised repo names and Docker's lowercase image rules. | assumed | recommendation |
| A2 | Token is 32 random bytes, base64url-encoded, stored at `/data/token` with mode 0600, printed only when generated. `ANQA_TOKEN` overrides generation; deleting the file and restarting rotates it. | High entropy, visible on the host, no secret in logs after first run, and an automation escape hatch. | assumed | recommendation |
| A3 | Cookie `anqa_session`: HttpOnly, SameSite=Strict, Path=/, 30-day Max-Age, Secure only when `ANQA_SECURE_COOKIE=1`. Bearer tokens are also accepted for tests and API clients. | Simple sessionless auth; SameSite blocks cross-site CSRF; HTTPS remains a reverse-proxy concern. | assumed | recommendation |
| A4 | API routes are unversioned under `/api`; `/`, static assets, `/api/login`, and `/healthz` are public. All other API routes require auth. | Keeps the internal API small and the login page usable before auth. | assumed | recommendation |
| A5 | Poll `/api/bots` every 2 seconds; pause while the tab is hidden; refresh on visibility and via a manual refresh control. | Fresh status without a heavyweight SSE/WebSocket layer. | assumed | recommendation |
| A6 | Manifest schema is strict: unknown fields are errors; Dockerfile and `image` are mutually exclusive; `command` is an argv array; volumes are named only; `env_file` defaults to `.env` when present. | Surfaces typos and avoids ambiguous or unsafe runtime configuration. | assumed | recommendation |
| A7 | Build contexts respect `.dockerignore`; `.env` and `.git` are excluded from the build context by default; Dockerfile builds tag `anqa/<slug>:latest`. | Prevents accidental secret baking and keeps contexts small. | assumed | recommendation |
| A8 | Containers are named `anqa-<slug>` and labelled `anqa.managed=true`, `anqa.bot=<dir>`, `anqa.config-hash=<hash>`, `anqa.version=<version>`. Config changes recreate the container and preserve named volumes. | A stable mapping from directory to Docker resources and a clean way to detect stale containers. | assumed | recommendation |
| A9 | Prebuilt images are pulled only when missing; existing local images are used as-is. | Offline-friendly and predictable; users update a pinned tag deliberately. | assumed | recommendation |
| A10 | Named volumes are prefixed `anqa-<slug>-<name>` and labelled with the bot directory. | Prevents collisions between bots and avoids shared accidental state. | assumed | recommendation |
| A11 | The bots directory is mounted read-only; `/data` is writable. | The dashboard only reads source and secrets; it writes only the token and app state. | assumed | recommendation |
| A12 | The dashboard's own compose service uses `restart: unless-stopped`, `init: true`, `security_opt: no-new-privileges:true`, a read-only root filesystem, and a writable `/data` mount. | Standard container hardening without affecting bot containers. | assumed | recommendation |
| A13 | The published image is a distroless static image with a musl-built binary and a `healthcheck` subcommand used by Docker HEALTHCHECK. | Small attack surface and a tiny image with a working health check. | assumed | recommendation |
| A14 | Orphan cleanup runs only when the bots root is readable and the sentinel at `/data/.anqa-volume` matches the bots root's Unix `dev:ino`; the app writes the sentinel on first sight or a changed root and skips cleanup that run. | Prevents a missing or freshly mounted volume from removing every bot; `/bots` stays read-only. | assumed | recommendation |
| A15 | If Docker is unreachable, the app still serves the login page and returns a clear 503 with an error banner; it reconnects on later requests. | The dashboard should explain the failure instead of failing to start. | assumed | recommendation |
| A16 | Operations are serialised per bot; a second action returns 409 while one is in progress. The UI disables the relevant controls. | Prevents build/start/stop races without a global lock. | assumed | recommendation |
| A17 | Discovery is top-level only; hidden entries and symlinks are ignored; a directory qualifies with a `Dockerfile` or `bot.toml`; sorted by directory name. | Predictable scanning and no path-escape surprises. | assumed | recommendation |
| A18 | Cards show display name, directory, status chip, health when present, uptime, Start/Stop, autostart switch, and the last operation error. | Gives the user the status and controls they asked for without a detail page. | assumed | recommendation |
| A19 | UI details: 64px top bar, responsive card grid, 40px desktop controls, 48px touch targets, 12px card radius, settings dialog for theme and accent, centred token login, empty and error states, semantic HTML and visible focus. | Applies the house design system to a card-grid dashboard. | assumed | recommendation |
| A20 | Public docs include README, AGENTS.md, LICENSE, font OFL license, CI badges, a screenshot, security warnings, deployment, backup, and API reference. | A public self-hosted tool needs more than source code to be usable and safe. | assumed | recommendation |
| A21 | The sample echo bot reads `TELEGRAM_BOT_TOKEN` from its `.env`; `.env.example` ships with the example. | The conventional BotFather token variable and a safe template. | assumed | recommendation |
| A22 | No i18n, telemetry, update checker, audit log, metrics, or user accounts. | Keeps v1 minimal and avoids data collection. | assumed | recommendation |
| A23 | Bot containers use Docker's `json-file` log driver with `max-size=10m` and `max-file=3` by default. | Prevents a chatty bot from filling the host disk; logs remain available via `docker logs`. | assumed | recommendation |
| A24 | Stop uses Docker's default 10-second grace period; there is no restart button; a running bot's Start control is disabled. | Predictable shutdown and a three-action surface. | assumed | recommendation |
| A25 | Private registry support is limited to whatever the Docker daemon can already pull; no credential UI or Docker config mount is added. | Avoids a secrets-management subsystem in v1. | assumed | recommendation |

## Architecture

### Product surface

One page after login, plus a token login screen and a settings dialog.

- **Login:** token field, submit, invalid-token error. No username.
- **Top bar:** wordmark `ANQA`, a live/total summary, refresh, settings, logout.
- **Main:** responsive grid of bot cards. Empty state explains the directory contract. A banner reports Docker or bots-volume failures.
- **Card:** display name, directory, status chip, health badge when available, uptime, Start/Stop, autostart switch, last error.
- **Settings:** theme (Light/Dark/OLED) and accent (Telegram blue/Rust orange), persisted in `localStorage`.
- **No detail page, no log viewer, no bot CRUD.**

### Components

1. **`anqa` Rust binary**
   - Configuration and environment parsing.
   - Token bootstrap and auth middleware.
   - Docker Engine client (bollard).
   - Bot directory scanner and `bot.toml` parser.
   - Build, container-create, start, stop, autostart, and orphan-cleanup operations.
   - In-memory per-bot operation state and last error.
   - Axum HTTP server with embedded static assets.
2. **Docker daemon**
   - Source of truth for images, containers, restart policies, health, and named volumes.
   - Accessed over the mounted Unix socket.
3. **Bots volume (`/bots`, read-only)**
   - One immediate child directory per bot.
   - Contains source, `Dockerfile`, optional `bot.toml`, optional `.env`.
4. **Data volume (`/data`, read-write)**
   - `/data/token` and `/data/.anqa-volume` are the persistent Anqa files.
5. **Browser**
   - Static SPA; polls the JSON API and renders cards.

### Data flow

1. **Startup:** read env; ensure `/data` is writable; load or generate the token; connect to Docker; ensure `/bots` is readable; create the `/data/.anqa-volume` sentinel if needed; run guarded orphan cleanup; start the HTTP server.
2. **`GET /api/bots`:** authenticate; scan bot directories; list Anqa-managed containers; inspect them; merge directory data, Docker state, health, uptime, autostart, and in-memory operation state; return JSON.
3. **Start:** authenticate; resolve the bot; take its per-bot lock; build the image if the bot uses a Dockerfile; ensure or recreate the container; start it; release the lock; return the operation result.
4. **Stop:** authenticate; take the lock; stop the container with a 10-second grace period; release; return.
5. **Autostart:** authenticate; take the lock; ensure the image and container exist without starting; update the restart policy to `always` or `no`; release.
6. **Orphan cleanup:** find `anqa.managed=true` containers whose `anqa.bot` directory no longer exists; stop and remove the container; keep named volumes; log the action.

### Bot directory contract

```
bots/
  echo-bot/
    Dockerfile
    bot.toml          # optional
    .env              # optional, runtime environment
    .dockerignore     # recommended
    bot.py
```

A directory is a bot when it contains a `Dockerfile` or a `bot.toml`. A directory with a `Dockerfile` and no manifest uses defaults. A directory with a `bot.toml` must resolve to either a Dockerfile or an `image`. A directory with neither source is ignored. A directory with an invalid manifest appears as an error card rather than crashing the scan.

The directory name is the stable identity used in API paths, container labels, and volume prefixes. The display name is cosmetic; duplicate display names are allowed.

### Manifest (`bot.toml`)

```toml
name = "Echo Bot"                 # optional, defaults to the directory name
# image = "ghcr.io/you/echo-bot:0.1.0"  # use instead of a Dockerfile
env_file = ".env"                 # optional; defaults to .env when it exists
command = ["python", "-u", "bot.py"]     # optional argv override
ports = ["8080:8080"]             # optional host:container[/protocol]
volumes = ["data:/app/data"]      # optional named volumes only
```

Rules:

- `image` and a `Dockerfile` are mutually exclusive.
- `command` is an argv array, never a shell string. Use `["sh", "-c", "..."]` if shell behaviour is needed.
- `env_file` is relative to the bot directory. A missing explicitly configured file is an error. The default `.env` is optional.
- `ports` entries are validated and passed to Docker. Host IPs may be included.
- `volumes` entries are `name:path` named volumes. The Docker volume becomes `anqa-<slug>-<name>`. Host binds are rejected.
- Unknown fields are errors. There is no `autostart`, `healthcheck`, `restart`, `network`, `user`, `limits`, or `labels` field.
- Autostart is read from and written to Docker, never from the manifest.

### Docker resources

- **Image:** Dockerfile bots build `anqa/<slug>:latest`. Prebuilt-image bots use the declared image.
- **Container:** `anqa-<slug>` with labels `anqa.managed=true`, `anqa.bot=<dir>`, `anqa.config-hash=<hash>`, `anqa.version=<version>`.
- **Restart policy:** `always` when autostart is on; `no` otherwise.
- **Logging:** `json-file`, `max-size=10m`, `max-file=3`.
- **Volumes:** `anqa-<slug>-<name>` only. Named volumes are never removed by Anqa.
- **Ports:** published only when declared in the manifest.
- **Networks:** Docker default bridge; no custom network field.
- **Stop:** Docker stop with the default 10-second grace period.

### Lifecycle semantics

- **Start (stopped):** rebuild the image when a Dockerfile is present; pull a prebuilt image only when missing; recreate the container if the image ID or config hash changed; start it.
- **Start (running):** disabled; there is no restart action.
- **Stop (running):** stop the container, keep it. With autostart on, `always` means a later Docker daemon start will start it again; with autostart off, it stays stopped.
- **Autostart on:** ensure the image/container exists (create without starting if needed), then set the restart policy to `always`. Does not start a stopped bot by itself.
- **Autostart off:** set the restart policy to `no`. Does not stop a running bot.
- **Config change:** the next Start detects the changed hash and recreates the container; named volumes persist.
- **Orphan:** removing the directory triggers guarded cleanup; the container is removed and named volumes stay.
- **Dashboard down:** bot containers keep running; Docker restart policies apply independently.
- **Dashboard restart:** reconnects to Docker; no state is lost except in-memory operation errors.

### HTTP API

All responses are JSON except static assets. Errors use `{"error": {"code": "...", "message": "..."}}`.

| Method | Path | Auth | Purpose |
| --- | --- | --- | --- |
| `GET` | `/` | public | SPA shell |
| `GET` | `/assets/*` | public | Embedded CSS, JS, font |
| `GET` | `/healthz` | public | Process liveness |
| `POST` | `/api/login` | public | Exchange token for cookie |
| `POST` | `/api/logout` | auth | Clear cookie |
| `GET` | `/api/bots` | auth | List bots and statuses |
| `POST` | `/api/bots/{dir}/start` | auth | Build/ensure and start |
| `POST` | `/api/bots/{dir}/stop` | auth | Stop |
| `PUT` | `/api/bots/{dir}/autostart` | auth | Set restart policy; body `{"enabled": true}` |

Bot object shape (fields may be added, not removed, within v1):

```json
{
  "dir": "echo-bot",
  "name": "Echo Bot",
  "status": "running",
  "health": "healthy",
  "uptime_seconds": 812,
  "autostart": true,
  "busy": false,
  "error": null,
  "image": "anqa/echo-bot:latest",
  "ports": ["8080:8080"],
  "volumes": ["data:/app/data"]
}
```

Status values: `running`, `stopped`, `starting`, `restarting`, `unhealthy`, `error`, `building`, `stopping`. `health` is `healthy`, `unhealthy`, `starting`, or `null`. `busy` is true while an operation holds the per-bot lock. Long operations return 202 and are observed through polling. A conflicting operation returns 409; an unknown directory returns 404; Docker or build failures return 500 with a message.

### Auth and security

- The dashboard has root-equivalent Docker control. Authentication is mandatory.
- On first run: use `ANQA_TOKEN` if set; otherwise read `/data/token`; otherwise generate 32 random bytes, base64url-encode, write mode 0600, and log the token once.
- Login compares the supplied token in constant time and sets `anqa_session` as an HttpOnly, SameSite=Strict cookie. `ANQA_SECURE_COOKIE=1` adds Secure for HTTPS reverse proxies.
- Bearer tokens are accepted for API clients and tests.
- `/`, assets, `/api/login`, and `/healthz` are public; every other API route is protected.
- CSRF is blocked by SameSite=Strict plus a JSON `Content-Type` requirement on mutations.
- Security headers: CSP with no inline scripts, no third-party origins, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, `Permissions-Policy` minimal.
- The README opens with a prominent warning: mounting the Docker socket is equivalent to root on the host; run Anqa only on a trusted host, keep the token secret, and use a TLS reverse proxy for remote access.
- No secrets are sent to the browser; `.env` values are passed only to bot containers and are never included in API responses.
- `.env` and `.git` are excluded from build contexts by default to prevent accidental image baking.

### Persistence and state

- Docker is the source of truth for images, containers, restart policies, health, and named volumes.
- `/data/token` and `/data/.anqa-volume` are the only persistent Anqa files.
- No database, no JSON state file, no session store.
- In-memory state is limited to per-bot operation progress and the last error; it is lost on dashboard restart, which is acceptable.
- Named volumes survive dashboard restarts, bot container recreation, and bot directory removal.
- Renaming a bot directory creates new resources and volume names; the old volumes remain and are documented as manual cleanup.

### UI / design note

This is the required design note for an undefined project before UI code. It follows the `design` skill and its house style, with one explicit departure: two accents instead of the six-accent default.

- **Identity:** terminal-inspired, restrained, monospace, developer-tool calm. No mascot, illustration, or decorative art. The wordmark is simply `ANQA`.
- **Themes:** Light, Dark, OLED. First run follows `prefers-color-scheme` (light → Light, dark → Dark); OLED is opt-in. Theme and accent are stored in `localStorage` and applied before first paint. `color-scheme` matches.
- **Accents:** Telegram blue (default) and Rust orange, both derived through the house OKLCH role system; raw brand hex is not used in components. One accent is active at a time; status colors are separate and fixed. This is the approved departure from the six-accent default.
- **Palette:** semantic roles only, not raw hex in components: primary family, secondary, tertiary, surface containers, outline, background/on-surface, and fixed profit/loss/warning status colors. Theme neutrals follow the house Light/Dark/OLED ladders; OLED uses true black.
- **Type:** JetBrains Mono, bundled OFL variable font, one family for chrome and data. Closed scale: 12, 14, 16, 20, 24, 32px. Body 14–16px; no UI text below 12px.
- **Density:** 64px top bar; 40px desktop controls; 48px touch targets; 4px spacing steps; 6px chips/inputs, 8px buttons, 12px cards, 16px dialogs; full pill for the switch.
- **Layout:** centred content with a responsive card grid, `minmax(320px, 1fr)`, one column on narrow screens. Cards use `surface-container` steps and outlines, never drop shadows. Long lists scroll with the page; the top bar stays put.
- **Card anatomy:** display name (20px/600), directory (12px muted), status chip with text plus color, health badge when present, uptime, Start (filled primary) or Stop (tonal), autostart switch with a visible label, and a last-error line.
- **Login:** centred card, wordmark, token input with a visible label, primary submit, invalid-token message, and a hint to read `data/token` or the container logs.
- **Settings:** dialog with Theme and Accent choices, immediate preview, and persistence. No account settings.
- **Empty/error states:** explain the bots directory contract; Docker-unavailable banner with retry; per-card build/start errors; loading skeleton cards.
- **Accessibility floor:** 4.5:1 body contrast in all themes; visible focus rings; semantic landmarks, buttons, labels, and lists; status never conveyed by color alone; keyboard operable in a sensible order; `aria-live="polite"` for status/error updates; `prefers-reduced-motion` respected.
- **Bans:** no gradients, glassmorphism, emoji, stock art, hero marketing layout, purple AI-SaaS look, soft card shadows, competing accents, raw hex in components, physical left/right CSS, decorative numbering, or multiple font families.
- **Responsive:** desktop, tablet, and phone are all first-class; the card order does not change.

The design record will be written into `AGENTS.md` after spec approval:

```markdown
## Design

- Palette: skill `design` — two accents: Telegram blue (default) and Rust orange; departure from the six-accent default per user request
- Themes: Light / Dark / OLED; first run follows prefers-color-scheme; OLED opt-in
- Type: JetBrains Mono (bundled, OFL)
- Approved: 2026-11-15
```

### Deployment

**Repository (`~/dev/anqa`)**

```
Cargo.toml
Cargo.lock
src/
assets/
examples/echo-bot/
tests/
Dockerfile
compose.yaml
compose.dev.yaml
README.md
AGENTS.md
LICENSE
.dockerignore
.gitignore
docs/superpowers/specs/
docs/superpowers/plans/
.github/workflows/
```

**Host deployment (`/srv/stacks/anqa`)**

```
compose.yaml
.env
data/
  token
bots/
  echo-bot/
```

- Dashboard URL: `http://192.168.1.100:7409`.
- `anqa: 7409` is added under `Personal Projects` in `/srv/stacks/dashboard/html/config.yaml`.
- Compose maps `7409:7409`, mounts `./bots:/bots:ro`, `./data:/data`, and `/var/run/docker.sock:/var/run/docker.sock`.
- Compose sets `user: "${ANQA_UID:-1000}:${ANQA_GID:-1000}"` and `group_add: ["${DOCKER_GID:-988}"]`; this host's Docker GID is 988.
- The dashboard service uses `restart: unless-stopped`, `init: true`, `read_only: true`, `security_opt: ["no-new-privileges:true"]`, and a writable `/data` mount.
- `compose.yaml` uses the published GHCR image; `compose.dev.yaml` builds the local source.
- Setup: `mkdir -p data bots`, create `.env` with `ANQA_UID`, `ANQA_GID`, `DOCKER_GID`, and optional `ANQA_PORT`, then `docker compose up -d`.
- First login: read `data/token` or the one-time container log line.
- Update: change the pinned image tag or pull a new tag, then `docker compose up -d`.
- Rollback: pin the previous image tag and `docker compose up -d`; named volumes remain untouched.
- Backup: copy `data/` and the labelled named volumes; bot source lives in git or the bots directory.

### Observability

- Structured `tracing` logs to stdout: startup, token generation (only once), Docker connection failures, scan errors, build/start/stop outcomes, orphan cleanup.
- Per-bot last error is exposed in the API and UI; full build/container logs are not part of v1.
- No metrics, traces, or telemetry.
- Docker health: Anqa's own healthcheck calls the binary's `healthcheck` subcommand; bot health comes from the bot image's `HEALTHCHECK`.

### Testing

- **Unit:** manifest parsing and validation, directory discovery, slug and volume prefixing, status/health mapping, uptime formatting, token generation/comparison, cookie/auth middleware, API error mapping.
- **Integration (real Docker):** build a fixture bot image, create/start/stop, read status, toggle autostart and verify restart policy, recreate on config change, preserve named volumes, clean an orphan with the sentinel present, skip cleanup when absent, auth-protected API.
- **Manual browser:** login/logout, empty and populated states, start/stop/build failure, autostart switch, theme and accent switching without flash, keyboard traversal, focus visibility, mobile card layout, Docker-unavailable banner.
- **CI:** format check, clippy with `-D warnings`, unit tests, Docker integration tests on the GitHub runner, release build. No Playwright.
- **Release:** tagged `v0.1.0` builds and pushes `linux/amd64` and `linux/arm64` to GHCR with OCI labels and provenance.

### Public project packaging

- README: what Anqa is, screenshot, security warning, quickstart, bot directory contract, manifest reference, autostart semantics, named volumes/backup, API reference, development, deployment, license, Telegram trademark disclaimer.
- AGENTS.md: project commands, architecture summary, design record, conventions, deployment paths.
- LICENSE: MIT, copyright 2026 benyamin-git.
- `assets/fonts/OFL.txt`: JetBrains Mono OFL license.
- `.github/workflows/ci.yml` and `release.yml`; Dependabot for Cargo, Actions, and Docker.
- Repo description and topics: `telegram`, `telegram-bot`, `self-hosted`, `docker`, `rust`, `dashboard`, `axum`.
- No standalone binaries; the container image is the release artifact.

## Risks + rollback

| Risk | Mitigation |
| --- | --- |
| Docker socket is root-equivalent | Mandatory token auth; HttpOnly cookie; README warning at the top; recommend trusted host and TLS reverse proxy; no default open access. |
| Build context handling differs from Docker CLI | Implement and test `.dockerignore` semantics; exclude `.env` and `.git`; integration tests with nested paths and ignores. |
| Rebuilding every Start is slow for large bots | Docker layer cache; `.dockerignore`; small Telegram bots; document the tradeoff. |
| Docker API/version changes | Use bollard's negotiated API; pin the crate; integration tests against Docker 29; CI on the runner's Docker. |
| UID/GID and socket GID differ per host | Compose exposes `ANQA_UID`, `ANQA_GID`, `DOCKER_GID`; docs show `stat -c '%g' /var/run/docker.sock`; defaults for this host. |
| Orphan cleanup removes a container accidentally | `/data/.anqa-volume` sentinel; skip if the bots root is unreadable or freshly mounted; named volumes are never removed; log every cleanup. |
| Named volumes are less discoverable than a data folder | Document `docker volume ls`, labels, backup, and restore; the UI shows declared volumes on the card/API. |
| Token lost or leaked | Stored at `data/token`; delete and restart to rotate; `ANQA_TOKEN` for automation; logout clears the cookie; changing the token invalidates sessions. |
| First-run token is hard to find | Printed once in logs; also written to the bind-mounted `data/token`; README quickstart shows both. |
| Port 7409 conflict | Configurable via `ANQA_PORT`; reserved and registered now; fail with a clear bind error. |
| Public image supply chain | GHCR with provenance; pinned GitHub Actions; Dependabot; no third-party runtime assets except the bundled OFL font. |
| Rust compile time slows CI | Cargo cache; release only on tags; integration tests reuse the debug/release binary; multi-arch build in CI. |
| A bot publishes an unexpected host port | Ports are opt-in in `bot.toml`; docs warn that published ports are reachable like any Docker port. |
| A running bot is unhealthy but looks fine | Docker health is shown when the image defines `HEALTHCHECK`; no Telegram probing by design. |
| User expects logs in the UI | Explicit non-goal; card shows the last operation error; README points to `docker logs`. |

**Rollback:** `v0.1.0` has no data migrations. Pin the previous image tag and `docker compose up -d`. Bot containers and named volumes are independent of the dashboard image. If a new container configuration is wrong, fix `bot.toml` and Start again; named volumes persist. If the dashboard itself is unusable, stop it with `docker compose down`; bots with `always` continue to run and restart on boot until their policies are changed.

## Dependencies / ordering

1. **User prerequisite:** run `sudo usermod -aG docker benyamin`, then re-login, so local Docker and integration tests work without sudo. The agent cannot run this password-protected command.
2. **GitHub:** create the empty public `benyamin-git/Anqa` repository (or install `gh`). Planning commits are local and do not require the remote.
3. **Port:** 7409 is free; add `anqa: 7409` to the dashboard config during implementation.
4. **Toolchain:** Rust 1.98.1 is installed; Docker 29.5.3 and Compose 5.1.4 are installed; Docker GID is 988.
5. **Rust crates:** axum, tokio, bollard, rust-embed, serde, toml, dotenvy, tracing, tower-http (compatibility to be verified), tar/ignore/globset, rand, base64, subtle, time, anyhow/thiserror. Exact versions are pinned in the plan.
6. **Assets:** JetBrains Mono variable font (OFL) and its license file.
7. **CI:** GitHub Actions with Docker available on `ubuntu-latest`; GHCR permissions for the repository.
8. **Ordering:** repo/docs → skeleton binary and config → scanner/manifest → Docker lifecycle → API/auth → UI → sample/fixtures → compose/deploy → CI/release → docs/screenshot. The implementation plan will break this into commit-sized tasks.

## Open questions

None. Every decision above is either `decided` or `assumed`; the assumed items are flagged for review and can be corrected before approval.
