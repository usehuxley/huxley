# syntax=docker/dockerfile:1.7
# Huxley server: API, worker, and scheduler roles in one binary.
# Build from the repo root: docker build -f docker/huxley.Dockerfile .

FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p huxley-server \
    && cp target/release/huxley /huxley

# Distroless: no shell, no package manager, runs as a non-root user.
FROM gcr.io/distroless/cc-debian13:nonroot
LABEL org.opencontainers.image.title="huxley" \
      org.opencontainers.image.description="Huxley automation platform server" \
      org.opencontainers.image.source="https://github.com/usehuxley/huxley" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /huxley /usr/local/bin/huxley
USER nonroot:nonroot
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/huxley", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/huxley"]
CMD ["serve", "--roles", "all"]
