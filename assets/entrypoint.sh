#!/usr/bin/env bash
set -uo pipefail

# Applies to this script and everything it starts, including the hc core and
# so every terminal (claude session) it runs. Has no effect on `docker exec`.
umask 002

# Allow `docker run <image> <cmd ...>` to override the keepalive loop.
if [ "$#" -gt 0 ]; then
    exec "$@"
fi

terminate() {
    hc server stop >/dev/null 2>&1
    exit 0
}
trap terminate TERM INT

ln -fs /workspace/.claude/.claude.json /workspace/.claude.json

# The hc core runs the claude sessions started through Herdr (`hc run --on
# dockerclaude-<space>` on the host reaches it via `docker exec`).
hc server start 2>&1 | ts || echo "entrypoint: hc server start failed" | ts >&2

while true; do
    claude update 2>&1 | ts || echo "entrypoint: claude update failed" | ts >&2
    # Backgrounded so the TERM trap fires immediately on `docker stop`.
    sleep 30m &
    wait $!
done
