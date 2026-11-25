# Build from the workspace root:  docker build -f server/signal-relay/Dockerfile .
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
COPY server ./server
RUN cargo build --release -p rv-signal-relay

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/rv-signal-relay /usr/local/bin/rv-signal-relay
ENV RV_PORT=21116
EXPOSE 21116/udp
USER nonroot
ENTRYPOINT ["/usr/local/bin/rv-signal-relay"]
