# Build stage
FROM rust:latest AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy all source files
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/standx-mm /app/standx-mm

# Copy config example
COPY config.example.yaml /app/config.example.yaml

# Create non-root user
RUN useradd -r -s /bin/false standx && chown -R standx:standx /app
USER standx

# Default config path
ENV RUST_LOG=info,standx_mm=debug

ENTRYPOINT ["/app/standx-mm"]
CMD ["config.yaml"]
