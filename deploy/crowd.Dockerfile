# A test instance and the crowd load generator (server/examples/crowd.rs), for
# load runs in a Railway project of their own. Never for fuwa.chat. One image
# for both: the service's start command picks `fuwa` or `crowd`.
FROM rust:1.99.0-bookworm@sha256:59037199c44290f2befcdd58dcc540164763fc296950255aaefeef096a1866b0 AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p fuwa-server --bin fuwa --example crowd \
    && cp target/release/fuwa target/release/examples/crowd /usr/local/bin/

FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa
COPY --from=build /usr/local/bin/fuwa /usr/local/bin/crowd /usr/local/bin/
ENV FUWA_DATA_PATH=/data FUWA_PORT=8080
EXPOSE 8080
CMD ["fuwa"]
