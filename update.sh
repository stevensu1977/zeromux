#!/bin/bash
# Rebuild ZeroMux from the current working tree and restart the systemd service.
# Usage: ./update.sh   (run after pulling/editing code)
set -e
cd "$(dirname "$0")"

echo "==> Building frontend..."
(cd frontend && npm run build)

echo "==> Building release binary..."
cargo build --release

echo "==> Restarting systemd service..."
sudo systemctl restart zeromux.service
sleep 1
sudo systemctl status zeromux.service --no-pager | head -6

echo "==> Done. Logs: journalctl -u zeromux -f"
