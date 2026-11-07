# syntax=docker/dockerfile:1

# ── Build stage ───────────────────────────────────────────────────────────
# The build context is the repository root (see docker-compose.yml) so the
# `path = "../.."` dependency on corrosive_agents resolves.
FROM rust:1-bookworm AS builder
WORKDIR /src

# Copy the whole repo (a root .dockerignore keeps target/, .git and .env out).
COPY . .

WORKDIR /src/usecases/decentralised_agents
RUN cargo build --release --locked || cargo build --release
RUN cp target/release/decentralised_agents /usr/local/bin/decentralised_agents

# ── Runtime stage ─────────────────────────────────────────────────────────
# Slim image; only CA certificates are needed for HTTPS to the NVIDIA NIM API
# (reqwest uses rustls, so there is no OpenSSL runtime dependency).
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/decentralised_agents /usr/local/bin/decentralised_agents

ENV RUST_LOG=info,decentralised_agents=info
# The role is supplied per-container via the ROLE env var (see compose).
ENTRYPOINT ["decentralised_agents"]
