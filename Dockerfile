# The fuwa server. Self-host with:
#   docker run -d -p 8080:8080 -v fuwa:/data ghcr.io/waifu-devs/fuwa
#
# Builds of one commit must come out byte-identical, so anyone can rebuild a
# release and check it (the Reproducible build workflow does this on every
# change). So base images are pinned by digest, and nothing that differs
# between builds (a time, a random value, an absolute path) goes in the binary.
# To move to a newer base, change the tag and its digest together.

# The web client, which the server carries inside its binary.
FROM node:22.23.3-bookworm-slim@sha256:43ac6c60b8f89723f746e8a92ce91abd5017e627ce1ddfe4238355d3a30b772c AS web
WORKDIR /src/web
RUN npm install --global pnpm@10.33.0
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web ./
RUN pnpm run build

FROM rust:1.99.0-bookworm@sha256:59037199c44290f2befcdd58dcc540164763fc296950255aaefeef096a1866b0 AS build
WORKDIR /src
# Crates that stamp a build time into the binary (turso does, and C code using
# __DATE__) read this instead of the clock, so every build comes out the same.
ENV SOURCE_DATE_EPOCH=0

# Build the dependencies on their own first, so code changes don't rebuild them.
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY server/Cargo.toml server/build.rs server/
COPY proto proto
RUN mkdir -p server/src \
    && echo 'fn main() {}' > server/src/main.rs \
    && touch server/src/lib.rs \
    && cargo build --release --locked --features web --bin fuwa \
    && rm -rf server/src

COPY server server
COPY --from=web /src/web/dist web/dist
# The commit, for GetNode (there's no .git in here). After the dependencies, so
# a new commit doesn't rebuild them.
ARG FUWA_COMMIT
RUN touch server/src/main.rs server/src/lib.rs \
    && cargo build --release --locked --features web --bin fuwa \
    && cp target/release/fuwa /fuwa

# Just the binary: `docker build --target binary --output type=local,dest=out .`
FROM scratch AS binary
COPY --from=build /fuwa /fuwa

FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa
LABEL org.opencontainers.image.source="https://github.com/waifu-devs/fuwa" \
      org.opencontainers.image.description="fuwa: a self-hostable, Discord-like chat server"
COPY --from=build /fuwa /usr/local/bin/fuwa
ENV FUWA_DATA_PATH=/data \
    FUWA_PORT=8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/usr/local/bin/fuwa", "health"]
ENTRYPOINT ["/usr/local/bin/fuwa"]
