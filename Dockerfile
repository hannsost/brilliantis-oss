# Pinned official images. The default build produces the independent distribution.
ARG RUST_IMAGE=docker.io/library/rust:1.98.1-bookworm@sha256:c49256cbe5ea0188bc658a689500d70c41eb51f009a7a7be209caf60a944f3ec
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian12:nonroot@sha256:777e96cf322c46bc32aca926c263624c4dc8d7cf37e2fa65ba2c7e697318ebbb
FROM ${RUST_IMAGE} AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY static ./static
COPY docs ./docs
COPY distributions ./distributions
COPY migrations ./migrations
ARG PACKAGE=brilliantis
ARG CARGO_NET_OFFLINE=false
ARG CARGO_BUILD_JOBS=2
RUN --mount=type=cache,id=brilliantis-target-bookworm,target=/build/target cargo build --locked --release --package "$PACKAGE" && \
    install -D -m 0755 "target/release/$PACKAGE" /out/brilliantis && \
    install -d -m 0700 -o 10001 -g 10001 /out/data

FROM ${RUNTIME_IMAGE}
LABEL org.opencontainers.image.title="Brilliantis" \
      org.opencontainers.image.version="0.2.0" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /out/brilliantis /usr/local/bin/brilliantis
COPY --from=build --chown=10001:10001 /out/data /data
USER 10001:10001
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/brilliantis"]
CMD ["--database", "/data/brilliantis.sqlite"]
