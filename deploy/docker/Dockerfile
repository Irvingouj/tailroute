# syntax=docker/dockerfile:1
# Production image: default features include `backup`; no `dev-defaults`.
FROM rust:1-bookworm AS builder
WORKDIR /app

# Dependency layer cache: only recompile crates when sources change.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p tailsvc-controller -p tailsvc-agent \
 && strip target/release/tailsvc-controller target/release/tailsvc-agent

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home /var/lib/tailsvc --create-home tailsvc

COPY --from=builder /app/target/release/tailsvc-controller /usr/local/bin/
COPY --from=builder /app/target/release/tailsvc-agent /usr/local/bin/

# Host-network / Tailscale binds often are not 127.0.0.1 — disable generic HEALTHCHECK.
# Compose/systemd should probe the real listen address.
HEALTHCHECK NONE
