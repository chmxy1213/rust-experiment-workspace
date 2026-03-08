#!/usr/bin/env bash
set -euo pipefail

# One-click local run for all 3 apps. Requires a running SigNoz OTLP endpoint.
OTLP_ENDPOINT="${1:-http://127.0.0.1:4317}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

mkdir -p logs/signoz-demo

export OTEL_EXPORTER_OTLP_ENDPOINT="${OTLP_ENDPOINT}"
export RUST_LOG="info"

MACHINE_ID="local-gateway" LISTEN_ADDR="0.0.0.0:7001" INVENTORY_URL="http://127.0.0.1:7002" PAYMENT_URL="http://127.0.0.1:7003" \
  cargo run -p signoz-demo --bin gateway > logs/signoz-demo/gateway.log 2>&1 &

MACHINE_ID="local-inventory" LISTEN_ADDR="0.0.0.0:7002" \
  cargo run -p signoz-demo --bin inventory > logs/signoz-demo/inventory.log 2>&1 &

MACHINE_ID="local-payment" LISTEN_ADDR="0.0.0.0:7003" \
  cargo run -p signoz-demo --bin payment > logs/signoz-demo/payment.log 2>&1 &

sleep 2

echo "All apps started. Try: curl http://127.0.0.1:7001/checkout/book"
echo "Logs in: logs/signoz-demo/"
