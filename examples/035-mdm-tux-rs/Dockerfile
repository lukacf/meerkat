# syntax=docker/dockerfile:1.7

FROM rust:1.94-bookworm AS build

ARG CARGO_PROFILE=debug

WORKDIR /src
COPY . .

RUN if [ "${CARGO_PROFILE}" = "release" ]; then \
      cargo build --manifest-path examples/035-mdm-tux-rs/Cargo.toml --release --locked; \
    else \
      cargo build --manifest-path examples/035-mdm-tux-rs/Cargo.toml --locked; \
    fi

FROM debian:bookworm-slim

ARG CARGO_PROFILE=debug

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/examples/035-mdm-tux-rs/target/${CARGO_PROFILE}/mdm-kennel /usr/local/bin/mdm-kennel
COPY --from=build /src/examples/035-mdm-tux-rs/target/${CARGO_PROFILE}/mdm-target /usr/local/bin/mdm-target
COPY --from=build /src/examples/035-mdm-tux-rs/target/${CARGO_PROFILE}/mdm-tux /usr/local/bin/mdm-tux

ENV RUST_LOG=warn

WORKDIR /data
