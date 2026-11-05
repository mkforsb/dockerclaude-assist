# dclgui

A simple [Iced](https://iced.rs) GUI frontend for `dockerclaude.sh`. It has no
knowledge of its own: everything it shows and does comes from running the script
and parsing its output.

```sh
cargo run --release
```

The script is looked for in `$DCLGUI_SCRIPT`, then in the directories above the
executable, i.e. the checkout it was built in, and finally on `PATH` as
`dockerclaude` or `dockerclaude.sh`.

## Tabs

**Overview** lists every instance with its container, snapshot, latest log
output, last prompt, and sessions and mounts. It refreshes every 30 seconds, as
`ps -a` is slow, and after every action. Refresh does so right away.

**Doctor** lists the problems found by `dockerclaude doctor`, offering to run
the command fixing each of them. It refreshes every 10 seconds, and the number
of problems is shown next to the tab.

## Buttons

| Button                 | Command                                         |
| ---------------------- | ----------------------------------------------- |
| ▶️                     | `start-container`                               |
| ⏹️                     | `halt-container`                                |
| x (container)          | `stop-container`, which removes the container   |
| x (snapshot)           | `snapshot-delete`                               |
| Snapshot               | `snapshot`                                      |
| Restore                | `restore`                                       |
| Bash                   | `bash`, in an `x-terminal-emulator`             |
| Rename                 | `rename-session <old> <new>`                    |
| x (mount)              | `umount <path>`, or `force-clean-mounts <slug>` if broken |
| Add mount              | `mount <path> [<session>]`                      |
| Add session w/ mount   | `mount <path> <session>`                        |
| Add prefix             | `start-container` with the new `DCPREFIX`       |

Directories to mount are chosen with the desktop's file chooser, by way of
`xdg-desktop-portal`, or else `zenity` if there is no portal.

## Look

`src/style.rs` approximates libadwaita, following the system's light or dark
preference where the desktop portal reports one. Accent blue is kept to the
default action of a dialog and focused entries, and red to destructive
confirmations and errors.

## sudo

`dockerclaude.sh` needs `sudo` for mounting, which has no terminal to ask for
its password on here. Unless `SUDO_ASKPASS` is already set, dclgui has sudo run
dclgui itself as the askpass helper, popping up a password prompt.
