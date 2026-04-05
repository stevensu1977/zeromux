# ── Stage 1: Build frontend ──
FROM node:22-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm ci
COPY frontend/ .
RUN npm run build

# ── Stage 2: Build Rust binary ──
FROM rust:1-slim-bookworm AS backend
RUN apt-get update && apt-get install -y pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY --from=frontend /app/frontend/dist frontend/dist
RUN cargo build --release

# ── Stage 3: Minimal runtime ──
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    tmux bash ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=backend /app/target/release/zeromux /usr/local/bin/zeromux

ENV ZEROMUX_HOST=0.0.0.0
ENV ZEROMUX_PORT=8080
EXPOSE 8080

ENTRYPOINT ["zeromux"]
CMD ["--host", "0.0.0.0", "--port", "8080"]
