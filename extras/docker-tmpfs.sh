#!/bin/bash
# Moves a docker volume, or the writable layer of a container, onto a tmpfs.
# Meant for data that may be lost: the tmpfs is mounted inside rootless
# dockerd's mount namespace (no sudo needed, and dockerd sees it), so it is
# gone after a reboot or a restart of the docker daemon. The container then
# sees the on-disk copy again, as it was when it was moved.
#
# Rootless docker only. Works with both the overlay2 graph driver and the
# containerd image store.
set -euo pipefail
shopt -s inherit_errexit   # most work happens inside $(...)

SELF="$(realpath "$0")"
PIDFILE="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/docker.pid"
CONTAINERD_SOCK="$(dirname "${PIDFILE}")/docker/containerd/containerd.sock"

function usage {
    cat >&2 <<EOF
usage: docker-tmpfs.sh volume <volume> [<size>]
       docker-tmpfs.sh container-state <container> [<size>]
       docker-tmpfs.sh umount volume|container-state <name>
       docker-tmpfs.sh ls
       docker-tmpfs.sh init

<size> is passed to tmpfs as size=, e.g. 4g or 25%. The default is half the RAM.
Any container using the volume, or the container itself, must be stopped.

For a \`docker-tmpfs\` command with bash completion, add this to ~/.bashrc:
    source <(/path/to/docker-tmpfs.sh init)
EOF
    exit 1
}

function die {
    echo "docker-tmpfs: $*" >&2
    exit 1
}

# Runs a command in rootless dockerd's user and mount namespace, where we are
# root and can mount things.
function in_docker_ns {
    [ -f "${PIDFILE}" ] || die "rootless docker doesn't seem to be running (no ${PIDFILE})"
    nsenter -U --preserve-credentials -m -t "$(cat "${PIDFILE}")" "$@"
}

function volume_dir {
    docker volume inspect -f '{{.Mountpoint}}' "$1"
}

# Prints the directory that holds both the upper and the work directory of
# container $1's writable layer. Overlayfs needs those two on the same
# filesystem, so this whole directory is what goes onto the tmpfs.
function container_state_dir {
    local id upper

    read -r id upper < <(docker inspect -f '{{.Id}} {{with .GraphDriver.Data}}{{.UpperDir}}{{end}}' "$1")
    [ "${upper}" == "<no value>" ] && upper=""

    # The containerd image store doesn't report the upper dir, but the
    # container's snapshot (keyed by the container ID) knows its mount options.
    if [ -z "${upper}" ]; then
        upper=$(in_docker_ns ctr --address "${CONTAINERD_SOCK}" -n moby \
                    snapshots --snapshotter overlayfs mounts /unused "${id}" \
                | grep -o 'upperdir=[^,]*' | cut -d= -f2) || true
    fi

    [ -n "${upper}" ] || die "can't find the writable layer of container $1"
    dirname "${upper}"
}

# Prints the directory for `volume <name>` or `container-state <name>`, after
# checking that nothing is using it.
function stopped_target_dir {
    local status

    case "$1" in
        volume)
            [ -z "$(docker ps -q --filter "volume=$2")" ] \
                || die "volume $2 is in use by a running container, stop it first"
            volume_dir "$2"
            ;;
        container-state)
            status=$(docker inspect -f '{{.State.Status}}' "$2")
            [ "${status}" == "exited" ] || [ "${status}" == "created" ] \
                || die "container $2 is ${status}, stop it first"
            container_state_dir "$2"
            ;;
        *)
            usage
            ;;
    esac
}

# Inside the namespace: copies directory $1 onto a new tmpfs (size $2, may be
# empty), using staging directory $3, and mounts that tmpfs over $1.
function ns_mount {
    local dir="$1" size="$2" stage="$3"
    local -a opts=()

    mountpoint -q "${dir}" && die "${dir} is already a mount point"

    [ -n "${size}" ] && opts=(-o "size=${size}")
    mount -t tmpfs "${opts[@]}" tmpfs "${stage}"
    chown --reference="${dir}" "${stage}"
    chmod --reference="${dir}" "${stage}"

    # -a keeps ownership, xattrs (user.overlay.*) and whiteouts (0/0 char devices).
    if ! cp -a "${dir}/." "${stage}/"; then
        umount "${stage}"
        die "copying ${dir} failed, nothing was changed"
    fi

    mount --move "${stage}" "${dir}"
}

function do_mount {
    local kind="$1" name="$2" size="${3:-}" dir stage

    dir=$(stopped_target_dir "${kind}" "${name}")
    stage=$(mktemp -d "$(docker info -f '{{.DockerRootDir}}')/tmp/docker-tmpfs.XXXXXX")

    in_docker_ns "${SELF}" __ns_mount "${dir}" "${size}" "${stage}" || { rmdir "${stage}"; exit 1; }
    rmdir "${stage}"

    echo "${kind} ${name} is now on a tmpfs (${dir})"
}

function do_umount {
    local kind="$1" name="$2" dir

    dir=$(stopped_target_dir "${kind}" "${name}")
    in_docker_ns mountpoint -q "${dir}" || die "${kind} ${name} is not on a tmpfs"
    in_docker_ns umount "${dir}"

    echo "${kind} ${name} is back on disk, as it was when it was moved to the tmpfs"
}

function do_ls {
    local dataRoot c dir size used target name
    local -A names=()

    dataRoot=$(docker info -f '{{.DockerRootDir}}')

    for c in $(docker ps -aq); do
        dir=$(container_state_dir "${c}" 2>/dev/null) || continue
        names["${dir}"]="container-state $(docker inspect -f '{{.Name}}' "${c}" | cut -c2-)"
    done

    printf '%-8s %-8s %s\n' SIZE USED NAME
    while read -r target size used; do
        printf -v target '%b' "${target}"   # findmnt -r escapes e.g. spaces as \x20

        if [[ "${target}" == "${dataRoot}/volumes/"*/_data ]]; then
            name="volume $(basename "$(dirname "${target}")")"
        else
            name="${names[${target}]:-${target}}"
        fi
        printf '%-8s %-8s %s\n' "${size}" "${used}" "${name}"
    done < <(in_docker_ns findmnt -rn -t tmpfs -o TARGET,SIZE,USED \
                | grep -F "${dataRoot}/" | grep -vF "${dataRoot}/tmp/")
}

# Prints shell code that defines a `docker-tmpfs` function running this
# script by its full path, and bash completion for it and docker-tmpfs.sh.
function print_init {
    printf '_docker_tmpfs_self=%q\n' "${SELF}"
    cat <<'EOF'
docker-tmpfs() {
    "${_docker_tmpfs_self}" "$@"
}

_docker_tmpfs() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local -a words=()

    case "${COMP_CWORD}" in
        1)
            words=(volume container-state umount ls)
            ;;
        2)
            case "${COMP_WORDS[1]}" in
                volume)
                    mapfile -t words < <(docker volume ls -q 2>/dev/null)
                    ;;
                container-state)
                    mapfile -t words < <(docker ps -a --filter status=exited \
                        --filter status=created --format '{{.Names}}' 2>/dev/null)
                    ;;
                umount)
                    words=(volume container-state)
                    ;;
            esac
            ;;
        3)
            # Only what is actually on a tmpfs, going by `ls`.
            if [ "${COMP_WORDS[1]}" == "umount" ]; then
                mapfile -t words < <("${_docker_tmpfs_self}" ls 2>/dev/null \
                    | awk -v kind="${COMP_WORDS[2]}" '$3 == kind { print $4 }')
            fi
            ;;
    esac

    mapfile -t COMPREPLY < <(compgen -W "${words[*]}" -- "${cur}")
}
complete -F _docker_tmpfs docker-tmpfs.sh docker-tmpfs
EOF
}

case "${1:-}" in
    volume|container-state)
        [ $# -ge 2 ] && [ $# -le 3 ] || usage
        do_mount "$@"
        ;;
    umount)
        [ $# -eq 3 ] || usage
        do_umount "$2" "$3"
        ;;
    ls)
        do_ls
        ;;
    init)
        print_init
        ;;
    __ns_mount)
        shift
        ns_mount "$@"
        ;;
    *)
        usage
        ;;
esac
