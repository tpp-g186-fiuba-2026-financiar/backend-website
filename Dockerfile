FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml .
COPY src ./src
COPY .sqlx ./.sqlx
COPY migrations ./migrations
ENV SQLX_OFFLINE=true

RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/backend-website .
EXPOSE 8000
COPY cert.pem key.pem ./
CMD ["./backend-website"]
