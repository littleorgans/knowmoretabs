//! Every program content runs, in a process group of its own: started,
//! ended with everything it started, and ended too when knowmoretabs is.
//!
//! slice: content
//! why: A tool such as yt-dlp starts helpers of its own, and killing only
//!      the tool at its deadline left them running. Each program starts as
//!      the leader of its own process group, and ending it ends the group.
//!      A group of its own no longer hears the terminal's Ctrl-C, so the
//!      first command that runs a program installs a handler that ends every
//!      running group, removes every scratch directory and exits 130. One
//!      lock orders it all: starting and registering a program, creating and
//!      registering a scratch directory, ending a program, and the handler
//!      each happen whole, so nothing escapes between starting and
//!      registering and nothing starts once the handler has begun. A signal
//!      inherited as ignored stays ignored; then a signal cleans up nothing,
//!      and the run says so once. Windows ends a tree with `taskkill /T /F`
//!      while its leader is alive.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus};
use std::sync::{Mutex, MutexGuard, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::capture::Log;

/// How long the members of an ended group are given to go before the
/// program is named for leaving them.
#[cfg(unix)]
const SURVIVOR_WAIT: Duration = Duration::from_secs(1);
/// How long `taskkill` may take to end a tree.
#[cfg(windows)]
const TASKKILL_WAIT: Duration = Duration::from_secs(3);
/// How often an ended group, or `taskkill`, is looked at.
const POLL: Duration = Duration::from_millis(20);
/// The exit code of a run a signal ended, as a shell reports Ctrl-C.
const INTERRUPTED: i32 = 130;

/// What a signal must clean up. Only ever touched under its lock.
#[derive(Debug, Default)]
struct Registry {
    /// The handler has begun: nothing more starts.
    stopping: bool,
    /// The leaders of the registered groups, each unreaped, so no group id
    /// here can have been reused.
    trees: Vec<u32>,
    scratch: Vec<PathBuf>,
    /// The programs already named for leaving processes behind.
    warned: Vec<String>,
}

impl Registry {
    /// The handler's work: nothing starts from now on, every registered
    /// group is ended and every scratch directory removed, one try each.
    fn stop(&mut self) {
        self.stopping = true;
        for &id in &self.trees {
            kill_group(id);
        }
        for path in &self.scratch {
            let _ = fs::remove_dir_all(path);
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    stopping: false,
    trees: Vec::new(),
    scratch: Vec::new(),
    warned: Vec::new(),
});

fn lock(registry: &Mutex<Registry>) -> MutexGuard<'_, Registry> {
    registry.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a signal cleans up, installing the handler on the first call.
/// The handler takes SIGINT, SIGTERM and SIGHUP together, and is refused
/// when any of them was inherited as ignored; then all three keep what
/// they were, and an ignored Ctrl-C stays ignored.
pub fn signals_covered() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        ctrlc::try_set_handler(|| {
            // `stop` runs whole under the lock, and once it has, nothing
            // more starts. It is the only lock taken here: never the
            // archive's, which the system releases at exit.
            lock(&REGISTRY).stop();
            std::process::exit(INTERRUPTED);
        })
        .is_ok()
    })
}

/// Warnings about processes left running are shown even under `-q`: no
/// command passes its `Log` down to the programs it runs.
fn warn(message: &str) {
    Log::default().warn(message);
}

/// Starts `command` as the leader of a process group of its own and
/// registers it, the signal handler installed first.
pub fn spawn(command: &mut Command) -> io::Result<Tree> {
    if !signals_covered() {
        static SAID: Once = Once::new();
        SAID.call_once(|| {
            warn(&format!(
                "signal cleanup is off (a signal was inherited as ignored); an interrupted run \
                 may leave {} running",
                program(command)
            ));
        });
    }
    spawn_in(&REGISTRY, command)
}

fn spawn_in(registry: &'static Mutex<Registry>, command: &mut Command) -> io::Result<Tree> {
    let mut held = lock(registry);
    if held.stopping {
        return Err(stopping());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn()?;
    held.trees.push(child.id());
    Ok(Tree {
        child,
        program: program(command),
        registry,
        registered: true,
    })
}

fn stopping() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "knowmoretabs is stopping")
}

/// The program's file name, for warnings.
fn program(command: &Command) -> String {
    let program = Path::new(command.get_program());
    program
        .file_name()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A running program and its process group: registered, its leader
/// unreaped, until the one retirement. Dropping it retires it.
#[derive(Debug)]
pub struct Tree {
    child: Child,
    program: String,
    registry: &'static Mutex<Registry>,
    registered: bool,
}

impl Tree {
    /// The leader's process id, which is also its group's.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Its standard output, the first time.
    pub fn stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// Its standard error, the first time.
    pub fn stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// How the program exited, if it has. Once it has, what it left running
    /// in its group is ended, in the same step as the reap, and the tree
    /// is retired.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut held = lock(self.registry);
        let status = self.child.try_wait()?;
        if status.is_some() && self.registered {
            // Windows cannot walk a tree whose leader is gone, and its
            // process id may already be another's.
            #[cfg(unix)]
            kill_group(self.id());
            self.unregister(&mut held);
            drop(held);
            self.survivors();
        }
        Ok(status)
    }

    /// Ends the program and everything in its group, reaps it and
    /// unregisters it; a second call does nothing. True when members of
    /// the group were still there after [`SURVIVOR_WAIT`].
    pub fn retire(&mut self) -> bool {
        let mut held = lock(self.registry);
        if !self.registered {
            return false;
        }
        kill_group(self.id());
        // The leader is this process's own unreaped child, so this reaches
        // it even when the group kill did not.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.unregister(&mut held);
        drop(held);
        self.survivors()
    }

    fn unregister(&mut self, held: &mut Registry) {
        let id = self.id();
        held.trees.retain(|&tree| tree != id);
        self.registered = false;
    }

    /// Whether the ended group still has members, named once per program.
    fn survivors(&self) -> bool {
        let left = group_lingers(self.id());
        if left {
            let mut held = lock(self.registry);
            if !held.warned.contains(&self.program) {
                held.warned.push(self.program.clone());
                // Output may block; the signal handler still needs the registry.
                drop(held);
                warn(&format!(
                    "{} left processes running after it was ended",
                    self.program
                ));
            }
        }
        left
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.retire();
    }
}

/// A private directory a program works in, registered while it exists so
/// that a signal removes it too. Dropping it removes it.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
    registry: &'static Mutex<Registry>,
}

impl Scratch {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let mut held = lock(self.registry);
        held.scratch.retain(|path| path != &self.path);
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A fresh scratch directory named `prefix` and a random tail, private to
/// this user, the signal handler installed first.
pub fn scratch(prefix: &str) -> io::Result<Scratch> {
    signals_covered();
    scratch_in(&REGISTRY, prefix)
}

fn scratch_in(registry: &'static Mutex<Registry>, prefix: &str) -> io::Result<Scratch> {
    let mut held = lock(registry);
    if held.stopping {
        return Err(stopping());
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let path = builder.tempdir()?.keep();
    held.scratch.push(path.clone());
    Ok(Scratch { path, registry })
}

/// Ends every process in group `id` at once.
#[cfg(unix)]
fn kill_group(id: u32) {
    use nix::sys::signal::{Signal, killpg};
    if let Ok(id) = i32::try_from(id) {
        let _ = killpg(nix::unistd::Pid::from_raw(id), Signal::SIGKILL);
    }
}

/// Ends process `id` and its descendants through `taskkill`, waiting for
/// it a bounded time. Only while `id` is alive: after that the tree cannot
/// be walked.
#[cfg(windows)]
fn kill_group(id: u32) {
    use std::process::Stdio;
    let Ok(mut taskkill) = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &id.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + TASKKILL_WAIT;
    while Instant::now() < deadline {
        match taskkill.try_wait() {
            Ok(None) => std::thread::sleep(POLL),
            Ok(Some(_)) | Err(_) => return,
        }
    }
    let _ = taskkill.kill();
    let _ = taskkill.wait();
}

/// Whether group `id` still has members after [`SURVIVOR_WAIT`]; signal 0
/// only asks.
#[cfg(unix)]
fn group_lingers(id: u32) -> bool {
    use nix::sys::signal::killpg;
    let Ok(id) = i32::try_from(id) else {
        return false;
    };
    let deadline = Instant::now() + SURVIVOR_WAIT;
    loop {
        if killpg(nix::unistd::Pid::from_raw(id), None).is_err() {
            return false;
        }
        if Instant::now() >= deadline {
            return true;
        }
        std::thread::sleep(POLL);
    }
}

/// Windows has no group to ask after `taskkill`.
#[cfg(windows)]
fn group_lingers(_id: u32) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A registry of the test's own: the handler is never installed and
    /// the process wide registry is never touched, so tests stay apart.
    fn fresh() -> &'static Mutex<Registry> {
        Box::leak(Box::default())
    }

    /// `sh` with two `sleep` children, and their process ids once both
    /// have started.
    fn family(registry: &'static Mutex<Registry>, dir: &Path) -> (Tree, Vec<u32>) {
        let pids = dir.join("pids");
        let script = format!(
            "sleep 30 & echo $! >> '{0}'; sleep 30 & echo $! >> '{0}'; wait",
            pids.display()
        );
        let tree = spawn_in(registry, Command::new("sh").args(["-c", &script])).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let ids: Vec<u32> = fs::read_to_string(&pids)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect();
            if ids.len() == 2 {
                return (tree, ids);
            }
            assert!(Instant::now() < deadline, "the children never started");
            std::thread::sleep(POLL);
        }
    }

    /// Whether process `id` exists, asked by a spawned `kill -0`, given a
    /// moment for the system to reap what was ended.
    fn alive(id: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let exists = Command::new("kill")
                .args(["-0", &id.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            if !exists || Instant::now() >= deadline {
                return exists;
            }
            std::thread::sleep(POLL);
        }
    }

    fn registered(registry: &Mutex<Registry>) -> (usize, usize) {
        let held = lock(registry);
        (held.trees.len(), held.scratch.len())
    }

    #[test]
    fn retiring_ends_the_whole_group() {
        let registry = fresh();
        let dir = tempfile::tempdir().unwrap();
        let (mut tree, children) = family(registry, dir.path());
        let leader = tree.id();
        assert_eq!(lock(registry).trees, [leader]);
        assert!(children.iter().all(|&id| alive(id)));
        assert!(!tree.retire(), "members survived");
        assert_eq!(registered(registry), (0, 0));
        assert!(!alive(leader));
        assert!(children.iter().all(|&id| !alive(id)));
    }

    #[test]
    fn a_dropped_tree_is_retired() {
        let registry = fresh();
        let dir = tempfile::tempdir().unwrap();
        let (tree, children) = family(registry, dir.path());
        drop(tree);
        assert_eq!(registered(registry), (0, 0));
        assert!(children.iter().all(|&id| !alive(id)));
    }

    #[test]
    fn a_tree_is_retired_once() {
        let registry = fresh();
        let mut tree = spawn_in(registry, Command::new("sleep").arg("30")).unwrap();
        assert!(!tree.retire());
        assert!(!tree.registered);
        // A process started since cannot be taken for this tree's leader.
        lock(registry).trees.push(tree.id());
        assert!(!tree.retire());
        assert_eq!(lock(registry).trees, [tree.id()]);
    }

    #[test]
    fn a_survivor_warning_does_not_hold_the_registry() {
        let registry = fresh();
        let mut tree = spawn_in(registry, Command::new("sleep").arg("30")).unwrap();
        let stderr = io::stderr();
        let held_stderr = stderr.lock();
        // Probe the live group to reach the warning without needing a
        // process that survives SIGKILL. Drop still retires this tree.
        let probing = std::thread::spawn(move || {
            let left = tree.survivors();
            tree.retire();
            left
        });
        let deadline = Instant::now() + SURVIVOR_WAIT + Duration::from_secs(5);
        let available = loop {
            if let Ok(held) = registry.try_lock()
                && !held.warned.is_empty()
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(POLL);
        };
        // Always unblock the warning and retire the tree before asserting.
        drop(held_stderr);
        assert!(probing.join().unwrap(), "the live group was not present");
        assert!(
            available,
            "a warning blocked the registry and signal cleanup"
        );
    }

    #[test]
    fn an_exit_ends_what_the_program_left_behind() {
        let registry = fresh();
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        let script = format!("sleep 30 & echo $! > '{}'", pid.display());
        let mut tree = spawn_in(registry, Command::new("sh").args(["-c", &script])).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = tree.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "sh never exited");
            std::thread::sleep(POLL);
        };
        assert!(status.success());
        assert_eq!(registered(registry), (0, 0));
        let left: u32 = fs::read_to_string(pid).unwrap().trim().parse().unwrap();
        assert!(!alive(left));
    }

    #[test]
    fn stopping_ends_every_group_and_starts_nothing() {
        let registry = fresh();
        let dir = tempfile::tempdir().unwrap();
        let (_tree, children) = family(registry, dir.path());
        let scratch = scratch_in(registry, "knowmoretabs-test-").unwrap();
        lock(registry).stop();
        assert!(children.iter().all(|&id| !alive(id)));
        assert!(!scratch.path().exists());
        let marker = dir.path().join("marker");
        let started = spawn_in(registry, Command::new("touch").arg(&marker));
        assert_eq!(
            started.map(|_| ()).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(scratch_in(registry, "knowmoretabs-test-").is_err());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!marker.exists());
    }

    #[test]
    fn scratch_is_private_and_registered_while_it_exists() {
        use std::os::unix::fs::PermissionsExt;
        let registry = fresh();
        let scratch = scratch_in(registry, "knowmoretabs-test-").unwrap();
        let path = scratch.path().to_owned();
        assert_eq!(lock(registry).scratch, [path.as_path()]);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(scratch);
        assert_eq!(registered(registry), (0, 0));
        assert!(!path.exists());
    }
}
