# Build stage
FROM rust:1.77-slim AS builder
WORKDIR /app
COPY server/Cargo.toml server/Cargo.lock ./server/
COPY server/src ./server/src
# Build in release mode
RUN cargo build --release --manifest-path server/Cargo.toml

# Runtime stage
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /app/server/target/release/kanban-server /usr/local/bin/kanban-server
EXPOSE 8787
ENV KANBAN_PORT=8787
CMD ["kanban-server"]
