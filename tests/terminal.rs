// SPDX-License-Identifier: BSD-3-Clause
#![cfg(unix)]
use rustix::{
    fd::{AsRawFd, BorrowedFd},
    fs::{self, OFlags},
    io::{self as rio, Errno, FdFlags},
    process::{self, Pid, Signal},
};
use std::{
    fs as stdfs,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Terminal {
    dir: PathBuf,
    host: Child,
    master: rustix::fd::OwnedFd,
    output: String,
    sequence: usize,
}
impl Terminal {
    fn new(label: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("procward-terminal-{}-{label}", std::process::id()));
        stdfs::create_dir(&dir).unwrap();
        if label == "redirect" {
            stdfs::write(dir.join("redirect"), b"redirect").unwrap();
        }
        let pty = rustix_openpty::openpty(None, None).unwrap();
        if label == "blocked" {
            let mut state = rustix::termios::tcgetattr(&pty.user).unwrap();
            state.local_modes |= rustix::termios::LocalModes::TOSTOP;
            rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &state)
                .unwrap();
        }
        if label == "paused-loss" {
            stdfs::write(dir.join("keep"), b"hold shell alive").unwrap();
        }
        fs::fcntl_setfl(&pty.controller, OFlags::NONBLOCK).unwrap();
        let raw = pty.user.as_raw_fd();
        let fixture = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/fixture");
        let mut host = Command::new(fixture);
        host.arg("terminal-host")
            .arg(raw.to_string())
            .arg(env!("CARGO_BIN_EXE_procward"))
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        unsafe {
            host.pre_exec(move || {
                rio::fcntl_setfd(BorrowedFd::borrow_raw(raw), FdFlags::empty())?;
                Ok(())
            });
        }
        let host = host.spawn().unwrap();
        drop(pty.user);
        Self {
            dir,
            host,
            master: pty.controller,
            output: String::new(),
            sequence: 0,
        }
    }
    fn observe(&mut self) {
        loop {
            let mut bytes = [0; 8192];
            match rio::read(&self.master, &mut bytes) {
                Ok(0) | Err(Errno::AGAIN | Errno::IO) => return,
                Ok(count) => self
                    .output
                    .push_str(&String::from_utf8_lossy(&bytes[..count])),
                Err(Errno::INTR) => {}
                Err(error) => panic!("terminal read: {error}"),
            }
        }
    }
    fn until(&mut self, description: &str, check: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            self.observe();
            if check(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{description}: deadline; result={:?}; tail_written={}; physical_queue={:?}; bytes={}; transcript_tail={:?}",
                stdfs::read_to_string(self.dir.join("result")),
                self.dir.join("tail-written").exists(),
                rio::ioctl_fionread(&self.master),
                self.output.len(),
                String::from_utf8_lossy(
                    &self.output.as_bytes()[self.output.len().saturating_sub(4096)..]
                )
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn input(&mut self, bytes: &[u8]) {
        rio::write(&self.master, bytes).unwrap();
    }
    fn action(&mut self, text: &str) {
        let n = self.sequence;
        let record = self.dir.join("action.tmp");
        stdfs::write(&record, text).unwrap();
        stdfs::rename(record, self.dir.join(format!("action-{n}"))).unwrap();
        self.until("shell action acknowledgement", |t| {
            t.dir.join(format!("acted-{n}")).exists()
        });
        self.sequence += 1;
    }
    fn file(&self, name: &str) -> String {
        stdfs::read_to_string(self.dir.join(name)).unwrap()
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if self.host.try_wait().ok().flatten().is_none() {
            eprintln!("HARNESS RESCUE: terminal host/owned identities; not product cleanup");
            for name in ["primary", "owner"] {
                if let Ok(text) = stdfs::read_to_string(self.dir.join(name)) {
                    if let Some(pid) = text.parse().ok().and_then(Pid::from_raw) {
                        let _ = process::kill_process(pid, Signal::KILL);
                    }
                }
            }
            let _ = self.host.kill();
            let _ = self.host.wait();
        }
        let _ = stdfs::remove_dir_all(&self.dir);
    }
}

#[test]
fn interactive_primary_has_private_foreground_terminal_and_preserves_tail_and_status() {
    let mut terminal = Terminal::new("isolation");
    terminal.until("primary terminal readiness", |t| {
        t.dir.join("terminal").exists()
    });
    let facts: Vec<i32> = terminal
        .file("terminal")
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    let primary: i32 = terminal.file("primary").parse().unwrap();
    assert_eq!(facts[0], primary, "primary must lead private session");
    assert_eq!(
        facts[1], primary,
        "primary must own private foreground, not shell foreground"
    );
    assert_eq!(facts[2], primary, "primary must own private session");
    terminal.until("ready output", |t| t.output.contains("TTY-READY"));
    terminal.input(b"q");
    terminal.until("task result and final output", |t| {
        t.dir.join("result").exists() && t.output.contains("FINAL-TAIL")
    });
    assert_eq!(terminal.file("result"), "23");
    terminal.until("host exit", |t| t.dir.join("restored").exists());
    assert_eq!(
        terminal.file("restored"),
        "true",
        "saved={} actual={}",
        terminal.file("saved-state"),
        terminal.file("takeover-final")
    );
    assert!(terminal.host.wait().unwrap().success());
}

#[test]
fn shell_reclaim_does_not_block_fatal_cleanup_or_kill_shell() {
    let mut terminal = Terminal::new("reclaim");
    terminal.until("ready output", |t| t.output.contains("TTY-READY"));
    terminal.action("reclaim");
    let fault = Instant::now();
    terminal.action("term");
    terminal.until("cleanup after shell reclaim", |t| {
        t.dir.join("takeover-final").exists()
    });
    assert!(
        fault.elapsed() < Duration::from_secs(2),
        "configured deadline must bound reclaimed-terminal cleanup"
    );
    assert!(
        terminal.host.wait().unwrap().success(),
        "unrelated shell must survive"
    );
}

fn stopped(pid: &str) -> bool {
    #[cfg(target_os = "linux")]
    return stdfs::read_to_string(format!("/proc/{pid}/stat"))
        .unwrap()
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .next()
        == Some("T");
    #[cfg(not(target_os = "linux"))]
    {
        let state = Command::new("ps")
            .args(["-o", "state=", "-p", pid])
            .output()
            .unwrap();
        assert!(
            state.status.success(),
            "process state query failed: {state:?}"
        );
        String::from_utf8(state.stdout)
            .unwrap()
            .trim()
            .starts_with('T')
    }
}

#[test]
fn repeated_stop_background_foreground_freezes_detached_work_and_reconciles_cont_race() {
    let mut terminal = Terminal::new("job-control");
    terminal.until("primary and detached worker readiness", |t| {
        t.output.contains("TTY-READY") && t.dir.join("worker").exists()
    });
    for cycle in 0..3 {
        let _ = stdfs::remove_file(terminal.dir.join("stopped"));
        terminal.input(&[26]);
        terminal.until("shell sees stopped job", |t| t.dir.join("stopped").exists());
        assert_eq!(
            terminal.file("stopped"),
            "true",
            "pause must restore saved cooked settings; saved={} actual={}",
            terminal.file("saved-state"),
            terminal.file("paused-state")
        );
        terminal.until("primary and detached worker stopped", |t| {
            stopped(&t.file("primary")) && stopped(&t.file("worker"))
        });
        terminal.action("race");
        terminal.until("freeze wins detached CONT race without extra Ctrl-Z", |t| {
            stopped(&t.file("worker"))
        });
        terminal.action("bg");
        terminal.until("private work resumed in background", |t| {
            !stopped(&t.file("worker")) && !stopped(&t.file("primary"))
        });
        terminal.input(b"p");
        stdfs::write(terminal.dir.join(format!("ping-{cycle}")), b"ping").unwrap();
        terminal.until("background worker response", |t| {
            t.dir.join(format!("pong-{cycle}")).exists()
        });
        let previous = terminal.output.matches("REPLY").count();
        assert_eq!(
            previous, cycle,
            "background job must not steal physical input/output"
        );
        terminal.action("fg");
        terminal.until("foreground input/output restored", |t| {
            t.output.matches("REPLY").count() == cycle + 1
        });
    }
    terminal.input(b"q");
    terminal.until("result and tail", |t| {
        t.dir.join("result").exists() && t.output.contains("FINAL-TAIL")
    });
    assert_eq!(terminal.file("result"), "23");
    assert!(terminal.host.wait().unwrap().success());
}

#[test]
fn interactive_stderr_keeps_its_redirected_destination() {
    let mut terminal = Terminal::new("redirect");
    terminal.until("ready output", |t| t.output.contains("TTY-READY"));
    terminal.input(b"q");
    terminal.until("redirected result and output", |t| {
        t.dir.join("result").exists() && t.output.contains("FINAL-TAIL")
    });
    assert_eq!(terminal.file("result"), "23");
    assert_eq!(terminal.file("stderr"), "REDIRECTED-TAIL\n");
    assert!(!terminal.output.contains("REDIRECTED-TAIL"));
    assert!(terminal.host.wait().unwrap().success());
}

#[test]
fn interrupt_is_single_and_normal_final_tail_is_not_discarded() {
    let mut terminal = Terminal::new("tail");
    terminal.until("ready output", |t| t.output.contains("TTY-READY"));
    terminal.input(&[3]);
    terminal.input(b"p");
    terminal.until("single interrupt acknowledgement", |t| {
        t.output.contains("REPLY 1")
    });
    terminal.input(b"q");
    terminal.until("complete final tail and result", |t| {
        t.output.contains("FINAL-TAIL") && t.dir.join("result").exists()
    });
    assert_eq!(
        terminal.output.bytes().filter(|b| *b == b'x').count(),
        262144
    );
    assert_eq!(terminal.file("result"), "23");
    assert_eq!(
        terminal.file("restored"),
        "true",
        "saved={} actual={}",
        terminal.file("saved-state"),
        terminal.file("takeover-final")
    );
    assert!(terminal.host.wait().unwrap().success());
}

#[test]
fn fatal_cleanup_discards_blocked_output_and_respects_unrelated_terminal_authority() {
    let mut terminal = Terminal::new("blocked");
    terminal.until("ready output", |t| {
        t.output.contains("TTY-READY") && t.dir.join("worker").exists()
    });
    terminal.input(b"f");
    terminal.until("writer starts output", |t| {
        t.output.contains("FLOOD-BEGIN") && t.dir.join("flood-count").exists()
    });
    terminal.action("takeover");
    assert!(
        !terminal.dir.join("flood-done").exists(),
        "writer must still have pending output"
    );
    let fault = Instant::now();
    terminal.action("term");
    terminal.until("fatal cleanup despite blocked physical output", |t| {
        t.dir.join("takeover-final").exists()
    });
    assert!(
        fault.elapsed() < Duration::from_secs(2),
        "configured teardown deadline"
    );
    assert_eq!(
        terminal.file("takeover-final"),
        terminal.file("takeover"),
        "denied restoration must not overwrite unrelated foreground state"
    );
    assert!(terminal.host.wait().unwrap().success());
}

fn absent(pid: &str) -> bool {
    let target = Pid::from_raw(pid.parse().unwrap()).unwrap();
    match process::waitpid(Some(target), process::WaitOptions::NOHANG) {
        Ok(Some((_, status))) => {
            assert!(status.exit_status().is_some() || status.terminating_signal().is_some())
        }
        Ok(None) | Err(Errno::CHILD | Errno::INTR) => {}
        Err(error) => panic!("owned-child wait failed: {error}"),
    }
    match process::test_kill_process(target) {
        Err(Errno::SRCH) => true,
        Ok(()) => false,
        Err(error) => panic!("liveness query failed: {error}"),
    }
}

#[test]
fn owner_loss_while_paused_cleans_work_without_foreground_rescue() {
    let mut terminal = Terminal::new("paused-loss");
    terminal.until("primary and worker ready", |t| {
        t.output.contains("TTY-READY") && t.dir.join("worker").exists()
    });
    terminal.input(&[26]);
    terminal.until("job stopped", |t| {
        t.dir.join("stopped").exists() && stopped(&t.file("worker"))
    });
    terminal.action("takeover");
    let fault = Instant::now();
    terminal.action("kill");
    terminal.until("tree and independent cleanup role gone", |t| {
        ["primary", "worker", "anchor"]
            .iter()
            .all(|name| absent(&t.file(name)))
    });
    assert!(
        fault.elapsed() < Duration::from_secs(2),
        "configured teardown deadline"
    );
    stdfs::write(terminal.dir.join("finish"), b"complete").unwrap();
    assert!(terminal.host.wait().unwrap().success());
    assert_eq!(terminal.file("takeover-final"), terminal.file("takeover"));
}
