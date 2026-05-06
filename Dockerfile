FROM rust:latest AS builder
WORKDIR /app
COPY Cargo.toml .
COPY src ./src
COPY .sqlx ./.sqlx 
ENV SQLX_OFFLINE=true

RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/backend-website .
EXPOSE 8000
CMD ["./backend-website"]
