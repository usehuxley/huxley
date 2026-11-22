# syntax=docker/dockerfile:1.7
# Huxley SvelteKit apps. Build from the repo root:
#   docker build -f docker/frontend.Dockerfile --build-arg APP=web .
#   docker build -f docker/frontend.Dockerfile --build-arg APP=portal .

ARG APP=web

FROM node:24-slim AS build
ARG APP
RUN npm install --global pnpm@12.21.0
WORKDIR /src
# Install dependencies first so this layer is cached across source changes.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml .npmrc ./
COPY apps/web/package.json apps/web/
COPY apps/portal/package.json apps/portal/
COPY packages/ui/package.json packages/ui/
COPY packages/api-client/package.json packages/api-client/
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile --filter "@usehuxley/${APP}..."
COPY packages packages
COPY apps/${APP} apps/${APP}
RUN pnpm --filter "@usehuxley/${APP}" build

# adapter-node bundles every dependency into build/, so no node_modules ship.
FROM gcr.io/distroless/nodejs24-debian13:nonroot
ARG APP
LABEL org.opencontainers.image.title="huxley-${APP}" \
      org.opencontainers.image.source="https://github.com/usehuxley/huxley" \
      org.opencontainers.image.licenses="MIT"
WORKDIR /app
COPY --from=build /src/apps/${APP}/build ./build
COPY --chmod=444 <<'JSON' ./package.json
{ "type": "module" }
JSON
ENV NODE_ENV=production HOST=0.0.0.0 PORT=3000
USER nonroot:nonroot
EXPOSE 3000
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/nodejs/bin/node", "-e", "fetch('http://127.0.0.1:3000/healthz').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"]
CMD ["build"]
