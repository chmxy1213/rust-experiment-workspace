#!/usr/bin/env bash
set -euo pipefail

# Stop locally started demo binaries.
pkill -f "target/.*/gateway" || true
pkill -f "target/.*/inventory" || true
pkill -f "target/.*/payment" || true

echo "Stopped gateway/inventory/payment if they were running."
