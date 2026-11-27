# One image holds every program of the deployment manifest at the path the manifest names.
# The composition (`bijective-deploy compose`) chooses which program a container runs.

FROM rust:1.97.1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked --workspace --bins \
 && mkdir -p /out \
 && find target/release -maxdepth 1 -type f -perm -u+x -name 'bijective-*' -exec cp {} /out/ \;

FROM debian:bookworm-slim
# The venue sockets verify TLS against the system's trust roots, and the slim base ships none:
# without this package no venue connection can be made from the image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
# Fixed numeric identity so a volume's ownership means the same on every host.
RUN useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin bijective \
 && mkdir -p /opt/bijective/bin /var/lib/bijective/store /run/bijective \
 && chown -R bijective /var/lib/bijective /run/bijective
COPY --from=build /out/ /opt/bijective/bin/
USER bijective
