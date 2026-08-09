FROM rust:1.94-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime
RUN useradd --system --uid 10001 fluxgate
COPY --from=builder /app/target/release/fluxgate /usr/local/bin/fluxgate
USER fluxgate
EXPOSE 8080
ENTRYPOINT ["fluxgate"]

