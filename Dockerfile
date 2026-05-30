FROM rust:1.86.0-alpine AS builder

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY build.rs favicon-192x192.png logo-512x512.png ./
COPY src ./src

RUN cargo build --release --locked --target x86_64-unknown-linux-musl

FROM scratch

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/driftdns /usr/local/bin/driftdns

ENTRYPOINT ["/usr/local/bin/driftdns"]
CMD ["--config", "/config/ddns.yaml"]
