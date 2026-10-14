# Idea: keep dockerclaude's writes in a volume instead of the writable layer

## Background

`¤docker-ramdisk` puts the whole docker data root (`~/.local/share/docker`) on a 20G
tmpfs. With Docker 29's containerd image store, ~17G of that is
`containerd/daemon/io.containerd.snapshotter.v1.overlayfs/`, where **image layers and
container writable layers live side by side** as numbered snapshot directories. They
can't be split by directory, so getting images and build cache off the ramdisk means
writable layers leave it too. Only `volumes/` (and `containers/`) can stay on the
tmpfs.

Writable layer sizes at the time (`docker ps -s`):

| container           | writable layer |
|---------------------|----------------|
| dockerclaude        | 8.5 GB         |
| node-cli-container  | 659 MB         |
| php-container-cli   | 66 MB          |
| everything else     | ~0             |

mysql and rabbitmq already write to volumes. The one container that writes a lot to
its writable layer is dockerclaude, because sandboxed Claude installs and builds things
all over the filesystem.

## The idea

Docker has no "overlay volume", but the container can build one itself. Before the
real entrypoint, `overlay-root.sh`:

1. mounts an overlayfs with the container's own `/` as the lower layer and a directory
   in a volume mounted at `/overlay` as the upper layer,
2. re-attaches everything that is mounted on top of `/` (`/proc`, `/dev`, `/etc/hosts`,
   `/mnt`, `/workspace/.claude`, ...) inside the new overlay with `mount --rbind`,
3. `mount --move`s the overlay onto `/` and `chroot`s into it.

From then on every write anywhere (`/usr`, `/etc`, `/workspace`, `/opt`, ...) lands in
the volume, so it lands on the ramdisk once `¤docker-ramdisk` only puts `volumes/` on
tmpfs. Sandboxed Claude doesn't need to know about it.

## Verified (2026-10-14, rootless Docker 29.9.0, kernel 6.8, `dockerclaude:latest`)

Tested in a throwaway container with `--cap-add SYS_ADMIN --security-opt no-new-privileges`
and a volume on the tmpfs:

- the overlay mounts inside the rootless user namespace. It needs `userxattr`, and
  tmpfs supports `user.*` xattrs since kernel 6.6, so an upper layer on tmpfs works.
- writes (a 50 MB file in `/usr/local`, a file in `/etc`, `apt-get install`) all end
  up in the volume's `upper/`. `docker ps -s` stays at **0B** for the container.
- **`docker exec` lands in the overlay root** (with `/workspace` as its working
  directory), because a process joining the mount namespace gets the topmost mount on
  `/`. So `dockerclaude.sh exec` and sessions work unchanged.
- **`/mnt` propagation survives.** A bind of a slave mount is a slave of the same master,
  so project mounts added later by `dockerclaude.sh` still show up. This was simulated
  inside the container (shared tmpfs → slave bind on `/mnt` → re-root → new submount),
  since it couldn't be tested against the host's shared mount without sudo.
- `docker restart`: the overlay is set up again on the same volume and the contents are
  still there.
- without a volume on `/overlay`, the script prints a warning and runs the entrypoint
  as usual.

## overlay-root.sh

```bash
#!/usr/bin/env bash
# Re-roots the container onto an overlayfs that has the container's own root as
# its lower layer and the volume mounted at /overlay as its upper layer, so all
# writes end up in the volume instead of the container's writable layer.
# Needs CAP_SYS_ADMIN.
set -euo pipefail

vol=/overlay
newRoot="${vol}/root"
workDir="${PWD}"

if ! mountpoint -q "${vol}"; then
    echo "overlay-root: ${vol} is not a mount point, not re-rooting" >&2
    exec "$@"
fi

mkdir -p "${vol}/upper" "${vol}/work" "${newRoot}"
mount -t overlay overlay \
    -o "lowerdir=/,upperdir=${vol}/upper,workdir=${vol}/work,userxattr" "${newRoot}"

# The overlay only sees the root mount itself, so carry over everything that is
# mounted on top of it (/proc, /dev, /etc/hosts, bind mounts, ...).
handled=()
while read -r target; do
    printf -v target '%b' "${target}"   # findmnt -r escapes e.g. spaces as \x20

    [ "${target}" == "/" ] && continue
    [ "${target}" == "${vol}" ] || [[ "${target}" == "${vol}/"* ]] && continue

    for h in "${handled[@]}"; do
        [[ "${target}" == "${h}/"* ]] && continue 2
    done

    mount --rbind "${target}" "${newRoot}${target}"
    handled+=("${target}")
done < <(findmnt -rn -o TARGET | sort)

cd "${newRoot}"
mount --move . /
exec /usr/sbin/chroot . /usr/bin/env -C "${workDir}" "$@"
```

`chroot` is called by full path because `/usr/sbin` isn't on the image's `PATH`.
`pivot_root` isn't in the image, which is why the script uses `mount --move` + `chroot`.

## Wiring it up (not done yet)

Dockerfile:

```dockerfile
COPY overlay-root.sh /overlay-root.sh
RUN chmod 0755 /overlay-root.sh
ENTRYPOINT ["/overlay-root.sh", "/entrypoint.sh"]
```

`entrypoint.sh` still gets no arguments when nothing is passed, so its
`docker run <image> <cmd ...>` override keeps working.

docker-compose.yml:

```yaml
services:
  dockerclaude:
    cap_add: [SYS_ADMIN]
    volumes:
      - type: volume
        source: dockerclaude-root
        target: /overlay
      # ... existing binds
volumes:
  dockerclaude-root:
```

`¤docker-ramdisk` would then only put `volumes/` on the tmpfs, using `sudo cp -a` /
`sudo rsync -aHAX` so that subuid ownership, `user.overlay.*` xattrs and whiteouts
(char 0/0 devices) survive the round trip.

## `dockerclaude.sh cp`: replacing `docker cp`

`docker cp` works on the container's rootfs from the daemon's side, outside the
container's mount namespace. It would read from and write to the layer *underneath* the
overlay: copied-in files would be invisible, and copied-out files would be stale or
missing. This can't be fixed inside `docker cp` itself.

Piping `tar` through `exec` does work, since exec'd processes see the overlay root.
This was tested against an overlay'd throwaway container: files and directories in
both directions, into an existing directory and to a new name, `src/.` (contents
only), relative container paths, symlinks, and names with spaces, `|` and `&`. Copied-in
files end up owned by container root (lur on the host), like with `docker cp`, and
everything landed in the volume.

```bash
function container_exec {
    docker compose -f "${DC2DIR}/docker-compose.yml" exec -T -w / dockerclaude "$@"
}

# Runs a command on the host or, with $1 == container, in the container.
function run_on {
    local side="$1"
    shift

    if [ "${side}" == "container" ]; then
        container_exec "$@"
    else
        "$@"
    fi
}

# Copies $1 to $2 between the host and the container, like `docker cp` but
# through `exec`, so it reads and writes the filesystem that the container's
# processes actually see. The container side is prefixed with `dockerclaude:`
# or just `:`, and relative container paths are resolved from /.
function perform_cp {
    local src="$1" dst="$2" srcSide=host dstSide=host dstDir name

    if [[ "${src}" =~ ^(dockerclaude)?:(.*)$ ]]; then
        srcSide=container
        src="${BASH_REMATCH[2]}"
    fi
    if [[ "${dst}" =~ ^(dockerclaude)?:(.*)$ ]]; then
        dstSide=container
        dst="${BASH_REMATCH[2]}"
    fi

    if [ "${srcSide}" == "${dstSide}" ]; then
        echo "usage: dockerclaude.sh cp <src> <dst>, with exactly one of them prefixed by \`dockerclaude:\` or \`:\`" >&2
        exit 1
    fi

    # Like `docker cp`: into $dst if it is an existing directory, otherwise as $dst.
    if run_on "${dstSide}" test -d "${dst}"; then
        dstDir="${dst}"
        name=$(basename "${src}")
    else
        dstDir=$(dirname "${dst}")
        name=$(basename "${dst}")
    fi

    # Renames the top-level entry to $name; escape what is special in a sed replacement.
    name=$(printf '%s' "${name}" | sed 's/[\\&|]/\\&/g')

    run_on "${srcSide}" tar -C "$(dirname "${src}")" -cf - \
            --transform "s|^[^/]*|${name}|S" -- "$(basename "${src}")" \
        | run_on "${dstSide}" tar -C "${dstDir}" -xf - --no-same-owner
}
```

```bash
elif [ "${1:-}" == "cp" ]; then
    shift
    perform_cp "$@"
```

The `S` flag on `--transform` keeps symlink targets from being renamed. `-w /`
resolves relative container paths from `/`, like `docker cp` does. This also works
without the overlay, so it could be added on its own.

## Trade-offs and caveats

- **`CAP_SYS_ADMIN`**: sandboxed Claude can mount things inside the container. Under
  rootless Docker the capability only applies inside the rootlesskit user namespace,
  not on the host, but the sandbox is still less locked down than before. The capability
  can't be dropped for `docker exec` sessions. Those get the container's configured caps,
  unless the exec'd command is wrapped in `setpriv --bounding-set=-sys_admin ...`.
- **Rebuilding the image**: the volume's upper layer stays on top of the new lower
  layer. Overlayfs allows lower-layer changes while it's unmounted (index/metacopy/
  redirect_dir aren't in use), but files in the upper layer, e.g. dpkg's database, hide
  the image's newer versions. Wipe the volume when rebuilding.
- **`docker cp` / `docker diff` / `docker export`** see the layer underneath the overlay.
  Use `dockerclaude.sh cp` instead.
- **Depth**: the container root is already an overlay, so this overlay sits one level
  deeper. Sandboxed Claude can't stack another overlayfs on top of `/` (the kernel's
  depth limit is 2), which only matters for things like docker-in-docker.

## Switch-over (if going ahead)

1. Turn the old-style ramdisk off by hand. The old `off` is broken: after `umount`,
   `docker/` still exists, so `mv _docker docker` moves the backup to
   `docker/_docker`. Stop docker, `sudo rsync -aHAX --delete` the tmpfs back into
   `_docker` (to keep today's state rather than the morning's backup), `umount`,
   `rmdir docker`, `mv _docker docker`.
2. Rebuild and recreate dockerclaude. Recreating discards its current 8.5 GB writable
   layer. Alternatively that snapshot's `fs/` directory could be copied (`rsync -aHAX`)
   into the new volume as `upper/`, since it uses the same overlay format.
3. `¤docker-ramdisk on` with the volumes-only version.
