#!/bin/sh
# Installs the dockerclaude-tabs, dockerclaude-spaces and dockerclaude-new-menu
# mods into the Herdr app, and a `dockerclaude` layout: a copy of one of your
# layouts (default: main) that uses them in place of the shipped `tabs` and
# `spaces` mods (dockerclaude-tabs opens dockerclaude-new-menu).
#
#   extras/herdr-joined/install.sh [--keys] [LAYOUT]
#   extras/herdr-joined/install.sh --uninstall
#
# --keys also adds a "layout:dockerclaude" section to keys.json: the keys you
# have bound to tab.next, tab.prev, tab.select and space.new then act on the
# joined tabs (and make new spaces on this computer) while that layout shows.
# Note that Herdr stops updating an edited keys.json's default shortcuts on
# upgrades; if you never edited it, this is the first edit.
#
# Running it again updates the mods (and with --keys, the key section); an
# existing `dockerclaude` layout is left as it is (delete it first to make it
# again). --uninstall removes the layout, the mods and the key section.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
app=${HERDR_APP_HOME:-$HOME/.config/herdr-app}
name=dockerclaude
mods="dockerclaude-tabs dockerclaude-spaces dockerclaude-new-menu"
keys=false
uninstall=false
from=main
for arg in "$@"; do
    case $arg in
        --keys) keys=true ;;
        --uninstall) uninstall=true ;;
        -h | --help) sed -n '2,18s/^# \{0,1\}//p' "$0"; exit 0 ;;
        -*) echo "unknown option $arg" >&2; exit 2 ;;
        *) from=$arg ;;
    esac
done

command -v herdr-app >/dev/null || { echo "herdr-app is not on PATH" >&2; exit 1; }
[ -d "$app/layouts" ] || { echo "$app/layouts not found: start the Herdr app once first" >&2; exit 1; }

# Edits keys.json's "layout:dockerclaude" section: `add` (re)makes our
# bindings from the global ones, `remove` takes ours out. Bindings someone
# added there themselves stay. Keeps a backup of the file the first time.
edit_keys() {
    command -v python3 >/dev/null || { echo "editing keys.json needs python3" >&2; exit 1; }
    python3 -I - "$1" "$app/keys.json" "$name" <<'EOF'
import json, os, re, shutil, sys, time

mode, path, layout = sys.argv[1:]
section = f"layout:{layout}"
ours = ("dockerclaude-tabs.",)
if not os.path.exists(path):
    sys.exit(f"{path} not found: start the Herdr app once first")
with open(path) as f:
    keys = json.load(f)
mine = {k: v for k, v in keys.get(section, {}).items() if not (isinstance(v, str) and v.startswith(ours))}

def action(v):
    """Our command for a global binding, or None."""
    if isinstance(v, dict):
        name, args = v.get("command"), v.get("args") or {}
        index = args.get("index")
    else:
        name, _, value = str(v).partition(":")
        index = value or None
    if name == "tab.next":
        return "next"
    if name == "tab.prev":
        return "prev"
    if name == "space.new" and not index:
        return "new-space"
    if name == "tab.select" and index and re.fullmatch(r"[1-9][0-9]*", str(index)):
        return f"select-{index}"
    return None

added = {}
if mode == "add":
    for key, v in keys.get("global", {}).items():
        a = action(v)
        if a and key not in mine:
            added[key] = f"dockerclaude-tabs.{a}"
    mine.update(added)
    # The terminal section wins over a layout section while a terminal has focus.
    for key in sorted(set(added) & set(keys.get("terminal", {}))):
        print(f"warning: {key} is bound in keys.json's terminal section, which wins while a terminal has focus", file=sys.stderr)

new = dict(keys)
if mine:
    new[section] = mine
else:
    new.pop(section, None)
if new == keys:
    print(f"keys.json: nothing to change")
    sys.exit(0)
backup = f"{path}.before-{layout}"
if mode == "remove" and os.path.exists(backup):
    with open(backup) as f:
        if json.load(f) == new:
            # Nothing else changed since: put back the very same bytes, so Herdr
            # can tell an untouched file again.
            os.replace(backup, path)
            print(f"keys.json: restored the one from before the install")
            sys.exit(0)
if not os.path.exists(backup):
    shutil.copy2(path, backup)
    print(f"keys.json: saved the old one as {backup}")
tmp = f"{path}.tmp-{os.getpid()}"
with open(tmp, "w") as f:
    json.dump(new, f, indent=2, ensure_ascii=False)
    f.write("\n")
os.replace(tmp, path)
if mode == "add":
    print(f"keys.json: {section} binds " + ", ".join(f"{k} → {v.split('.', 1)[1]}" for k, v in sorted(added.items())))
else:
    print(f"keys.json: removed the dockerclaude bindings from {section}")
EOF
    # A running window rereads it at once and writes keys.error if it's invalid.
    sleep 1
    if [ "$app/keys.error" -nt "$app/keys.json" ]; then
        echo "Herdr reports a problem with keys.json:" >&2
        cat "$app/keys.error" >&2
    fi
}

if $uninstall; then
    [ -f "$app/keys.json" ] && grep -q "\"layout:$name\"" "$app/keys.json" && edit_keys remove
    if herdr-app status 2>/dev/null | grep -q "\"layout\": \"$name\""; then
        herdr-app do layout.switch '{"name":"main"}' >/dev/null 2>&1 || true
    fi
    rm -f "$app/layouts/$name.json"
    for mod in $mods; do rm -rf "${app:?}/mods/$mod"; done
    echo "removed the $name layout and the $mods mods"
    exit 0
fi

for mod in $mods; do
    cmp -s "$here/mods/$mod/join.js" "$here/mods/dockerclaude-tabs/join.js" ||
        { echo "$here/mods/$mod/join.js differs from dockerclaude-tabs' copy; they must be the same" >&2; exit 1; }
done
for mod in $mods; do
    mkdir -p "$app/mods/$mod"
    cp "$here/mods/$mod/"*.js "$here/mods/$mod/mod.json" "$app/mods/$mod/"
    herdr-app check "$app/mods/$mod" >/dev/null 2>&1 || {
        echo "herdr-app check failed for $mod:" >&2
        herdr-app check "$app/mods/$mod" >&2 || true
        exit 1
    }
    echo "installed and checked $app/mods/$mod"
done

layout="$app/layouts/$name.json"
if [ -e "$layout" ]; then
    echo "kept the existing $layout"
else
    src="$app/layouts/$from.json"
    [ -f "$src" ] || { echo "no layout $src" >&2; exit 1; }
    grep -q '"mod": *"tabs"' "$src" && grep -q '"mod": *"spaces"' "$src" ||
        echo "warning: $from.json doesn't use both the tabs and spaces mods; the copy may not change much" >&2
    sed -e 's/"mod": *"tabs"/"mod": "dockerclaude-tabs"/g' \
        -e 's/"mod": *"spaces"/"mod": "dockerclaude-spaces"/g' "$src" >"$layout.tmp"
    mv "$layout.tmp" "$layout"
    echo "made $layout from $from.json"
fi

if $keys; then
    edit_keys add
fi

if herdr-app do layout.switch "{\"name\":\"$name\"}" >/dev/null 2>&1; then
    echo "switched the Herdr window to the $name layout (ctrl-shift-l cycles layouts)"
else
    echo "pick the $name layout in the Herdr window: ctrl-shift-l, or Settings"
fi
