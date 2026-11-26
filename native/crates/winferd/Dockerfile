# winferd, winfer for servers, as a container. Build from the repository root:
#   docker build -f native/crates/winferd/Dockerfile -t winferd .
# Run (pair with a link from winfer: Settings → Connect):
#   docker run -d --name winferd --restart unless-stopped -v winferd:/data -e WINFERD_PAIR='<link>' winferd
# Everything it keeps (pairings, sessions, agents' sign-ins under $HOME) is in /data.
# --build-arg BIN=release --build-arg VERSION=v0.1.102 takes the release's binary instead of
# compiling (release.yml, for both architectures).

ARG BIN=source

FROM rust:1-bookworm AS source
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/native/target \
    cd native && cargo build --release --locked -p winferd && cp target/release/winferd /usr/local/bin/winferd

FROM debian:bookworm-slim AS release
ARG VERSION
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && arch=$([ "$TARGETARCH" = arm64 ] && echo aarch64 || echo x86_64) \
 && base="https://github.com/RickyDane/winfer/releases/download/$VERSION/winferd-linux-$arch.tar.gz" \
 && curl -fsSL "$base" -o /tmp/w.tgz && curl -fsSL "$base.sha256" -o /tmp/w.sha \
 && [ "$(sha256sum /tmp/w.tgz | cut -d' ' -f1)" = "$(tr -d ' \n' < /tmp/w.sha)" ] \
 && tar -xzf /tmp/w.tgz -C /usr/local/bin winferd && rm /tmp/w.*

FROM ${BIN} AS bin

# Node 22+ for the agent CLIs (Claude Code needs it); Chromium for the workspaces' browser
# (agents' browser tools, and the desktop's browser panel on this server).
FROM node:22-bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git bash procps less tini \
    chromium fonts-liberation fonts-noto-color-emoji openssh-client \
 && rm -rf /var/lib/apt/lists/*
COPY --from=bin /usr/local/bin/winferd /usr/local/bin/winferd
COPY native/crates/winferd/docker/entrypoint.sh /usr/local/bin/winferd-entrypoint
# Agents (WINFERD_AGENTS) install into /usr/local, on the login PATH; a new container installs
# them again on first start. Their sign-ins live under $HOME, in the volume.
ENV XDG_DATA_HOME=/data \
    HOME=/data/home
VOLUME /data
WORKDIR /data/home
ENTRYPOINT ["tini", "--", "winferd-entrypoint"]
CMD ["run"]
