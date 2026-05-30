FROM rust:1.86.0 AS builder

RUN apt update && apt install -y build-essential pkg-config ca-certificates

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY build.rs favicon-192x192.png logo-512x512.png ./
COPY src ./src

RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt update && apt install -y ca-certificates && rm -rf /var/lib/apt/lists/*

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

EXPOSE 8080/tcp

COPY --from=builder /src/target/release/driftdns /usr/local/bin/driftdns

ENTRYPOINT ["/usr/local/bin/driftdns"]
CMD ["--config", "/data/ddns.yaml"]