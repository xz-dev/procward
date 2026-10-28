// SPDX-License-Identifier: BSD-3-Clause
use rustix::{
    fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    fs::{self, Mode, OFlags},
    io::{self as rio, Errno, FdFlags},
    process,
    termios::{self, OptionalActions, Termios},
};
use std::{
    ffi::{CString, OsStr, OsString},
    io,
    os::unix::{ffi::OsStrExt, process::CommandExt},
    process::Command,
};

pub fn interactive() -> io::Result<bool> {
    let input = unsafe { BorrowedFd::borrow_raw(0) };
    let output = unsafe { BorrowedFd::borrow_raw(1) };
    if !termios::isatty(input) || !termios::isatty(output) {
        return Ok(false);
    }
    let a = fs::fstat(input)?;
    let b = fs::fstat(output)?;
    Ok(a.st_rdev == b.st_rdev && termios::tcgetsid(input)? == process::getsid(None)?)
}

pub struct Terminal {
    physical: OwnedFd,
    saved: Termios,
    master: OwnedFd,
    user: Option<OwnedFd>,
    input: Vec<u8>,
    output: Vec<u8>,
    eof: bool,
    raw: bool,
    pause: bool,
}
impl Terminal {
    pub fn open() -> io::Result<Option<Self>> {
        if !interactive()? {
            return Ok(None);
        }
        let physical = fs::open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?;
        let saved = termios::tcgetattr(&physical)?;
        let size = termios::tcgetwinsize(&physical)?;
        let pty = rustix_openpty::openpty(Some(&saved), Some(&size))?;
        fs::fcntl_setfl(&pty.controller, OFlags::NONBLOCK)?;
        Ok(Some(Self {
            physical,
            saved,
            master: pty.controller,
            user: Some(pty.user),
            input: Vec::new(),
            output: Vec::new(),
            eof: false,
            raw: false,
            pause: false,
        }))
    }
    pub fn command(&mut self, command: &OsStr, args: &[OsString]) -> io::Result<Command> {
        // signal-hook uses SA_RESTART; tty authority denial must return, not loop on SIGTTOU.
        unsafe {
            let mut action = std::mem::zeroed();
            if libc::sigaction(libc::SIGTTOU, std::ptr::null(), &mut action) != 0 {
                return Err(io::Error::last_os_error());
            }
            action.sa_flags &= !libc::SA_RESTART;
            if libc::sigaction(libc::SIGTTOU, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let raw = self.user.as_ref().unwrap().as_raw_fd();
        let mut launch = Command::new(std::env::current_exe()?);
        launch
            .arg("__terminal")
            .arg(raw.to_string())
            .arg(command)
            .args(args);
        // Trusted bootstrap alone performs login_tty; no ProcessGroup/ProcessSession is stacked.
        unsafe {
            launch.pre_exec(move || {
                rio::fcntl_setfd(BorrowedFd::borrow_raw(raw), FdFlags::empty())?;
                Ok(())
            });
        }
        Ok(launch)
    }
    pub fn launched(&mut self) {
        self.user.take();
    }
    pub fn foreground(&self) -> io::Result<bool> {
        Ok(termios::tcgetpgrp(&self.physical)? == process::getpgrp())
    }
    pub fn pump(&mut self) -> io::Result<()> {
        if !self.foreground()? {
            return Ok(());
        }
        if !self.raw {
            let mut raw = self.saved.clone();
            raw.make_raw();
            termios::tcsetattr(&self.physical, OptionalActions::Now, &raw)?;
            self.raw = true;
        }
        termios::tcsetwinsize(&self.master, termios::tcgetwinsize(&self.physical)?)?;
        read_buffer(&self.physical, &mut self.input, false)?;
        let suspend = self.saved.special_codes[termios::SpecialCodeIndex::VSUSP];
        if suspend != 0 && self.input.contains(&suspend) {
            self.input.retain(|byte| *byte != suspend);
            self.pause = true;
            return Ok(());
        }
        write_buffer(&self.master, &mut self.input)?;
        self.eof |= read_buffer(&self.master, &mut self.output, true)?;
        write_buffer(&self.physical, &mut self.output)?;
        Ok(())
    }
    pub fn pause_requested(&mut self) -> bool {
        std::mem::take(&mut self.pause)
    }
    pub fn drained(&self) -> bool {
        self.eof && self.output.is_empty()
    }
    pub fn restore(&mut self) -> io::Result<()> {
        if self.raw {
            // Kernel decides authority. No equality shortcut or TTIN/TTOU masking.
            let result = termios::tcsetattr(&self.physical, OptionalActions::Now, &self.saved);
            self.raw = false;
            if let Err(error) = result {
                if !self.foreground()? {
                    eprintln!("procward: terminal restoration denied: {error}");
                } else {
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("procward: terminal restoration denied: {error}");
        }
    }
}
fn read_buffer(fd: &OwnedFd, target: &mut Vec<u8>, pty: bool) -> io::Result<bool> {
    if target.len() >= 65536 {
        return Ok(false);
    }
    let mut bytes = [0; 4096];
    match rio::read(fd, &mut bytes) {
        Ok(0) if pty => Ok(true),
        Ok(0) => Err(io::Error::other("terminal input ended")),
        Ok(count) => {
            target.extend_from_slice(&bytes[..count]);
            Ok(false)
        }
        Err(Errno::IO) if pty => Ok(true),
        Err(Errno::AGAIN | Errno::INTR) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn write_buffer(fd: &OwnedFd, source: &mut Vec<u8>) -> io::Result<()> {
    if !source.is_empty() {
        match rio::write(fd, source) {
            Ok(count) => {
                source.drain(..count);
            }
            Err(Errno::AGAIN | Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn exec(args: &[OsString]) -> io::Result<u8> {
    let [raw, command, rest @ ..] = args else {
        return Err(io::Error::other("invalid terminal bootstrap"));
    };
    let raw = raw
        .to_str()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| *v > 2)
        .ok_or_else(|| io::Error::other("invalid terminal endpoint"))?;
    let fd = unsafe { BorrowedFd::borrow_raw(raw) };
    termios::tcgetattr(fd)?;
    let stderr = unsafe { BorrowedFd::borrow_raw(2) };
    let stdout = unsafe { BorrowedFd::borrow_raw(1) };
    let redirected =
        !termios::isatty(stderr) || fs::fstat(stderr)?.st_rdev != fs::fstat(stdout)?.st_rdev;
    let saved_stderr = if redirected {
        Some(rio::fcntl_dupfd_cloexec(stderr, 3)?)
    } else {
        None
    };
    rustix_openpty::login_tty(unsafe { OwnedFd::from_raw_fd(raw) })?;
    if let Some(stderr) = saved_stderr {
        rustix::stdio::dup2_stderr(stderr)?;
    }
    // Native handlers/masks and shell-ignored job-control dispositions must not reach payload.
    unsafe {
        let mut empty = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        for signal in [libc::SIGTTIN, libc::SIGTTOU, libc::SIGTSTP] {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
    exec_payload(command, rest)
}

fn exec_payload(command: &OsStr, args: &[OsString]) -> io::Result<u8> {
    let cstring = |v: &OsStr| {
        CString::new(v.as_bytes()).map_err(|_| io::Error::other("NUL in command/environment"))
    };
    let argv: Vec<CString> = std::iter::once(command)
        .chain(args.iter().map(OsString::as_os_str))
        .map(cstring)
        .collect::<io::Result<_>>()?;
    let env: Vec<CString> = std::env::vars_os()
        .map(|(key, value)| {
            let mut pair = key;
            pair.push("=");
            pair.push(value);
            cstring(&pair)
        })
        .collect::<io::Result<_>>()?;
    let pointers = |strings: &[CString]| {
        strings
            .iter()
            .map(|v| v.as_ptr())
            .chain([std::ptr::null()])
            .collect::<Vec<_>>()
    };
    let argv_ptr = pointers(&argv);
    let env_ptr = pointers(&env);
    let paths = if command.as_bytes().contains(&b'/') {
        vec![std::path::PathBuf::from(command)]
    } else {
        std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/bin:/usr/bin")),
        )
        .map(|dir| dir.join(command))
        .collect()
    };
    let mut denied = false;
    for path in paths {
        // execve (not execvp) preserves the no-ENOEXEC-shell-fallback contract on every Unix.
        unsafe {
            libc::execve(
                cstring(path.as_os_str())?.as_ptr(),
                argv_ptr.as_ptr(),
                env_ptr.as_ptr(),
            );
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTDIR) => {}
            Some(libc::EACCES) => denied = true,
            _ => return Err(error),
        }
    }
    Err(io::Error::from_raw_os_error(if denied {
        libc::EACCES
    } else {
        libc::ENOENT
    }))
}
