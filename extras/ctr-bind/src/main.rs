//! ctr-bind: bind-mount a host path into a *running* container's mount
//! namespace using the new mount API (open_tree + move_mount, Linux >= 5.2).
//!
//! The mount lives only in the container's namespace: it never appears in
//! the host mount table and disappears when the container stops.
//!
//! Usage:
//!   ctr-bind [--ro] [--mkdir] <container|--pid PID> <host-src> <ctr-dst>
//!   ctr-bind --umount <container|--pid PID> <ctr-dst>
//!
//! The container engine used to resolve names is `docker` by default;
//! set CTR_ENGINE=podman to use Podman.
//!
//! Rootless containers: run ctr-bind as the same user that runs the
//! container engine (e.g. with DOCKER_HOST pointing at the rootless daemon).
//! If the container is in a different user namespace than ctr-bind, the
//! tool joins that user namespace first, so the mount is made with the
//! container's privileges: container root == your host user.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{exit, Command};

// Constants from <linux/mount.h> / <linux/fcntl.h>, defined here so we don't
// depend on a particular libc crate version exposing them.
const OPEN_TREE_CLONE: libc::c_uint = 1;
const OPEN_TREE_CLOEXEC: libc::c_uint = libc::O_CLOEXEC as libc::c_uint;
const AT_RECURSIVE: libc::c_uint = 0x8000;
const AT_EMPTY_PATH: libc::c_uint = 0x1000;
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x4;
const MOUNT_ATTR_RDONLY: u64 = 0x1;

#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn check(ret: libc::c_long, what: &str) -> io::Result<libc::c_long> {
    if ret < 0 {
        let e = io::Error::last_os_error();
        Err(io::Error::new(e.kind(), format!("{what}: {e}")))
    } else {
        Ok(ret)
    }
}

/// Clone `src` (recursively) into a detached mount and return its fd.
fn open_tree_clone(src: &str) -> io::Result<OwnedFd> {
    let path = cstr(src)?;
    let fd = check(
        unsafe {
            libc::syscall(
                libc::SYS_open_tree,
                libc::AT_FDCWD,
                path.as_ptr(),
                OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC | AT_RECURSIVE,
            )
        },
        &format!("open_tree({src})"),
    )?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

/// Make the detached tree read-only *before* it is attached anywhere.
fn set_readonly(fd: &OwnedFd) -> io::Result<()> {
    let attr = MountAttr { attr_set: MOUNT_ATTR_RDONLY, attr_clr: 0, propagation: 0, userns_fd: 0 };
    let empty = cstr("")?;
    check(
        unsafe {
            libc::syscall(
                libc::SYS_mount_setattr,
                fd.as_raw_fd(),
                empty.as_ptr(),
                AT_EMPTY_PATH | AT_RECURSIVE,
                &attr as *const MountAttr,
                std::mem::size_of::<MountAttr>(),
            )
        },
        "mount_setattr(RDONLY)",
    )?;
    Ok(())
}

/// Attach the detached tree at `dst` in the *current* mount namespace.
fn move_mount_to(fd: &OwnedFd, dst: &str) -> io::Result<()> {
    let empty = cstr("")?;
    let target = cstr(dst)?;
    check(
        unsafe {
            libc::syscall(
                libc::SYS_move_mount,
                fd.as_raw_fd(),
                empty.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                MOVE_MOUNT_F_EMPTY_PATH,
            )
        },
        &format!("move_mount(-> {dst})"),
    )?;
    Ok(())
}

fn setns(fd: &File, nstype: libc::c_int, what: &str) -> io::Result<()> {
    if unsafe { libc::setns(fd.as_raw_fd(), nstype) } != 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(e.kind(), format!("setns({what}): {e}")));
    }
    Ok(())
}

/// The container's namespaces, opened up front (before we change any of our
/// own namespaces or credentials).
struct ContainerNs {
    mnt: File,
    user: File,
    /// The container lives in a different user namespace than we do
    /// (rootless engine, or userns-remap).
    foreign_userns: bool,
}

impl ContainerNs {
    fn open(pid: u32) -> io::Result<Self> {
        let open = |ns: &str| {
            File::open(format!("/proc/{pid}/ns/{ns}"))
                .map_err(|e| io::Error::new(e.kind(), format!("open /proc/{pid}/ns/{ns}: {e}")))
        };
        let mnt = open("mnt")?;
        let user = open("user")?;
        let ours = fs::metadata("/proc/self/ns/user")?;
        let theirs = user.metadata()?;
        let foreign_userns = (ours.dev(), ours.ino()) != (theirs.dev(), theirs.ino());
        Ok(Self { mnt, user, foreign_userns })
    }

    /// Rootless: join the container's user namespace (gaining full
    /// capabilities *within* it, nothing on the host), then create a private
    /// mount namespace owned by it. That mount namespace still shows the
    /// host's filesystem, and lets us clone mounts that the container's
    /// namespace is allowed to receive.
    fn prepare_rootless(&self) -> io::Result<()> {
        if !self.foreign_userns {
            return Ok(());
        }
        setns(&self.user, libc::CLONE_NEWUSER, "container user namespace")?;
        if unsafe { libc::unshare(libc::CLONE_NEWNS) } != 0 {
            let e = io::Error::last_os_error();
            return Err(io::Error::new(e.kind(), format!("unshare(CLONE_NEWNS): {e}")));
        }
        Ok(())
    }

    /// Enter the container's mount namespace. This also resets our root and
    /// cwd to the namespace's root, i.e. the container's root filesystem.
    fn enter_mnt(&self) -> io::Result<()> {
        setns(&self.mnt, libc::CLONE_NEWNS, "container mount namespace")
    }
}

fn container_pid(name: &str) -> io::Result<u32> {
    let engine = std::env::var("CTR_ENGINE").unwrap_or_else(|_| "docker".into());
    let out = Command::new(&engine)
        .args(["inspect", "-f", "{{.State.Pid}}", name])
        .output()
        .map_err(|e| io::Error::new(e.kind(), format!("running {engine}: {e}")))?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "{engine} inspect {name}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let pid: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| io::Error::other("could not parse container PID"))?;
    if pid == 0 {
        return Err(io::Error::other(format!("container {name} is not running")));
    }
    Ok(pid)
}

/// Create the mount point inside the container: a directory for directory
/// sources, an empty file for file sources.
fn make_mountpoint(dst: &str, src_is_dir: bool) -> io::Result<()> {
    let p = Path::new(dst);
    if p.exists() {
        return Ok(());
    }
    if src_is_dir {
        fs::create_dir_all(p)
    } else {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).truncate(false).write(true).open(p).map(|_| ())
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: ctr-bind [--ro] [--mkdir] <container|--pid PID> <host-src> <ctr-dst>\n       \
         ctr-bind --umount <container|--pid PID> <ctr-dst>"
    );
    exit(2)
}

fn run() -> io::Result<()> {
    let mut ro = false;
    let mut mkdir = false;
    let mut umount = false;
    let mut pid: Option<u32> = None;
    let mut pos: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ro" => ro = true,
            "--mkdir" => mkdir = true,
            "--umount" => umount = true,
            "--pid" => pid = Some(args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            _ if a.starts_with("--") => usage(),
            _ => pos.push(a),
        }
    }

    // With --pid there is no container name among the positionals.
    let (pid, rest) = match pid {
        Some(p) => (p, &pos[..]),
        None if !pos.is_empty() => (container_pid(&pos[0])?, &pos[1..]),
        None => usage(),
    };

    let ns = ContainerNs::open(pid)?;

    if umount {
        let [dst] = rest else { usage() };
        ns.prepare_rootless()?;
        ns.enter_mnt()?;
        let target = cstr(dst)?;
        if unsafe { libc::umount2(target.as_ptr(), libc::MNT_DETACH) } != 0 {
            let e = io::Error::last_os_error();
            return Err(io::Error::new(e.kind(), format!("umount({dst}): {e}")));
        }
        return Ok(());
    }

    let [src, dst] = rest else { usage() };
    if !dst.starts_with('/') {
        return Err(io::Error::other("container path must be absolute"));
    }

    // 1. Resolve the source on the host and clone it into a detached mount.
    //    (Resolve to an absolute path first: relative paths would otherwise
    //    be fine too, but this gives clearer errors.)
    let src = fs::canonicalize(src)
        .map_err(|e| io::Error::new(e.kind(), format!("{src}: {e}")))?;
    let src = src.to_str().ok_or_else(|| io::Error::other("source path is not UTF-8"))?;
    let src_is_dir = fs::metadata(src)?.is_dir();
    ns.prepare_rootless()?;
    let tree = open_tree_clone(src)?;

    // 2. Set flags while it's still detached, so it is never visible writable.
    if ro {
        set_readonly(&tree)?;
    }

    // 3. Switch into the container's mount namespace; from here on, paths
    //    are resolved inside the container.
    ns.enter_mnt()?;
    if mkdir {
        make_mountpoint(dst, src_is_dir)?;
    }

    // 4. Attach it.
    move_mount_to(&tree, dst)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ctr-bind: {e}");
        exit(1);
    }
}
