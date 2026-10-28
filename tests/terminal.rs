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
        let pty = rustix_openpty::openpty(None, None).unwrap();
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
                "{description}: deadline; transcript={:?}",
                self.output
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn input(&mut self, bytes: &[u8]) {
        rio::write(&self.master, bytes).unwrap();
    }
    fn action(&mut self, text: &str) {
        let n = self.sequence;
        stdfs::write(self.dir.join(format!("action-{n}")), text).unwrap();
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
    assert_eq!(terminal.file("restored"), "true");
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
        t.dir.join("result").exists()
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
