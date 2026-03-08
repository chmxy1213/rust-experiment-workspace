#!/usr/bin/env bash
set -euo pipefail

# Start one service process on an app node and export telemetry to SigNoz OTLP endpoint.
if [[ $# -lt 2 ]]; then
  echo "usage: $0 <gateway|inventory|payment> <otlp_endpoint> [listen_addr] [machine_id]"
  echo "example: $0 gateway http://10.0.0.8:4317 0.0.0.0:7001 node-b"
  exit 1
fi

SERVICE="$1"
OTLP_ENDPOINT="$2"
LISTEN_ADDR="${3:-}"
MACHINE_ID="${4:-}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

if [[ -z "${LISTEN_ADDR}" ]]; then
  case "${SERVICE}" in
    gateway) LISTEN_ADDR="0.0.0.0:7001" ;;
    inventory) LISTEN_ADDR="0.0.0.0:7002" ;;
    payment) LISTEN_ADDR="0.0.0.0:7003" ;;
    *)
      echo "unknown service: ${SERVICE}"
      exit 1
      ;;
  esac
fi

if [[ -z "${MACHINE_ID}" ]]; then
  MACHINE_ID="$(hostname)"
fi

if [[ "${SERVICE}" == "gateway" ]]; then
  : "${INVENTORY_URL:=http://127.0.0.1:7002}"
  : "${PAYMENT_URL:=http://127.0.0.1:7003}"
  export INVENTORY_URL PAYMENT_URL
fi

export OTEL_EXPORTER_OTLP_ENDPOINT="${OTLP_ENDPOINT}"
export LISTEN_ADDR
export MACHINE_ID
export RUST_LOG="info"

cargo run -p signoz-demo --bin "${SERVICE}"
