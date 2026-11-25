# herdr-joined: container spaces joined with their Herdr space

Herdr keeps each space on one machine. A dockerclaude session started from a
Herdr space `myspace` therefore runs claude in a second `myspace` that belongs
to the container's machine (`dockerclaude-myspace`). This adds a Herdr layout
that shows the two as one space:

- **Sidebar:** the container's space gets no row of its own, and the
  container's machine group is hidden once all its spaces are joined. The host
  space's row gets a server icon and stays highlighted while either space is
  shown. In the narrow sidebar, its status dot shows the more urgent of the
  two.
- **New space:** the sidebar's + makes the space on this computer even while
  a container's space is shown (Herdr's default is the shown space's machine).
  It opens in the host folder the container is in under `/mnt`, else in the
  joined host space's folder, else in your home folder. "New space" in the
  tab bar's + menu does the same, and so does `ctrl-shift-n` with `--keys`.
  The menu's Terminal and agent items still open in the shown space. Only the
  command palette (Herdr's own, not a mod) still uses Herdr's default.
- **Tab bar:** the host space's tabs come first, then the container's, which
  are marked with a server icon. Clicking a container tab shows it, and the
  mouse wheel steps through both spaces' tabs. Dragging only reorders tabs
  within their own space.
- **Keys** (with `--keys`, see below): next/previous tab (`ctrl-tab`,
  `ctrl-shift-tab`, `ctrl-pagedown`, `ctrl-pageup`, `alt-]`, `alt-[`) go
  through both spaces' tabs, and `ctrl-1`…`ctrl-9` pick by position in the
  joined tab bar.

It only changes what you see: Herdr still holds two spaces. A pane can't
mix machines (Herdr refuses with "A terminal's pane must stay on its own
machine"), and a tab can't be dragged from one space into the other.

## Which spaces join

A space on another machine joins a space on this computer when:

- it is on a dockerclaude container (machine `dockerclaude-*`) and has the
  same space id. dockerclaude starts claude in a container space with the id
  of the Herdr space you ran it from, so this works for unnamed spaces and
  `-s` too; or
- both spaces are named, with the same name, on any machine.

The rule is in `join.js`, which is the same in each mod folder.

## Install

```sh
extras/herdr-joined/install.sh                 # copy of your `main` layout
extras/herdr-joined/install.sh my-look         # or of another layout
extras/herdr-joined/install.sh --keys          # also bind the keys
extras/herdr-joined/install.sh --uninstall     # remove it all
```

The script:

1. copies `mods/dockerclaude-tabs`, `mods/dockerclaude-spaces` and
   `mods/dockerclaude-new-menu` into `~/.config/herdr-app/mods/` (or
   `$HERDR_APP_HOME`), checking each with `herdr-app check`;
2. writes `layouts/dockerclaude.json`, a copy of the chosen layout that uses
   them in place of the shipped `tabs` and `spaces` mods;
3. with `--keys`, adds a `"layout:dockerclaude"` section to `keys.json` (it
   needs `python3`);
4. switches the window to it.

`ctrl-shift-l` cycles back to your other layouts. Running the script again
updates the mods (and with `--keys`, the key section) and keeps an existing
`dockerclaude.json`.

### The keys

Herdr uses a `layout:<name>` section of `keys.json` only while that layout is
shown, and prefers it to `global`. Your other layouts keep their keys. The
script takes the keys you have bound globally to `tab.next`, `tab.prev`,
`tab.select:<n>` and `space.new`, and binds the same keys in that section
to the `dockerclaude-tabs` mod's `next`, `prev`, `select-<n>` and
`new-space` commands. Keys you add to the section yourself are kept.

The first change saves the old file as `keys.json.before-dockerclaude`.
Herdr updates an untouched `keys.json` with new default shortcuts on
upgrades, but leaves an edited one alone. `--uninstall` therefore puts back
the exact old file when nothing else in it changed, so Herdr can tell it is
untouched again. Otherwise it only removes the script's bindings.

The mods are the shipped `tabs`, `spaces` and `new-menu` mods of Herdr 0.1.17
with a few changes, described at the top of each `main.js`
(`diff` against `~/.config/herdr-app/mods/{tabs,spaces,new-menu}` shows them). When a Herdr update
changes the shipped ones, these won't follow until they are redone from the
new versions.
