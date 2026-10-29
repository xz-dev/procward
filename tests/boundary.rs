// SPDX-License-Identifier: BSD-3-Clause
// Descendants across an access boundary: payload status must propagate; cleanup kills what
// the OS allows and reports what it denies (docs/platforms.md).
#![cfg(unix)]
use rustix::{
    io::Errno,
    process::{self, Pid, Signal},
};
use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, chown},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const SCRIPT: &str = r#""$1" boundary "$2" keep &
"$1" boundary "$2" root &
if [ "$(uname)" = Linux ]; then "$3" boundary "$2" nondumpable & fi
"$3" await "$2/go"
exit 23"#;
const DENIED: &str = "denied signals to privileged descendant";
// Conventional unprivileged account; only used when the test itself runs as root.
const NOBODY: u32 = 65534;

fn poll(limit: Duration, phase: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !predicate() {
        assert!(Instant::now() < deadline, "deadline expired: {phase}");
        thread::sleep(Duration::from_millis(5));
    }
}
fn alive(pid: i32) -> bool {
    match process::test_kill_process(Pid::from_raw(pid).unwrap()) {
        Ok(()) | Err(Errno::PERM) => true,
        Err(Errno::SRCH) => false,
        Err(error) => panic!("existence probe for {pid} failed: {error}"),
    }
}
fn fixture() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/fixture")
}
fn install(from: &Path, to: &Path, mode: u32) {
    fs::copy(from, to).unwrap();
    chown(to, Some(0), Some(0)).unwrap();
    fs::set_permissions(to, fs::Permissions::from_mode(mode)).unwrap();
}

struct Boundary {
    dir: PathBuf,
    child: Child,
    // Real UID procward runs with; helpers that keep it remain signalable.
    user: u32,
    pids: Vec<(&'static str, i32)>,
    unsupported: bool,
}
impl Boundary {
    fn start() -> Option<Self> {
        let dir = std::env::temp_dir().join(format!("procward-boundary-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        let helper = dir.join("setuid-fixture");
        let (procward, plain, user) = if process::geteuid().is_root() {
            // Drop procward to an unprivileged user so the boundary is real; copies stay reachable.
            install(fixture().as_path(), &helper, 0o4755);
            let (procward, plain) = (dir.join("procward"), dir.join("fixture"));
            install(Path::new(env!("CARGO_BIN_EXE_procward")), &procward, 0o755);
            install(fixture().as_path(), &plain, 0o755);
            (procward, plain, NOBODY)
        } else {
            let installed = Command::new("sudo")
                .args(["-n", "install", "-o", "root", "-m", "4755"])
                .arg(fixture())
                .arg(&helper)
                .stdin(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !installed {
                fs::remove_dir_all(&dir).unwrap();
                assert!(
                    std::env::var_os("CI").is_none(),
                    "CI needs root or passwordless sudo for the access-boundary test"
                );
                eprintln!(
                    "BOUNDARY-NOT-RUN: needs root or passwordless sudo for a setuid-root helper"
                );
                return None;
            }
            (
                PathBuf::from(env!("CARGO_BIN_EXE_procward")),
                fixture(),
                process::getuid().as_raw(),
            )
        };
        let mut command = Command::new(procward);
        command
            .args(["run", "--", "/bin/sh", "-c", SCRIPT, "sh"])
            .arg(&helper)
            .arg(&dir)
            .arg(plain)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(dir.join("stderr.log")).unwrap());
        if process::geteuid().is_root() {
            command.uid(NOBODY).gid(NOBODY);
        }
        Some(Self {
            child: command.spawn().unwrap(),
            dir,
            user,
            pids: Vec::new(),
            unsupported: false,
        })
    }
    fn stderr(&self) -> String {
        fs::read_to_string(self.dir.join("stderr.log")).unwrap()
    }
}
impl Drop for Boundary {
    fn drop(&mut self) {
        let _ = fs::write(self.dir.join("release"), b"go");
        let _ = fs::write(self.dir.join("go"), b"go");
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut rescued = Vec::new();
        for (kind, pid) in &self.pids {
            // A released root helper exits by itself; anything else left is a survivor.
            if *kind != "root" && alive(*pid) {
                process::kill_process(Pid::from_raw(*pid).unwrap(), Signal::KILL).unwrap();
                rescued.push(*kind);
            }
        }
        poll(Duration::from_secs(3), "harness cleanup", || {
            self.pids.iter().all(|(_, pid)| !alive(*pid))
        });
        if !rescued.is_empty() {
            eprintln!(
                "HARNESS RESCUE: {rescued:?} survived; NOT product cleanup; stderr={:?}",
                fs::read_to_string(self.dir.join("stderr.log"))
            );
        }
        fs::remove_dir_all(&self.dir).unwrap();
        assert!(
            rescued.is_empty() || self.unsupported || thread::panicking(),
            "access-boundary descendants needed rescue"
        );
    }
}

#[test]
fn descendant_across_access_boundary_keeps_status_and_reports_denial() {
    let Some(mut run) = Boundary::start() else {
        return;
    };
    let kinds: &[&'static str] = if cfg!(target_os = "linux") {
        &["keep", "root", "nondumpable"]
    } else {
        &["keep", "root"]
    };
    for &kind in kinds {
        let path = run.dir.join(format!("{kind}.pid"));
        poll(Duration::from_secs(20), "helper readiness", || {
            path.try_exists().unwrap()
        });
        let ids: Vec<u32> = fs::read_to_string(&path)
            .unwrap()
            .split_whitespace()
            .map(|value| value.parse().unwrap())
            .collect();
        run.pids.push((kind, ids[0] as i32));
        let wanted = match kind {
            "keep" => (run.user, 0),
            "root" => (0, 0),
            _ => (run.user, run.user),
        };
        if (ids[1], ids[2]) != wanted {
            if kind != "nondumpable" && std::env::var_os("PROCWARD_TEST_QEMU_USER").is_some() {
                run.unsupported = true;
                println!(
                    "EMULATOR-UNSUPPORTED: descendant_across_access_boundary: qemu-user binfmt did not honour setuid (helper uid {}/{})",
                    ids[1], ids[2]
                );
                return;
            }
            panic!(
                "{kind} helper has real/effective UID {}/{}, expected {wanted:?}",
                ids[1], ids[2]
            );
        }
    }
    assert!(
        run.child.try_wait().unwrap().is_none(),
        "procward exited while descendants crossed the boundary: {}",
        run.stderr()
    );
    fs::write(run.dir.join("go"), b"go").unwrap();
    poll(Duration::from_secs(6), "denial report", || {
        run.stderr().contains(DENIED) || run.child.try_wait().unwrap().is_some()
    });
    assert!(
        run.child.try_wait().unwrap().is_none(),
        "procward stopped cleanup instead of reporting the denied descendant: {}",
        run.stderr()
    );
    let allowed: Vec<i32> = run
        .pids
        .iter()
        .filter(|(kind, _)| *kind != "root")
        .map(|(_, pid)| *pid)
        .collect();
    poll(
        Duration::from_secs(6),
        "signalable descendants removed",
        || allowed.iter().all(|pid| !alive(*pid)),
    );
    assert!(run.child.try_wait().unwrap().is_none(), "{}", run.stderr());
    fs::write(run.dir.join("release"), b"go").unwrap();
    poll(Duration::from_secs(8), "owner completion", || {
        run.child.try_wait().unwrap().is_some()
    });
    assert_eq!(
        run.child.wait().unwrap().code(),
        Some(23),
        "{}",
        run.stderr()
    );
    let root = run.pids[1].1;
    poll(Duration::from_secs(3), "released root helper exit", || {
        !alive(root)
    });
}
