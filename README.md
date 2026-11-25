# dockerclaude

Claude Code in Docker containers, one per *space*, with host directories
mounted in on demand. It integrates with [Herdr](https://herdr.dev): run from
a Herdr pane, claude opens as a pane in Herdr, and agents it starts with
`hc run` show up there too.

```sh
cargo install --path .
echo 'source <(dockerclaude bash-completion)' >> ~/.bashrc

cd ~/src/my-project
dockerclaude            # build image, start container, mount ., run claude
```

## Requirements

- Linux ≥ 5.2 (≥ 5.12 for `mount --ro`), **rootless Docker**
- `nsenter` (util-linux) for `--tmpfs`, `zstd` for snapshots
- Optional: Herdr (`herdr-app`) and herdr-core (`hc`) on the host

## Spaces

A space is a container `dockerclaude-<space>` (and, in Herdr, an hc machine of
the same name). `-s/--space` picks one. Without it, dockerclaude uses the name
of the Herdr space you are in, if you named it, and otherwise `default`.
Space names are 1–19 of `a-z 0-9 - _ .`. Herdr names are converted, e.g.
`My Project` → `my-project`.

To name a Herdr space so that it gets its own container:
`herdr-app do space.rename '{"id":"<space id>","name":"<name>"}'`. When you
start a session in an unnamed space, dockerclaude asks first and prints that
command with the right id.

## The session command

`dockerclaude [-s SPACE] [-y] [--streaming-json]`:

1. builds `dockerclaude:latest` if there is no container and no image;
2. creates/starts the container if it isn't running;
3. mounts the current directory unless it is already visible;
4. runs claude in `/mnt/<absolute path of .>` in the container.

In a Herdr pane, step 4 instead adds the container as an hc machine (once,
`hc machine add --via 'docker exec -i dockerclaude-<space> sh -c'`). It then
starts claude with `hc run --on dockerclaude-<space>` in a new tab of a space
on that machine. The space is named like the dockerclaude space and shown in
Herdr's sidebar under the machine. Herdr spaces belong to one machine, which
is why claude can't open as a pane in the host's space. Agents that claude
starts with `hc run` appear under it, and as panes beside it if started with
its `herdr.space`/`herdr.tab` labels.

To show the container's space and the Herdr space you started from as one
(one sidebar row, both spaces' tabs in one tab bar), install the layout in
[`extras/herdr-joined`](extras/herdr-joined/README.md).

Every step asks for confirmation unless you pass `-y`. `--streaming-json`
always uses a plain `docker exec -i`, even in Herdr.

## Subcommands

| command | what it does |
|---|---|
| `build-image [-t TAG]` | builds `~/.config/dockerclaude/Dockerfile[.TAG]` as `dockerclaude:TAG`, then removes the image it replaced (unless containers use it). Writes the default Dockerfile, entrypoint.sh and installers/rust.sh if they are missing. Claude Code and hc are always downloaded fresh. |
| `start-container [-s] [-t] [-d DOT_CLAUDE] [--tmpfs SIZE]` | creates and starts the container, with `DOT_CLAUDE` (default `~/.config/dockerclaude/dot-claude`) at `/workspace/.claude`, on the space's network `dockerclaude-<space>` (created if missing). If it is already running with the same settings, nothing happens; different settings are an error. A stopped container is just started; only `-s/--tmpfs/-y` are allowed then. |
| `stop-container` / `rm-container` | stops (keeps) / removes the container. `rm-container` also removes its tmpfs, its Herdr machine and its network, unless other containers use the network. |
| `install NAME...` | runs `~/.config/dockerclaude/installers/NAME.sh` in the container. Names without an installer are passed to `apt install`. |
| `exec COMMAND...` | runs a command in the container (umask 002), in the current directory if it is mounted. |
| `mount [--ro] PATH` / `umount PATH` | mounts a host directory into the *running* container at `/mnt/<PATH>`, or unmounts it (lazily, then removes the empty mount point directories). |
| `ps [-s] [-a]` | state, image, dot-claude, tmpfs, Herdr machine, snapshot, mounts, last log lines and last prompt. |
| `snapshot` / `snapshot-delete` / `restore [--tmpfs SIZE]` | saves the container to `~/.config/dockerclaude/snapshots/<space>.tar.zst`, deletes that snapshot (file and image), or recreates the container from it, with the dot-claude it had. |
| `dot-claude-clean [-d]` | archives the session history to `<parent>/session-history/` and wipes the dot-claude directory except credentials, config and skills. |
| `dir` | prints `~/.config/dockerclaude`. |
| `bash-completion` | prints the completion script. |

## How it works

- **Mounts** use the new mount API (`open_tree` + `move_mount`), from inside
  the container's user and mount namespaces (the former `extras/ctr-bind`).
  A mount exists only in the container and vanishes when it stops. The
  container's mount table is the only state; there is no database. The
  namespace work runs in a re-executed helper process
  (`dockerclaude __ctr-bind`).
- **`--tmpfs`** copies the container's writable layer onto a tmpfs mounted in
  rootless dockerd's mount namespace (the former `extras/docker-tmpfs.sh`). It
  is gone after a reboot or a docker restart, and the container falls back to
  the on-disk copy from the time of the move.
- **Networks**: each space's container is on its own user-defined bridge
  network, `dockerclaude-<space>`, like a compose project's default network.
  Containers on such networks use Docker's embedded DNS server, which can also
  reach resolvers that only the host can reach: private DNS from a VPN client
  such as Cloudflare WARP, or systemd-resolved's stub. On the default bridge,
  Docker only copies the host's resolv.conf, drops loopback resolvers like
  these, and falls back to public DNS. Containers created before
  dockerclaude used networks stay on the default bridge (`ps` shows it) until
  they are recreated.
- **Container settings** are kept as labels (`dockerclaude.space`,
  `dockerclaude.dot-claude`). `docker commit` copies the labels, so `restore`
  knows the dot-claude path.
- The **entrypoint** starts an hc core in the container (umask 002, which
  claude sessions started through Herdr inherit) and runs `claude update`
  every 30 minutes.
