#!/usr/bin/env bash
set -u

# Allow `docker run <image> <cmd ...>` to override the keepalive loop.
if [ "$#" -gt 0 ]; then
    exec "$@"
fi

terminate() {
    exit 0
}
trap terminate TERM INT

ln -s /workspace/.claude/.claude.json /workspace/.claude.json

# Installs to ~/.local/bin/claude with versions in ~/.local/share/claude,
# so the bind mount over ~/.claude does not shadow the binary.
curl -fsSL https://claude.ai/install.sh | bash

while true; do
    claude update || echo "entrypoint: claude update failed (exit $?)" >&2
    # Backgrounded so the TERM trap fires immediately on `docker stop`.
    sleep 30m &
    wait $!
done
