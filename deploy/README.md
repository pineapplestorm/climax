# Running the Climax server (Docker / NAS / bare Linux)

The Climax server is the "home": it owns the database, talks to Stash, and
serves the full web dashboard plus the API every client uses (browser, the
desktop app in client mode, the Stash bridge). One home per household - the
database lives only here.

A browser at `http://<server>:9998` gets the complete dashboard. The desktop
app is an optional extra client (tray, global shortcut, idle detection); it is
never required.

## Environment reference

| Variable | Default | What it does |
| --- | --- | --- |
| `CLIMAX_DATA_DIR` | platform data dir | Where the SQLite database + settings live. In Docker this is `/data` - mount a volume there or a restart loses everything. |
| `CLIMAX_PORT` | `9998` | HTTP port. |
| `CLIMAX_BIND` | `127.0.0.1` | Bind address. Set `0.0.0.0` to be reachable from other devices (the Docker image already does; publishing the port is what actually exposes it). |
| `CLIMAX_TOKEN` | unset | Shared auth token. When set, the data endpoints require it; the web UI prompts for it once and remembers it. Required in practice for anything beyond localhost - the server logs a loud warning if exposed without one. Note an unprotected server isn't just readable: anyone who can reach it can also restore or reset the database. |
| `CLIMAX_ALLOWED_HOSTS` | unset | Comma-separated extra hostnames allowed past the DNS-rebinding guard. IP literals, `localhost`, single-label names, and `*.local` always pass; add public DNS names here (e.g. a Tailscale `ts.net` name). |

## Docker

A ready-made image is published with every release, for both amd64 (ordinary
PCs and most NAS boxes) and arm64 (Raspberry Pi and ARM NAS boxes). Docker
picks the right one by itself. There is nothing to build:

```
docker run -d --name climax \
  --restart unless-stopped \
  -p 9998:9998 \
  -v climax-data:/data \
  -e CLIMAX_TOKEN=change-me \
  ghcr.io/pineapplestorm/climax:latest
```

Or with compose. [`docker-compose.yml`](../docker-compose.yml) in the repo root
works on its own, so copy it wherever you keep your compose files, or paste its
`climax` service into one you already have. Change the token first, then:

```
docker compose up -d
```

Then open `http://<host>:9998`, enter the token, and connect Stash under
Settings -> Stash configuration (use the address the SERVER can reach Stash at,
e.g. `http://192.168.1.20:9999`, not `localhost`, unless Stash runs on the same
host network).

`latest` follows each release. To stay on one version until you choose to move,
use its number instead, e.g. `ghcr.io/pineapplestorm/climax:1.0.0`.

To update, fetch the new image and recreate the container. Your library is in
the volume, so it carries over:

```
docker compose pull
docker compose up -d
```

### Building the image yourself

Only needed if you want to run changes of your own. From the repo root:

```
docker build -t climax-server .
```

Then use `climax-server` in place of the `ghcr.io/...` name above, or swap the
`image:` line in `docker-compose.yml` for `build: .`. This builds for the
machine it runs on; add `--platform linux/arm64` (or `linux/amd64`) to build
for the other kind.

## Bare Linux (no Docker)

Build the binary (needs Rust + Node):

```
npm ci && npm run build
cargo build --release -p climax-server
```

`npm run build` must run BEFORE the cargo build - the release binary embeds the
web UI from `build/`. Then install it as a service: see
`deploy/climax-server.service` (instructions in the file's header comment).

## Pointing clients at it

- **Browser**: `http://<server>:9998` - that's the whole dashboard.
- **Desktop app**: Settings -> Server / connection -> enter the address + token
  -> Apply and restart. The app becomes a thin client; the session lives on the
  server.
- **Stash bridge**: the bridge reports watching to the server over WebSocket.
  In Stash's plugin settings, point its Climax address at the server (e.g.
  `http://192.168.1.30:9998`) and paste the token (bridge v0.7.0+).

## Backing up, restoring, resetting

Everything lives in the data dir (`/data` in Docker). Backup options:

- Settings -> Backup and restore -> "Download a backup" in the web UI: the
  server snapshots its own database and your browser saves it - works from any
  device.
- Back up the volume / data dir directly (stop the container first, or use the
  web download, which snapshots safely while running).

Restore and reset also live on that page: restore uploads a backup file to the
server, reset wipes it clean. Both work by staging the change and RESTARTING
the server process - so they need a supervisor that restarts it: Docker's
`--restart unless-stopped` (in the run command above; the compose file and the
systemd unit already have their equivalents). A bare binary with no supervisor
just exits and stays down - start it again by hand and the staged restore or
reset applies on boot. Before each restore the server saves the outgoing
library next to the database as a `*-pre-restore-*.sqlite` rollback file.

## One writer per database

Never point two Climax backends (the container AND a desktop app in host mode)
at the same database file at once - two SQLite writers corrupt it. The desktop
app connecting in CLIENT mode is fine (it talks over HTTP; the server is the
only writer).
