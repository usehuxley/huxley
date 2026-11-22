# Huxley task runner. Install just: https://just.systems
# Run `just` to list recipes.

set dotenv-load := false

dev_db := "postgres://huxley:huxley@127.0.0.1:5432/huxley"

default:
    @just --list

# Start local dependencies (Postgres, Valkey, Mailpit, OTel + Grafana)
dev-up:
    docker compose -f docker-compose.dev.yml up -d --wait

# Stop local dependencies (add -v to the compose command to wipe data)
dev-down:
    docker compose -f docker-compose.dev.yml down

# Run the server with all roles against the dev database, exporting telemetry to otel-lgtm
server *args:
    HUXLEY_DATABASE__URL={{dev_db}} \
    HUXLEY_MIGRATE_ON_START=true \
    HUXLEY_TELEMETRY__LOG_FORMAT=pretty \
    HUXLEY_TELEMETRY__OTLP_ENDPOINT=http://127.0.0.1:4317 \
    HUXLEY_HTTP__TRUST_FORWARDED_FOR=true \
    HUXLEY_HTTP__COOKIE_SECURE=false \
    cargo run -p huxley-server -- serve {{args}}

# Create the root organization and first user in the dev database (prints a generated password)
bootstrap org="Dev MSP" email="admin@example.com" name="Dev Admin":
    HUXLEY_DATABASE__URL={{dev_db}} cargo run -p huxley-server -- admin bootstrap --org-name "{{org}}" --email "{{email}}" --name "{{name}}"

# Apply database migrations to the dev database
migrate:
    HUXLEY_DATABASE__URL={{dev_db}} cargo run -p huxley-server -- migrate

# Run the staff web app (http://localhost:5173)
web:
    pnpm --filter @usehuxley/web dev

# Run the client portal (http://localhost:5174)
portal:
    pnpm --filter @usehuxley/portal dev

# Install all dependencies
setup:
    pnpm install
    pnpm --filter @usehuxley/web exec playwright install chromium

# Format everything
fmt:
    cargo fmt --all
    pnpm format

# Everything CI checks, in CI's order
check: check-rust check-js

check-rust:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked

check-js:
    pnpm check
    pnpm lint
    pnpm test

# Rust tests including database integration tests (needs `just dev-up`)
test-db:
    HUXLEY_TEST_DATABASE_URL={{dev_db}} HUXLEY_REQUIRE_DB_TESTS=1 cargo test --workspace

# Browser end-to-end tests
e2e:
    pnpm test:e2e

# Regenerate the TypeScript API client from the server's OpenAPI document
api-client:
    pnpm api-client:generate

# License, advisory, and source checks (cargo install cargo-deny)
deny:
    cargo deny check

# Build the container images locally
images:
    docker build -f docker/huxley.Dockerfile -t ghcr.io/usehuxley/huxley:dev .
    docker build -f docker/frontend.Dockerfile --build-arg APP=web -t ghcr.io/usehuxley/web:dev .
    docker build -f docker/frontend.Dockerfile --build-arg APP=portal -t ghcr.io/usehuxley/portal:dev .
