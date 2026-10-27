// SPDX-License-Identifier: BSD-3-Clause
#![cfg(target_os = "linux")]

use rustix::{
    fd::{AsFd, OwnedFd},
    process::{
        Pid, PidfdFlags, Signal, WaitId, WaitIdOptions, pidfd_open, pidfd_send_signal,
        set_child_subreaper, waitid,
    },
};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_procward"))
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

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "procward-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Run {
    child: Option<Child>,
    fixtures: Vec<OwnedFd>,
}

impl Run {
    fn start(mut command: Command) -> Self {
        // Nested procward owns normal reaping; this test-only reaper catches failed-run rescues.
        set_child_subreaper(Some(Pid::INIT)).unwrap();
        Self {
            child: Some(
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            ),
            fixtures: Vec::new(),
        }
    }

    fn fixture(mode: &str, dir: &Path) -> Self {
        let mut cmd = command();
        cmd.args([OsStr::new("run"), OsStr::new("--"), fixture().as_os_str()])
            .arg(mode)
            .arg(dir);
        Self::start(cmd)
    }

    fn remember(&mut self, dir: &Path, mode: &str) -> u32 {
        let path = dir.join(format!("{mode}.pid"));
        let mut pid = None;
        poll(|| {
            pid = match fs::read_to_string(&path) {
                Ok(text) => text.parse::<u32>().ok(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => panic!("{path:?}: {error}"),
            };
            pid.is_some()
        });
        let pid = pid.unwrap();
        // These fixture processes stay alive behind input/file gates until released.
        self.fixtures
            .push(pidfd_open(Pid::from_raw(pid as i32).unwrap(), PidfdFlags::empty()).unwrap());
        pid
    }

    fn release_primary(&mut self) {
        self.child
            .as_mut()
            .unwrap()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"x")
            .unwrap();
    }

    fn finish(&mut self) -> Output {
        poll(|| self.child.as_mut().unwrap().try_wait().unwrap().is_some());
        self.child.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let mut rescued = false;
        for pidfd in &self.fixtures {
            match pidfd_send_signal(pidfd, Signal::KILL) {
                Ok(()) => rescued = true,
                Err(rustix::io::Errno::SRCH) => {}
                Err(error) => {
                    rescued = true;
                    eprintln!("fixture rescue failed: {error}");
                }
            }
        }
        if let Some(child) = &mut self.child {
            let deadline = Instant::now() + Duration::from_secs(1);
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        for pidfd in &self.fixtures {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                match waitid(
                    WaitId::PidFd(pidfd.as_fd()),
                    WaitIdOptions::EXITED | WaitIdOptions::NOHANG,
                ) {
                    Ok(Some(_)) | Err(rustix::io::Errno::CHILD) => break,
                    Err(rustix::io::Errno::INTR) => continue,
                    result if Instant::now() >= deadline => {
                        eprintln!("rescue reaping failed: {result:?}");
                        rescued = true;
                        break;
                    }
                    _ => thread::sleep(Duration::from_millis(5)),
                }
            }
        }
        if rescued {
            eprintln!("HARNESS RESCUE: killed surviving fixture; this is not product cleanup");
            assert!(thread::panicking(), "fixture required harness rescue");
        }
    }
}

fn poll(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "observation deadline expired");
        thread::sleep(Duration::from_millis(5));
    }
}

fn absent(pid: u32) -> bool {
    match fs::metadata(format!("/proc/{pid}")) {
        Ok(_) => false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => panic!("process query failed: {error}"),
    }
}

#[test]
fn help_and_version_do_not_launch_payload() {
    for flag in ["--help", "-h", "--version", "-V"] {
        let mut cmd = command();
        cmd.arg(flag);
        let output = Run::start(cmd).finish();
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("procward 0.1.0")
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn native_argv_cwd_environment_stdio_and_primary_status_are_preserved() {
    let scratch = Scratch::new();
    let args = [
        OsString::from("two words"),
        OsString::from("--literal"),
        OsString::new(),
        OsString::from("$(touch should-not-exist); *"),
        OsString::from_vec(vec![b'x', 0xff]),
    ];
    let mut cmd = command();
    cmd.arg("run")
        .arg("--")
        .arg(fixture())
        .arg("probe")
        .args(&args)
        .current_dir(&scratch.0)
        .env("PROCWARD_TEST_VALUE", "inherited value");
    let mut run = Run::start(cmd);
    run.child
        .as_mut()
        .unwrap()
        .stdin
        .take()
        .unwrap()
        .write_all(b"input\0bytes")
        .unwrap();
    let output = run.finish();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    let mut expected = Vec::new();
    use std::os::unix::ffi::OsStrExt;
    for value in args
        .iter()
        .map(OsString::as_os_str)
        .chain([scratch.0.as_os_str(), OsStr::new("inherited value")])
    {
        let bytes = value.as_bytes();
        expected.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        expected.extend_from_slice(bytes);
    }
    expected.extend_from_slice(b"input\0bytes");
    assert_eq!(output.stdout, expected);
    assert_eq!(output.stderr, b"fixture-stderr\n");
    assert!(!scratch.0.join("should-not-exist").exists());
}

#[test]
fn primary_exit_cleans_ordinary_tree_without_touching_unrelated_work() {
    let dir = Scratch::new();
    let unrelated_dir = Scratch::new();
    let mut unrelated = Run::fixture("waiting", &unrelated_dir.0);
    let unrelated_pid = unrelated.remember(&unrelated_dir.0, "waiting");
    let mut run = Run::fixture("tree", &dir.0);
    let primary = run.remember(&dir.0, "tree");
    let branch = run.remember(&dir.0, "branch");
    let worker = run.remember(&dir.0, "worker");
    run.release_primary();
    let output = run.finish();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    for pid in [primary, branch, worker] {
        assert!(absent(pid), "owned process {pid} survived (zombies count)");
    }
    assert!(!absent(unrelated_pid));
    assert!(
        unrelated
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none()
    );
    unrelated.release_primary();
    assert!(unrelated.finish().status.success());
}

#[test]
fn healthy_primary_does_not_retain_adopted_zombies() {
    let dir = Scratch::new();
    let mut run = Run::fixture("orphan", &dir.0);
    let primary = run.remember(&dir.0, "orphan");
    let orphan = run.remember(&dir.0, "orphan-child");
    let owner = run.child.as_ref().unwrap().id();
    poll(|| dir.0.join("ready").try_exists().unwrap());
    poll(|| {
        let status = fs::read_to_string(format!("/proc/{orphan}/status")).unwrap();
        status.lines().any(|line| line == format!("PPid:\t{owner}"))
    });
    fs::write(dir.0.join("release"), b"exit now").unwrap();
    poll(|| absent(orphan));
    assert!(!absent(primary));
    assert!(run.child.as_mut().unwrap().try_wait().unwrap().is_none());
    run.release_primary();
    assert_eq!(run.finish().status.code(), Some(23));
}

#[test]
fn primary_signal_status_is_preserved() {
    let dir = Scratch::new();
    let mut run = Run::fixture("waiting", &dir.0);
    let _primary = run.remember(&dir.0, "waiting");
    pidfd_send_signal(&run.fixtures[0], Signal::TERM).unwrap();
    assert_eq!(run.finish().status.code(), Some(143));
}

#[test]
fn owner_termination_signals_reach_primary() {
    for (signal, code) in [(Signal::HUP, 129), (Signal::INT, 130), (Signal::TERM, 143)] {
        let dir = Scratch::new();
        let mut run = Run::fixture("waiting", &dir.0);
        run.remember(&dir.0, "waiting");
        let owner = pidfd_open(
            Pid::from_raw(run.child.as_ref().unwrap().id() as i32).unwrap(),
            PidfdFlags::empty(),
        )
        .unwrap();
        pidfd_send_signal(&owner, signal).unwrap();
        assert_eq!(run.finish().status.code(), Some(code));
    }
}

#[test]
fn rejected_cli_causes_no_payload_side_effects() {
    let dir = Scratch::new();
    let marker = dir.0.join("payload");
    for args in [vec![], vec!["run"], vec!["run", "--"]] {
        let mut cmd = command();
        cmd.args(args);
        assert_eq!(Run::start(cmd).finish().status.code(), Some(125));
    }
    for prefix in [
        vec!["unknown", "--"],
        vec!["run", "--bad", "--"],
        vec!["--help", "run", "--"],
        vec!["--version", "run", "--"],
        vec!["run", "--", ""],
    ] {
        let mut cmd = command();
        cmd.args(&prefix);
        if prefix.len() >= 2 {
            cmd.arg(fixture()).arg("touch").arg(&marker);
        }
        let output = Run::start(cmd).finish();
        assert_eq!(output.status.code(), Some(125), "{prefix:?}: {output:?}");
        assert!(!output.stderr.is_empty());
        assert!(!marker.try_exists().unwrap());
    }
}

#[test]
fn exec_failures_are_visible_without_falling_back_to_shell() {
    let dir = Scratch::new();
    let marker = dir.0.join("payload");
    let plain_text = dir.0.join("not-an-executable-image");
    fs::write(&plain_text, format!("touch {}\n", marker.display())).unwrap();
    fs::set_permissions(&plain_text, fs::Permissions::from_mode(0o700)).unwrap();
    let denied = dir.0.join("not-executable");
    fs::write(&denied, b"no execute permission").unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o600)).unwrap();
    for executable in [dir.0.join("missing"), plain_text, denied] {
        let mut cmd = command();
        cmd.arg("run").arg("--").arg(&executable);
        let output = Run::start(cmd).finish();
        assert_eq!(
            output.status.code(),
            Some(125),
            "{executable:?}: {output:?}"
        );
        assert!(!output.stderr.is_empty());
        assert!(!marker.try_exists().unwrap());
    }
}

#[test]
fn ordinary_exit_codes_are_preserved() {
    for code in [0, 1, 23, 125, 255] {
        let mut cmd = command();
        cmd.arg("run")
            .arg("--")
            .arg(fixture())
            .arg("exit")
            .arg(code.to_string());
        assert_eq!(Run::start(cmd).finish().status.code(), Some(code));
    }
}
