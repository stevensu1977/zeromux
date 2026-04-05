#!/bin/bash
cd "$(dirname "$0")"

BINARY="./target/release/zeromux"
LOG="zeromux.log"
PID_FILE="zeromux.pid"

# Stop existing instance
if [ -f "$PID_FILE" ]; then
    OLD_PID=$(cat "$PID_FILE")
    if kill -0 "$OLD_PID" 2>/dev/null; then
        echo "Stopping existing zeromux (PID $OLD_PID)..."
        kill "$OLD_PID"
        sleep 1
    fi
    rm -f "$PID_FILE"
fi

if [ ! -f "$BINARY" ]; then
    echo "Binary not found. Building..."
    cd frontend && npm run build && cd ..
    cargo build --release || exit 1
fi

nohup "$BINARY" "$@" > "$LOG" 2>&1 &
echo $! > "$PID_FILE"

sleep 0.5
if kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    echo "ZeroMux started (PID $(cat "$PID_FILE")), log: $LOG"
    head -5 "$LOG"
else
    echo "Failed to start. Check $LOG"
    tail -20 "$LOG"
    exit 1
fi
