# syntax=docker/dockerfile:1.7
# Huxley script runner: `huxley runner` plus rootless Podman, which runs
# each script in its own sandbox container (design doc §6.9, docs/runners.md).
# Build from the repo root: docker build -f docker/runner.Dockerfile .
#
# Run it with what nested rootless Podman needs, and nothing more:
#   --device /dev/fuse --security-opt label=disable
# Never mount a Docker or Podman socket into it.

FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p huxley-server \
    && cp target/release/huxley /huxley

# Podman's own image, set up for rootless use as the `podman` user.
FROM quay.io/podman/stable:v6.2
LABEL org.opencontainers.image.title="huxley-runner" \
      org.opencontainers.image.description="Huxley script runner (rootless Podman sandboxes)" \
      org.opencontainers.image.source="https://github.com/usehuxley/huxley" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /huxley /usr/local/bin/huxley
RUN mkdir -p /var/lib/huxley-runner && chown podman:podman /var/lib/huxley-runner
# Enrolment state (key, certificate) and the script images both persist.
VOLUME ["/var/lib/huxley-runner", "/home/podman/.local/share/containers"]
ENV HUXLEY_RUNNER_STATE=/var/lib/huxley-runner
USER podman
WORKDIR /home/podman
ENTRYPOINT ["/usr/local/bin/huxley", "runner"]
CMD ["run"]
