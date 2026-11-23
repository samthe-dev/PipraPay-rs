# syntax=docker/dockerfile:1
FROM rust:1.100-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src
COPY . .
RUN cargo build --release && strip target/release/piprapay-rs

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/piprapay-rs /piprapay-rs
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENV DATABASE_PATH=/data/piprapay.db
ENV LISTEN_ADDR=0.0.0.0:8080
VOLUME ["/data"]
EXPOSE 8080
# Healthcheck is done by the deployer against GET /healthz; the distroless
# image has no shell or curl, so no in-image HEALTHCHECK is set here.
ENTRYPOINT ["/piprapay-rs"]
