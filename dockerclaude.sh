#!/bin/bash
set -euo pipefail

DC2DIR="$(dirname "$(realpath "$0")")"
DC2MOUNTSDIR="$DC2DIR/mounts"
DC2MOUNTSDB="$DC2DIR/mounts.sqlite3"

function slugify {
    echo "$1" | sed -E 's/[^A-Za-z0-9]/-/g'
}

function bound_source {
    local boundSourceDevice boundFsRoot rootMountPoint hostPath

    read -r boundSourceDevice boundFsRoot < <(findmnt -rn -o MAJ:MIN,FSROOT --mountpoint "$1")
    read -r rootMountPoint < <(findmnt -rn -o MAJ:MIN,FSROOT,TARGET \
        | awk -v dev="${boundSourceDevice}" '$1 == dev && $2 == "/" { print $3; exit }')

    # findmnt -r escapes special characters, e.g. a space becomes \x20
    printf -v hostPath '%b' "${rootMountPoint%/}${boundFsRoot}"

    [ "$(slugify "${hostPath}")" == "$2" ] && echo "${hostPath}"
}

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
    sqlite3 "${DC2MOUNTSDB}" " \
        create table if not exists mounts (slug text primary key, reference_count int); \
    "

    targetDir=$(realpath "${1:-.}")
    slug=$(slugify "${targetDir}")
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
            # otherwise the rm would reach through the bind mount into the real
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
elif [ "${1:-}" == "mounts" ]; then
    # ./mounts must be a shared mount point (see `make setup-mounts`), otherwise
    # project mounts will not propagate into the running container.
    if ! mountpoint -q "${DC2MOUNTSDIR}" || \
        [[ "$(findmnt -n -o PROPAGATION --mountpoint "${DC2MOUNTSDIR}")" != *shared* ]]; then
        echo "${DC2MOUNTSDIR} is not mounted properly! (run 'make setup-mounts')" >&2
        exit 1
    fi

    mapfile -t mountsOnDisk < <(
        find "${DC2MOUNTSDIR}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort
    )

    # A missing or unreadable database is treated as empty.
    mapfile -t mountsRefCount < <(
        sqlite3 -readonly "${DC2MOUNTSDB}" \
            "select slug, reference_count from mounts order by slug;" 2>/dev/null
    )

    if [ "${#mountsOnDisk[@]}" -eq 0 ] && [ "${#mountsRefCount[@]}" -eq 0 ]; then
        echo 'No mounts'
        exit 0
    fi

    declare -A onDisk=() inDb=()
    for slug in "${mountsOnDisk[@]}"; do
        onDisk["${slug}"]=1
    done

    for row in "${mountsRefCount[@]}"; do
        IFS='|' read -r slug refcount <<< "${row}"
        inDb["${slug}"]=1

        if [ -z "${onDisk[${slug}]:-}" ]; then
            echo "Broken mount! ${slug} with refcount ${refcount} is not present in filesystem!"
        elif ! mountpoint -q "${DC2MOUNTSDIR}/${slug}"; then
            echo "Broken mount! ${slug} with refcount ${refcount} exists but is not mounted!"
        else
            srcDir=$(bound_source "${DC2MOUNTSDIR}/${slug}" "${slug}") || srcDir="${slug}"
            echo "${srcDir} -> /mnt/${slug} (refcount ${refcount})"
        fi
    done

    for slug in "${mountsOnDisk[@]}"; do
        if [ -z "${inDb[${slug}]:-}" ]; then
            echo "Broken mount! ${slug} not present in refcount database!"
        fi
    done
else
    start_session "$@"
fi
