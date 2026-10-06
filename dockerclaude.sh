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

function init_mounts_db {
    sqlite3 "${DC2MOUNTSDB}" " \
        create table if not exists mounts (slug text primary key, reference_count int); \
    "
}

# Prints the reference count for slug $1, or nothing if it has no references.
function mount_refcount {
    sqlite3 "${DC2MOUNTSDB}" " \
        select reference_count from mounts \
        where slug = '${1}'; \
    "
}

# Bind-mounts the absolute host directory $1 under ./mounts (unless it is
# already mounted) and takes a reference on it.
function acquire_mount {
    local slug mountPoint
    slug=$(slugify "$1")
    mountPoint="$DC2MOUNTSDIR/${slug}"

    init_mounts_db

    if ! mountpoint -q "${mountPoint}"; then
        mkdir -p "${mountPoint}"
        sudo mount --bind "$1" "${mountPoint}"
    fi

    sqlite3 "${DC2MOUNTSDB}" " \
        insert into mounts (slug, reference_count)  \
        values ('${slug}', 1) \
        on conflict(slug) do update set reference_count = reference_count + 1; \
    "
}

# Drops a reference on the absolute host directory $1 and unmounts it once no
# references remain.
function release_mount {
    local slug mountPoint
    slug=$(slugify "$1")
    mountPoint="$DC2MOUNTSDIR/${slug}"

    init_mounts_db

    sqlite3 "${DC2MOUNTSDB}" " \
        update mounts set reference_count = reference_count - 1 \
        where slug = '${slug}'; \
        \
        delete from mounts where reference_count <= 0; \
    "

    if [ "$(mount_refcount "${slug}")" == "" ] && mountpoint -q "${mountPoint}"; then
        # Only remove the directory once the unmount has actually succeeded;
        # otherwise the rm would reach through the bind mount into the real
        # project directory.
        if sudo umount "${mountPoint}"; then
            rmdir "${mountPoint}"
        else
            echo "dockerclaude: failed to unmount ${mountPoint}, leaving it in place" >&2
            return 1
        fi
    fi
}

# Parses `[<container-name>] <path>` for subcommand $1 into the globals
# container and targetPath.
function parse_mount_args {
    local cmd="$1"
    shift

    case $# in
        1) container=dockerclaude; targetPath="$1" ;;
        2) container="$1"; targetPath="$2" ;;
        *)
            if [ ! "$cmd" == "" ]; then
                echo "usage: dockerclaude.sh ${cmd} [<container-name>] <path>" >&2
            else
                echo "usage: dockerclaude.sh [<container-name>] <path>" >&2
            fi
            exit 1
            ;;
    esac

    if [ ! -d "${targetPath}" ]; then
        echo "Invalid path \`${targetPath}\`" >&2
        exit 1
    fi
}

function start_session {
    local targetDir
    targetDir=$(realpath "${1:-.}")

    acquire_mount "${targetDir}"

    # From here on, always release the mount/refcount on exit, including when
    # the session is interrupted or the exec fails.
    trap "release_mount $(printf '%q' "${targetDir}")" EXIT

    docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
        bash -c "cd /mnt/$(slugify "${targetDir}") && claude"
}

# Wipes ./.claude except for credentials, config and skills, after stashing a
# compressed copy of the session history in ./session-history.
function dot_claude_clean {
    local claudeDir="${DC2DIR}/.claude"
    local historyDir="${DC2DIR}/session-history"
    local ans f archive
    local -a toArchive=()

    read -r -p "Really wipe ${claudeDir}? Y/n? " -n 1 ans </dev/tty
    echo
    [ "${ans}" == "Y" ] || return 0

    for f in file-history history.jsonl plans projects session-env sessions shell-snapshots; do
        if [ -e "${claudeDir}/${f}" ]; then
            toArchive+=(".claude/${f}")
        fi
    done

    # set -e aborts before anything is removed if the archive can't be written.
    if [ "${#toArchive[@]}" -gt 0 ]; then
        mkdir -p "${historyDir}"
        archive="${historyDir}/$(date +%Y%m%d-%H%M%S).tar.gz"
        tar -C "${DC2DIR}" -czf "${archive}" "${toArchive[@]}"
        echo "Saved session history to ${archive}"
    fi

    find "${claudeDir}" -mindepth 1 -maxdepth 1 \
        ! -name ".credentials.json" \
        ! -name ".claude.json" \
        ! -name "settings.json" \
        ! -name "skills" \
        ! -name ".gitkeep" \
        -exec rm -vrf {} +
}

if [ "${1:-}" == "install" ]; then
    shift
    perform_install "$@"
elif [ "${1:-}" == "exec" ]; then
    shift
    docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude "$@"
elif [ "${1:-}" == "make" ]; then
    shift
    cd "${DC2DIR}" && make "$@"
elif [ "${1:-}" == "dir" ]; then
    shift
    echo "${DC2DIR}"
elif [ "${1:-}" == "dot-claude-clean" ]; then
    dot_claude_clean
elif [ "${1:-}" == "mount" ]; then
    shift
    parse_mount_args mount "$@"

    targetDir=$(realpath "${targetPath}")
    slug=$(slugify "${targetDir}")
    acquire_mount "${targetDir}"
    echo "${targetDir} -> ${container}:/mnt/${slug} (refcount $(mount_refcount "${slug}"))"
elif [ "${1:-}" == "umount" ]; then
    shift
    parse_mount_args umount "$@"

    # -m: allow unmounting even if the source directory has since been removed
    targetDir=$(realpath -m "${targetPath}")
    slug=$(slugify "${targetDir}")
    init_mounts_db

    if [ -z "$(mount_refcount "${slug}")" ] && ! mountpoint -q "${DC2MOUNTSDIR}/${slug}"; then
        echo "${targetDir} is not mounted" >&2
        exit 1
    fi

    release_mount "${targetDir}"

    refCount=$(mount_refcount "${slug}")
    if [ -n "${refCount}" ]; then
        echo "${targetDir} is still in use (refcount ${refCount})"
    else
        echo "Unmounted ${targetDir} from ${container}:/mnt/${slug}"
    fi
elif [ "${1:-}" == "ps" ]; then
    docker ps -a -f name=dockerclaude

    (IFS=$'\n' && for s in $(docker logs dockerclaude | tail -n2); do echo "> $s"; done)

    echo
    echo "Sessions/mounts:"
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
        echo 'None'
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
elif [ -d "$@" ]; then
    parse_mount_args "" "$@"
    start_session "${targetPath}"
else
    echo "Invalid path or command \`$@\`"
fi
