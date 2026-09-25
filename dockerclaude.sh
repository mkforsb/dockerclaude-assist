#!/bin/bash
set -euo pipefail

DC2DIR="$(dirname "$(realpath "$0")")"

function perform_install {
    if [ -f "$DC2DIR/installers/${1}.sh" ]; then
        docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
            bash -c "/installers/${1}.sh"
    else
        docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
            bash -c "apt update && apt install -y $*"
    fi
}

function start_session {
    DC2MOUNTSDIR="$DC2DIR/mounts"
    DC2MOUNTSDB="$DC2DIR/mounts.sqlite3"

    sqlite3 "${DC2MOUNTSDB}" " \
        create table if not exists mounts (slug text primary key, reference_count int); \
    "

    targetDir=$(realpath "${1:-.}")
    slug=$(echo "${targetDir}" | sed -E 's/[^A-Za-z0-9]/-/g')
    mountPoint="$DC2MOUNTSDIR/${slug}"

    cleanup() {
        sqlite3 "${DC2MOUNTSDB}" " \
            update mounts set reference_count = reference_count - 1 \
            where slug = '${slug}'; \
            \
            delete from mounts where reference_count <= 0; \
        "

        refCount=$(sqlite3 "${DC2MOUNTSDB}" " \
            select reference_count from mounts \
            where slug = '${slug}'; \
        ")

        if [ "${refCount}" == "" ] && mountpoint -q "${mountPoint}"; then
            # Only remove the directory once the unmount has actually succeeded;
            # otherwise the rm would reach thr@ough the bind mount into the real
            # project directory.
            if sudo umount "${mountPoint}"; then
                rmdir "${mountPoint}"
            else
                echo "dockerclaude: failed to unmount ${mountPoint}, leaving it in place" >&2
            fi
        fi
    }

    if ! mountpoint -q "${mountPoint}"; then
        mkdir -p "${mountPoint}"
        sudo mount --bind "${targetDir}" "${mountPoint}"
    fi

    # From here on, always release the mount/refcount on exit, including when
    # the session is interrupted or the exec fails.
    trap cleanup EXIT

    sqlite3 "${DC2MOUNTSDB}" " \
        insert into mounts (slug, reference_count)  \
        values ('${slug}', 1) \
        on conflict(slug) do update set reference_count = reference_count + 1; \
    "

    docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
        bash -c "cd /mnt/${slug} && claude"
}

if [ "${1:-}" == "install" ]; then
    shift
    perform_install "$@"
elif [ "${1:-}" == "exec" ]; then
    shift
    docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude "$@"
else
    start_session "$@"
fi
