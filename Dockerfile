# syntax=docker/dockerfile:1
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
#
# Two images from one build:
#
#   docker build --target server -t pistack .
#   docker build --target agent  -t pistack:agent .
#
# server is the control plane on a distroless base, running as a non-root
# user. agent adds the host helpers and what they call, and runs as root.
# docs/RELEASING.md says what each can and cannot do in a container.

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p pistack -p bench \
 && install -d /out/bin /out/sbin /out/data \
 && install -m 0755 target/release/pistack target/release/pi /out/bin/ \
 && install -m 0755 scripts/pistack-overlay.py /out/sbin/pistack-overlay \
 && install -m 0755 scripts/pistack-fip.py /out/sbin/pistack-fip \
 && install -m 0755 scripts/pistack-shelf-helper scripts/pistack-youki-cleanup /out/sbin/

# The control plane. No shell, no package manager, uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot AS server
LABEL org.opencontainers.image.source="https://github.com/PIndustries/pistack" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.title="PiStack server"
COPY --from=build /out/bin/ /usr/local/bin/
COPY --from=build --chown=65532:65532 /out/data /var/lib/pistack
COPY LICENSE NOTICE /usr/share/doc/pistack/
ENV PISTACK_DATA_DIR=/var/lib/pistack
VOLUME /var/lib/pistack
EXPOSE 8443
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/pistack"]
CMD ["server", "--init"]

# The host agent. Root, because everything it does to a host needs root.
FROM debian:bookworm-slim AS agent
LABEL org.opencontainers.image.source="https://github.com/PIndustries/pistack" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.title="PiStack agent"
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates dnsmasq-base iproute2 iptables python3 sudo \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/bin/ /usr/local/bin/
COPY --from=build /out/sbin/ /usr/local/sbin/
COPY LICENSE NOTICE /usr/share/doc/pistack/
ENV PISTACK_DATA_DIR=/var/lib/pistack
VOLUME /var/lib/pistack
ENTRYPOINT ["/usr/local/bin/pistack"]
CMD ["agent", "--help"]
