FROM docker.io/library/rust:1.100-bookworm AS builder
# Proxy support for crate downloads behind an egress proxy
# (no-op when unset).
ARG HTTPS_PROXY
ARG HTTP_PROXY
ARG https_proxy
ARG http_proxy
# Optional: path (mounted into the build) of an extra CA bundle for
# MITM egress proxies. Unset = no-op.
ARG CA_BUNDLE_PATH
RUN if [ -n "$CA_BUNDLE_PATH" ] && [ -f "$CA_BUNDLE_PATH" ]; then \
      cp "$CA_BUNDLE_PATH" /usr/local/share/ca-certificates/extra-ca.crt && \
      update-ca-certificates; \
    fi
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src
COPY . .
# NOTE: cargo decides freshness by file mtime. The dummy src/main.rs
# created above has a NEWER mtime than the real sources copied in, so a
# plain `cargo build` would consider the dummy binary fresh and skip
# rebuilding (verified: STEP 12 finished in 0.92s with the dummy binary).
# `cargo clean --release -p` forces only our crate to rebuild; deps stay
# cached. The `--release` flag matters: without it, clean only touches
# the dev profile and removes 0 files, leaving the dummy binary in place.
RUN cargo clean --release -p piprapay-rs && cargo build --release && strip target/release/piprapay-rs
# Data directory for the final image (owned by the nonroot user, see below).
RUN mkdir -p /data && chown 65532:65532 /data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/piprapay-rs /piprapay-rs
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
# The image runs as nonroot (uid 65532). /data must be owned by that user
# in the image, otherwise the app cannot create its SQLite file: Docker
# initialises a fresh named volume by copying the image's /data (with its
# ownership), so a named volume works out of the box. A bind mount still
# needs the host directory chowned to 65532 (see README).
COPY --from=builder --chown=nonroot:nonroot /data /data
USER nonroot
ENV DATABASE_PATH=/data/piprapay.db
ENV LISTEN_ADDR=0.0.0.0:8080
VOLUME ["/data"]
EXPOSE 8080
# Healthcheck is done by the deployer against GET /healthz; the distroless
# image has no shell or curl, so no in-image HEALTHCHECK is set here.
ENTRYPOINT ["/piprapay-rs"]
