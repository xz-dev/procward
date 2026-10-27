// SPDX-License-Identifier: BSD-3-Clause
use crate::linux_inventory::Inventory;
use rustix::{
    fd::{AsRawFd, FromRawFd, OwnedFd},
    io::{self as rio, Errno, FdFlags},
    pipe::{PipeFlags, pipe_with},
    process::{self, Pid, Signal},
};
use signal_hook::{
    consts::{SIGCHLD, SIGHUP, SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::process::CommandExt,
    process::Command,
    thread,
    time::Duration,
};

pub fn run(command: &OsStr, args: &[OsString]) -> io::Result<u8> {
    process::set_child_subreaper(Some(Pid::INIT))?;
    let mut signals = Signals::new([SIGCHLD, SIGHUP, SIGINT, SIGTERM])?;
    let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
    let expected = process::getpid();
    let raw = read.as_raw_fd();
    let mut anchor = Command::new(std::env::current_exe()?);
    anchor
        .arg("__anchor")
        .arg(expected.to_string())
        .arg(raw.to_string())
        .arg(command)
        .args(args);
    anchor.process_group(0);
    // Only async-signal-safe OS calls run between fork and exec.
    unsafe {
        anchor.pre_exec(move || {
            arm_parent(expected)?;
            rio::fcntl_setfd(rustix::fd::BorrowedFd::borrow_raw(raw), FdFlags::empty())?;
            Ok(())
        });
    }
    let mut anchor = anchor.spawn()?;
    drop(read);
    rio::write(&write, b"R")?;
    let pid = Pid::from_raw(anchor.id() as i32).unwrap();
    loop {
        if let Some(status) = anchor.try_wait()? {
            drop(write);
            // Anchor loss is fatal even when it occurred before publishing any primary result.
            let cleanup = crate::linux::cleanup(&mut Inventory::new());
            cleanup?;
            return Ok(status.code().unwrap_or(125) as u8);
        }
        for signal in signals.pending().filter(|signal| *signal != SIGCHLD) {
            process::kill_process(pid, Signal::from_named_raw(signal).unwrap())?;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn arm_parent(expected: Pid) -> io::Result<()> {
    // SIGCONT is a targeted kernel wake even if this independent role was stopped.
    process::set_parent_process_death_signal(Some(Signal::CONT))?;
    if process::getppid() != Some(expected) {
        return Err(io::Error::other(
            "owner lost before parent-death wake was armed",
        ));
    }
    Ok(())
}

pub fn anchor(args: &[OsString]) -> io::Result<u8> {
    let [parent, fd, command, rest @ ..] = args else {
        return Err(io::Error::other("invalid private anchor invocation"));
    };
    let parent = parent
        .to_str()
        .and_then(|s| s.parse().ok())
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("invalid anchor parent"))?;
    let raw: i32 = fd
        .to_str()
        .and_then(|s| s.parse().ok())
        .filter(|fd| *fd > 2)
        .ok_or_else(|| io::Error::other("invalid anchor lease"))?;
    arm_parent(parent)?;
    // Validate descriptor before taking ownership; immediately restore close-on-exec.
    let borrowed = unsafe { rustix::fd::BorrowedFd::borrow_raw(raw) };
    rio::fcntl_setfd(borrowed, FdFlags::CLOEXEC)?;
    let lease = unsafe { OwnedFd::from_raw_fd(raw) };
    loop {
        let mut grant = [0];
        match rio::read(&lease, &mut grant) {
            Ok(1) if grant[0] == b'R' => break,
            Err(Errno::AGAIN | Errno::INTR) => thread::sleep(Duration::from_millis(1)),
            _ => return Err(io::Error::other("owner lost or invalid launch grant")),
        }
    }
    if !alive(&lease)? || process::getppid() != Some(parent) {
        return Err(io::Error::other("owner lost before payload launch"));
    }
    crate::linux::run_owned(command, rest, &lease)
}

pub fn alive(lease: impl rustix::fd::AsFd) -> io::Result<bool> {
    let mut byte = [0];
    match rio::read(lease, &mut byte) {
        Ok(0) => Ok(false),
        Err(Errno::AGAIN | Errno::INTR) => Ok(true),
        Ok(_) => Err(io::Error::other("unexpected native lease data")),
        Err(error) => Err(error.into()),
    }
}
