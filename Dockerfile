# syntax=docker/dockerfile:1

FROM rust:1.100.0-alpine3.22 AS builder
RUN apk add --no-cache build-base
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=builder /src/target/release/anqa /usr/local/bin/anqa
WORKDIR /data
USER nonroot:nonroot
EXPOSE 7409
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/anqa", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/anqa"]
