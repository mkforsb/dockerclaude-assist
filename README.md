# dockerclaude

Run [Claude Code](https://claude.com/claude-code) inside a long-lived Docker
container while working on project directories from your host.

A single `dockerclaude` container runs in the background and keeps Claude Code
installed and up to date. When you start a session, `dockerclaude.sh`
bind-mounts the chosen project directory into the container and launches
`claude` there. Claude can only see the projects you have explicitly opened,
not the rest of your filesystem.

## How it works

- The container (`debian:testing`) uses `/workspace` as `$HOME`. On startup it
  installs Claude Code and then runs `claude update` every 30 minutes.
- `./.claude` on the host is bind-mounted to `~/.claude` in the container, so
  credentials, settings and history persist across container restarts.
- `./mounts` on the host is mounted at `/mnt` in the container with mount
  propagation. Mounting a project under `./mounts/<slug>` on the host makes it
  appear at `/mnt/<slug>` inside the already-running container, with no
  restart needed.
- The slug is the project's absolute path with every non-alphanumeric
  character replaced by `-` (e.g. `/home/me/src/foo` → `-home-me-src-foo`).
- Mounts are reference-counted in `mounts.sqlite3`, so several sessions can
  share a project. The bind mount is removed when the last session for that
  project exits.

## Requirements

- Linux host (uses bind mounts and mount propagation)
- Docker with the Compose plugin (`docker compose`)
- `sudo` (for `mount`/`umount`)
- `sqlite3`
- `mountpoint` (from `util-linux`)

## Setup

1. Build the image:

   ```sh
   make build
   ```

2. Make `./mounts` a shared mount point. This must happen **before** the
   container starts, and must be redone after every host reboot:

   ```sh
   make setup-mounts
   ```

3. Start the container:

   ```sh
   make start-container
   ```

4. Log in. Either run `./dockerclaude.sh` and go through the login flow, or
   [copy your auth from the host](#how-to-copy-auth-from-host-machine).

5. Optionally, put `dockerclaude.sh` on your `PATH`, e.g.:

   ```sh
   ln -s "$PWD/dockerclaude.sh" ~/.local/bin/dockerclaude
   ```

## Usage

Start a Claude Code session in a project directory (defaults to the current
directory):

```sh
dockerclaude.sh [DIR]
```

Install extra tooling in the container. If `installers/<NAME>.sh` exists it
is run; otherwise the arguments are passed to `apt install`:

```sh
dockerclaude.sh install rust            # runs installers/rust.sh
dockerclaude.sh install ripgrep jq      # apt install -y ripgrep jq
```

Run an arbitrary command in the container:

```sh
dockerclaude.sh exec bash
```

Mount a directory into the container without starting a session, e.g. to give
an already-running session access to another project. This takes a reference
on the mount just like a session does, so it stays mounted until it is
explicitly unmounted (and no session is using it). The container name
defaults to `dockerclaude`:

```sh
dockerclaude.sh mount [CONTAINER] DIR
# /home/me/src/bar -> dockerclaude:/mnt/-home-me-src-bar (refcount 1)
dockerclaude.sh umount [CONTAINER] DIR
```

List mounted projects with their reference counts, and report broken mounts
(e.g. a mount left behind by a crashed session, or a database entry whose
mount is gone). It also checks that `./mounts` is set up as a shared mount:

```sh
dockerclaude.sh ps
# /home/me/src/foo -> /mnt/-home-me-src-foo (refcount 2)
```

Wipe `./.claude`, keeping only credentials, `.claude.json`, `settings.json`
and `skills`. Session history (`history.jsonl`, `projects`, `sessions`,
`file-history`, `plans`, `session-env`, `shell-snapshots`) is first archived
to `./session-history/<timestamp>.tar.gz`:

```sh
dockerclaude.sh dot-claude-clean
```

Stop the container:

```sh
make stop-container
```

Note that `make stop-container` removes the container. Anything installed
with `dockerclaude.sh install` is lost and must be reinstalled after the next
`make start-container`. Only `./.claude` and your mounted projects persist.
To make a tool available permanently, add it to the `Dockerfile` or add a
script to `installers/`.

## How to copy auth from host machine

1. Copy `~/.claude/.credentials.json` to the dockerclaude `.claude` folder.
2. Copy keys from `~/.claude.json` to dockerclaude `.claude/.claude.json`:
  - `"hasCompletedOnboarding": true`
  - `"oauthAccount": { ... }`
