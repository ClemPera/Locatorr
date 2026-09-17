# ── Builder stage ────────────────────────────────────────────────────────────
FROM golang:1.26 AS builder

WORKDIR /src

COPY server/go.mod server/go.sum ./
RUN go mod download

COPY server/ ./

RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/locatorr-server .

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /out/locatorr-server /usr/local/bin/locatorr-server
COPY server/schema.sql /app/schema.sql

# main.go calls godotenv.Load() and panics when .env is absent, so ship an empty
# placeholder. Real configuration comes from container environment variables
# (notably DATABASE_URL).
RUN touch /app/.env

EXPOSE 9191

CMD ["locatorr-server"]
