// SPDX-License-Identifier: BSD-3-Clause
use rustix::{
    fd::{BorrowedFd, FromRawFd, OwnedFd},
    io::{self as rio},
    process::{self as rp, Pid, Signal, WaitOptions},
    termios,
};
use std::{
    env, fs,
    io::Write,
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub fn host(fd: &std::ffi::OsStr, binary: &std::ffi::OsStr, dir: &Path) {
    let fd = fd.to_str().unwrap().parse::<i32>().unwrap();
    rustix_openpty::login_tty(unsafe { OwnedFd::from_raw_fd(fd) }).unwrap();
    let tty = unsafe { BorrowedFd::borrow_raw(0) };
    let saved = termios::tcgetattr(tty).unwrap();
    publish(dir.join("saved-state"), format!("{saved:?}")).unwrap();
    let mut child = Command::new(binary);
    child
        .args([
            "run",
            "--grace",
            "500ms",
            "--fatal-grace",
            "250ms",
            "--cleanup-deadline",
            "1500ms",
            "--",
        ])
        .arg(env::current_exe().unwrap())
        .arg("terminal-child")
        .arg(dir)
        .process_group(0);
    if dir.join("redirect").exists() {
        child.stderr(fs::File::create(dir.join("stderr")).unwrap());
    }
    unsafe {
        child.pre_exec(|| {
            for signal in [libc::SIGTTIN, libc::SIGTTOU] {
                libc::signal(signal, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    #[expect(
        clippy::zombie_processes,
        reason = "waitpid below consumes both stopped and terminal child statuses"
    )]
    let child = child.spawn().unwrap();
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    publish(dir.join("owner"), child.id().to_string()).unwrap();
    // TTOU denial must not be hidden in production; shell fixture uses its normal shell policy.
    unsafe {
        libc::signal(signal_hook::consts::SIGTTOU, libc::SIG_IGN);
    }
    termios::tcsetpgrp(tty, pid).unwrap();
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut sequence = 0;
    loop {
        if let Some((_, status)) =
            rp::waitpid(Some(pid), WaitOptions::NOHANG | WaitOptions::UNTRACED).unwrap()
        {
            if status.stopped() {
                termios::tcsetpgrp(tty, rp::getpgrp()).unwrap();
                publish(
                    dir.join("paused-state"),
                    format!("{:?}", termios::tcgetattr(tty).unwrap()),
                )
                .unwrap();
                publish(
                    dir.join("stopped"),
                    format!(
                        "{}",
                        format!("{:?}", termios::tcgetattr(tty).unwrap()) == format!("{saved:?}")
                    ),
                )
                .unwrap();
            } else if let Some(code) = status
                .exit_status()
                .or_else(|| status.terminating_signal().map(|s| 128 + s))
            {
                termios::tcsetpgrp(tty, rp::getpgrp()).unwrap();
                publish(dir.join("result"), code.to_string()).unwrap();
                publish(
                    dir.join("restored"),
                    format!(
                        "{}",
                        format!("{:?}", termios::tcgetattr(tty).unwrap()) == format!("{saved:?}")
                    ),
                )
                .unwrap();
                publish(
                    dir.join("takeover-final"),
                    format!("{:?}", termios::tcgetattr(tty).unwrap()),
                )
                .unwrap();
                if dir.join("keep").exists() {
                    while !dir.join("finish").exists() {
                        assert!(Instant::now() < deadline, "shell hold deadline");
                        thread::sleep(Duration::from_millis(2));
                    }
                }
                return;
            }
        }
        let action = dir.join(format!("action-{sequence}"));
        if let Ok(text) = fs::read_to_string(&action) {
            match text.as_str() {
                "bg" => rp::kill_process_group(pid, Signal::CONT).unwrap(),
                "fg" => {
                    termios::tcsetpgrp(tty, pid).unwrap();
                    rp::kill_process_group(pid, Signal::CONT).unwrap();
                }
                "reclaim" => termios::tcsetpgrp(tty, rp::getpgrp()).unwrap(),
                "takeover" => {
                    termios::tcsetpgrp(tty, rp::getpgrp()).unwrap();
                    let mut state = termios::tcgetattr(tty).unwrap();
                    state.local_modes ^= termios::LocalModes::ECHO;
                    termios::tcsetattr(tty, termios::OptionalActions::Now, &state).unwrap();
                    publish(dir.join("takeover"), format!("{state:?}")).unwrap();
                }
                "race" => {
                    let worker = fs::read_to_string(dir.join("worker"))
                        .unwrap()
                        .parse::<i32>()
                        .unwrap();
                    rp::kill_process(Pid::from_raw(worker).unwrap(), Signal::CONT).unwrap();
                }
                "term" => rp::kill_process(pid, Signal::TERM).unwrap(),
                "kill" => rp::kill_process(pid, Signal::KILL).unwrap(),
                _ => panic!("unknown terminal action"),
            }
            publish(dir.join(format!("acted-{sequence}")), b"done").unwrap();
            sequence += 1;
        }
        assert!(Instant::now() < deadline, "terminal host deadline");
        thread::sleep(Duration::from_millis(2));
    }
}

pub fn child(dir: &Path) {
    let stdin = unsafe { BorrowedFd::borrow_raw(0) };
    let group = rp::getpgrp();
    let tty = rustix::fs::open(
        "/dev/tty",
        rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    let terminal = termios::tcgetpgrp(&tty).unwrap();
    publish(dir.join("primary"), std::process::id().to_string()).unwrap();
    publish(dir.join("anchor"), rp::getppid().unwrap().to_string()).unwrap();
    publish(
        dir.join("terminal"),
        format!(
            "{} {} {}",
            group,
            terminal,
            termios::tcgetsid(&tty).unwrap()
        ),
    )
    .unwrap();
    let mut state = termios::tcgetattr(stdin).unwrap();
    state.make_raw();
    termios::tcsetattr(stdin, termios::OptionalActions::Now, &state).unwrap();
    let interrupts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = interrupts.clone();
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
    }
    #[expect(
        clippy::zombie_processes,
        reason = "fixture leaves detached descendant for procward cleanup"
    )]
    let _worker = Command::new(env::current_exe().unwrap())
        .arg("terminal-worker")
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    publish(dir.join("ready"), b"ready").unwrap();
    println!("TTY-READY");
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    loop {
        match rio::read(stdin, &mut byte) {
            Ok(1) => match byte[0] {
                b'q' => {
                    if dir.to_string_lossy().ends_with("tail") {
                        std::io::stdout().write_all(&vec![b'x'; 262144]).unwrap();
                    }
                    println!("FINAL-TAIL");
                    eprintln!("REDIRECTED-TAIL");
                    std::process::exit(23);
                }
                b'f' => {
                    println!("FLOOD-BEGIN");
                    std::io::stdout().flush().unwrap();
                    for n in 1..16384 {
                        std::io::stdout().write_all(&[b'x'; 4096]).unwrap();
                        publish(dir.join("flood-count"), n.to_string()).unwrap();
                    }
                    publish(dir.join("flood-done"), b"done").unwrap();
                }
                b'p' => {
                    println!(
                        "REPLY {}",
                        interrupts.load(std::sync::atomic::Ordering::SeqCst)
                    );
                    std::io::stdout().flush().unwrap();
                }
                3 => {
                    rp::kill_process(rp::getpid(), Signal::INT).unwrap();
                }
                26 => {
                    rp::kill_process(rp::getpid(), Signal::TSTP).unwrap();
                }
                _ => {}
            },
            Err(rustix::io::Errno::INTR) => {}
            other => panic!("terminal input: {other:?}"),
        }
    }
}

pub fn worker(dir: &Path) {
    rp::setsid().unwrap();
    publish(dir.join("worker"), std::process::id().to_string()).unwrap();
    let mut sequence = 0;
    loop {
        if dir.join(format!("ping-{sequence}")).exists() {
            publish(dir.join(format!("pong-{sequence}")), b"alive").unwrap();
            sequence += 1;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn publish(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}
