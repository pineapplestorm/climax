# Climax headless server (SERVER-PLAN.md Milestone C).
#
# Multi-stage: the SvelteKit SPA builds in a node stage, the Rust binary builds
# in a rust stage (rust-embed bakes the SPA in - release mode embeds bytes, so
# the final binary is fully self-contained), and the runtime stage is a slim
# Debian with just the binary + CA roots. reqwest uses rustls (no OpenSSL) and
# sqlx bundles SQLite, so no other system libraries are needed.
#
# Build:  docker build -t climax-server .
# Run:    docker run -d -p 9998:9998 -v climax-data:/data \
#           -e CLIMAX_TOKEN=change-me climax-server
# See deploy/README.md for compose + bare-Linux + full env reference.
#
# One image, two CPU types (amd64 for ordinary PCs and most NAS boxes, arm64 for
# Raspberry Pi and ARM NAS boxes). The two build stages always run on the
# BUILDER's own CPU and cross-compile for the target, so an arm64 image never
# compiles Rust under emulation (which takes hours). Only the tiny runtime stage
# runs as the target CPU. .github/workflows/docker-image.yml publishes both on
# every release; a plain `docker build` just builds for the machine it runs on.

# ---- Stage 1: the web UI ----
# The output is plain HTML/JS/CSS, identical for every CPU, so build it once.
FROM --platform=$BUILDPLATFORM node:22-slim AS web
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY svelte.config.js vite.config.js tsconfig.json ./
COPY static ./static
COPY src ./src
RUN npm run build

# ---- Stage 2: the server binary ----
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
# Set by Docker to the CPU the image is FOR (amd64 / arm64).
ARG TARGETARCH
WORKDIR /app
# Pick the Rust target for that CPU and, when it is not the builder's own, add
# the matching C cross-compiler (bundled SQLite and ring both compile C).
RUN case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu;  gnu=x86-64-linux-gnu;  libc=amd64 ;; \
      arm64) triple=aarch64-unknown-linux-gnu; gnu=aarch64-linux-gnu; libc=arm64 ;; \
      *) echo "Unsupported CPU type: '$TARGETARCH' (amd64 and arm64 only)" >&2; exit 1 ;; \
    esac \
    && echo "$triple" > /rust-target \
    && rustup target add "$triple" \
    && if [ "$(dpkg --print-architecture)" != "$TARGETARCH" ]; then \
         apt-get update \
         && apt-get install -y --no-install-recommends "gcc-$gnu" "libc6-dev-$libc-cross" \
         && rm -rf /var/lib/apt/lists/*; \
       fi
# Debian names every gcc by its target, native ones included, so these hold
# whether or not the build is crossing.
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
# The whole workspace manifest set is needed for cargo to parse the workspace,
# but only climax-server (+ climax-core) actually compiles - the Tauri desktop
# shell is parsed, never built.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri ./src-tauri
# The SPA build lands at repo-root build/ - exactly where climax-server's
# rust-embed expects it (its build.rs would create an empty one otherwise, which
# compiles but serves no UI; copying the real build BEFORE cargo runs means the
# release binary embeds the actual assets).
COPY --from=web /app/build ./build
RUN cargo build --release -p climax-server --target "$(cat /rust-target)" \
    && cp "target/$(cat /rust-target)/release/climax-server" /climax-server

# ---- Stage 3: runtime ----
FROM debian:bookworm-slim
# ca-certificates for rustls trust; tzdata so a named TZ (e.g. Europe/Berlin)
# resolves - without it the server's chrono::Local falls back to UTC and day
# boundaries (assigned_day) drift for clients in other timezones. Set the actual
# zone with the TZ env (see docker-compose.yml).
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 1000 climax \
    && mkdir -p /data \
    && chown climax:climax /data
COPY --from=build /climax-server /usr/local/bin/climax-server
USER climax
# 0.0.0.0 is correct INSIDE a container (the container's localhost isn't the
# host's); publish/firewall decides actual exposure. Set CLIMAX_TOKEN - the
# server warns loudly if reachable beyond loopback without one.
ENV CLIMAX_DATA_DIR=/data \
    CLIMAX_BIND=0.0.0.0 \
    CLIMAX_PORT=9998
EXPOSE 9998
# The database lives here - mount a volume or a restart loses everything.
VOLUME /data
ENTRYPOINT ["climax-server"]
