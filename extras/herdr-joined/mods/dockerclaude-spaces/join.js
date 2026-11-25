// Which spaces show as one: a space on another machine joins a space on this
// computer when
// - it is on a dockerclaude container (`dockerclaude-<space>`) and has the same
//   id: dockerclaude starts claude in a space on the container with the id of
//   the Herdr space it was run from; or
// - both are named, with the same name (any machine).
// The same file is in each dockerclaude-* mod folder; keep them equal (install.sh checks).

import { spaces, run, focused, home } from "herdr";

export const CONTAINER_PREFIX = "dockerclaude-";

// A remote space's id without its "<machine>/" prefix.
const bare = (id) => id.slice(id.indexOf("/") + 1);

/** The space on this computer that remote space `s` joins, or null. */
export function hostOf(s, all) {
  if (!s?.machine) return null;
  const container = s.machine.startsWith(CONTAINER_PREFIX);
  return all.find((h) => !h.machine && ((container && bare(s.id) === h.id) || (h.named && s.named && h.name === s.name))) ?? null;
}

/** The spaces shown together with `space`: its host first, then the joined
 *  ones in sidebar order. Just [space] when it joins nothing. */
export function group(all, space) {
  if (!space) return [];
  const host = space.machine ? hostOf(space, all) : space;
  if (!host) return [space];
  return [host, ...all.filter((s) => s.machine && hostOf(s, all)?.id === host.id)];
}

/** A new space, like `space.new`. Herdr makes it on the shown space's machine;
 *  while that is a dockerclaude container, make it on this computer instead, in
 *  the host folder the container shows under /mnt (or the joined space's
 *  folder, or home). */
export function newSpace() {
  const all = spaces(), shown = all.find((s) => s.active);
  if (!shown?.machine?.startsWith(CONTAINER_PREFIX)) return run("space.new");
  const cwd = focused()?.cwd ?? "";
  const folder = cwd.startsWith("/mnt/") ? cwd.slice(4) : hostOf(shown, all)?.folder || home();
  run("space.new", { machine: "", folder });
}

const RANK = { blocked: 4, done: 3, working: 2, idle: 1 };

/** The most urgent status of the spaces in `list` (blocked > done > working > idle), or null. */
export function urgent(list) {
  return list.map((s) => s.status).reduce((a, b) => ((RANK[b] ?? 0) > (RANK[a] ?? 0) ? b : a), null);
}
