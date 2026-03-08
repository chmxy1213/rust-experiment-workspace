#!/usr/bin/env bash
set -euo pipefail

# One-click start for SigNoz control plane on the observability node.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SIGNOZ_DIR="${ROOT_DIR}/.signoz"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required"
  exit 1
fi

if ! docker compose version >/dev/null 2>&1; then
  echo "docker compose is required"
  exit 1
fi

if [[ ! -d "${SIGNOZ_DIR}" ]]; then
  git clone https://github.com/SigNoz/signoz.git "${SIGNOZ_DIR}"
fi

cd "${SIGNOZ_DIR}/deploy"
./install.sh

echo "SigNoz is starting. Open: http://127.0.0.1:3301"
echo "OTLP gRPC endpoint for remote apps: http://<observability-node-ip>:4317"
