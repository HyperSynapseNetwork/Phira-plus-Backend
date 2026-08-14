# syntax=docker/dockerfile:1.7

FROM rust:1.96-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p ppb-server --bin ppb-server --bin ppctl

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates wget \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --create-home --home-dir /var/lib/ppb ppb
COPY --from=build /src/target/release/ppb-server /usr/local/bin/ppb-server
COPY --from=build /src/target/release/ppctl /usr/local/bin/ppctl
USER ppb
WORKDIR /var/lib/ppb
EXPOSE 8080
ENV PPB_RUNTIME_CONFIG=/etc/ppb/ppb.toml
CMD ["ppb-server", "--config", "/etc/ppb/ppb.toml"]
