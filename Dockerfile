# The fuwa server. Self-host with:
#   docker run -d -p 8080:8080 -v fuwa:/data ghcr.io/waifu-devs/fuwa

FROM rust:1-bookworm AS build
WORKDIR /src

# Build the dependencies on their own first, so code changes don't rebuild them.
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY server/Cargo.toml server/build.rs server/
COPY proto proto
RUN mkdir -p server/src \
    && echo 'fn main() {}' > server/src/main.rs \
    && touch server/src/lib.rs \
    && cargo build --release --locked --bin fuwa \
    && rm -rf server/src

COPY server server
RUN touch server/src/main.rs server/src/lib.rs \
    && cargo build --release --locked --bin fuwa \
    && cp target/release/fuwa /fuwa

FROM gcr.io/distroless/cc-debian12
LABEL org.opencontainers.image.source="https://github.com/waifu-devs/fuwa" \
      org.opencontainers.image.description="fuwa: a self-hostable, Discord-like chat server"
COPY --from=build /fuwa /usr/local/bin/fuwa
ENV FUWA_DATA_PATH=/data \
    FUWA_PORT=8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/usr/local/bin/fuwa", "health"]
ENTRYPOINT ["/usr/local/bin/fuwa"]
