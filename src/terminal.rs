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

pub struct SavedTerminal {
    physical: OwnedFd,
    saved: Termios,
}
impl SavedTerminal {
    pub fn open() -> io::Result<Self> {
        let physical = fs::open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?;
        let saved = termios::tcgetattr(&physical)?;
        Ok(Self { physical, saved })
    }
    pub fn restore(&self) -> io::Result<()> {
        // Kernel decides authority. No equality shortcut or TTIN/TTOU masking.
        if let Err(error) = termios::tcsetattr(&self.physical, OptionalActions::Now, &self.saved) {
            if termios::tcgetpgrp(&self.physical)? == process::getpgrp() {
                return Err(error.into());
            }
        }
        Ok(())
    }
}

pub struct Terminal {
    state: SavedTerminal,
    master: OwnedFd,
    user: Option<OwnedFd>,
    input: Vec<u8>,
    output: Vec<u8>,
    eof: bool,
    raw: bool,
    pause: bool,
    size: termios::Winsize,
    control: Option<std::os::unix::net::UnixStream>,
    result_bytes: Vec<u8>,
    released: bool,
}
impl Terminal {
    pub fn open() -> io::Result<Option<Self>> {
        if !interactive()? {
            return Ok(None);
        }
        let state = SavedTerminal::open()?;
        let size = termios::tcgetwinsize(&state.physical)?;
        let pty = rustix_openpty::openpty(Some(&state.saved), Some(&size))?;
        fs::fcntl_setfl(&pty.controller, OFlags::NONBLOCK)?;
        Ok(Some(Self {
            state,
            master: pty.controller,
            user: Some(pty.user),
            input: Vec::new(),
            output: Vec::new(),
            eof: false,
            raw: false,
            pause: false,
            size,
            control: None,
            result_bytes: Vec::new(),
            released: false,
        }))
    }
    pub fn command(
        &mut self,
        command: &OsStr,
        args: &[OsString],
        deadline: std::time::Duration,
    ) -> io::Result<Command> {
        interrupt_job_control()?;
        let raw = self.user.as_ref().unwrap().as_raw_fd();
        let (control, peer) = std::os::unix::net::UnixStream::pair()?;
        control.set_nonblocking(true)?;
        peer.set_nonblocking(true)?;
        let peer_raw = peer.as_raw_fd();
        self.control = Some(control);
        let mut launch = Command::new(std::env::current_exe()?);
        launch
            .arg("__terminal")
            .arg(raw.to_string())
            .arg(peer_raw.to_string())
            .arg(deadline.as_millis().to_string())
            .arg(command)
            .args(args);
        // Trusted bootstrap alone performs login_tty; no ProcessGroup/ProcessSession is stacked.
        // The command owns the peer until spawn completes, then only the private keeper inherits it.
        unsafe {
            launch.pre_exec(move || {
                let _keep_peer = &peer;
                for fd in [raw, peer_raw] {
                    rio::fcntl_setfd(BorrowedFd::borrow_raw(fd), FdFlags::empty())?;
                }
                Ok(())
            });
        }
        Ok(launch)
    }
    pub fn result(&mut self) -> io::Result<Option<u8>> {
        use std::io::Read;
        if self.result_bytes.len() < 2 {
            let mut bytes = [0; 3];
            match self.control.as_mut().unwrap().read(&mut bytes) {
                Ok(0) => {
                    return Err(io::Error::other(
                        "terminal keeper closed before primary result",
                    ));
                }
                Ok(count) => self.result_bytes.extend_from_slice(&bytes[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        match self.result_bytes.as_slice() {
            [] | [b'X'] => Ok(None),
            [b'X', code] => Ok(Some(*code)),
            _ => Err(io::Error::other("invalid terminal primary result")),
        }
    }
    pub fn release_if_drained(&mut self) -> io::Result<()> {
        use std::io::Write;
        if self.released || !self.output.is_empty() || rio::ioctl_fionread(&self.master)? != 0 {
            return Ok(());
        }
        let mut queued: libc::c_int = 0;
        if unsafe {
            libc::ioctl(
                self.user.as_ref().unwrap().as_raw_fd(),
                libc::TIOCOUTQ,
                &mut queued,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if queued == 0 {
            self.control.as_mut().unwrap().write_all(b"D")?;
            self.released = true;
        }
        Ok(())
    }
    pub fn finish_output(&mut self) {
        self.user.take();
    }
    pub fn foreground(&self) -> io::Result<bool> {
        Ok(termios::tcgetpgrp(&self.state.physical)? == process::getpgrp())
    }
    pub fn pump(&mut self) -> io::Result<()> {
        if !self.foreground()? {
            return Ok(());
        }
        if !self.raw {
            let mut raw = self.state.saved.clone();
            raw.make_raw();
            termios::tcsetattr(&self.state.physical, OptionalActions::Now, &raw)?;
            self.raw = true;
        }
        let size = termios::tcgetwinsize(&self.state.physical)?;
        if (size.ws_row, size.ws_col, size.ws_xpixel, size.ws_ypixel)
            != (
                self.size.ws_row,
                self.size.ws_col,
                self.size.ws_xpixel,
                self.size.ws_ypixel,
            )
        {
            termios::tcsetwinsize(&self.master, size)?;
            self.size = size;
        }
        read_buffer(&self.state.physical, &mut self.input, false)?;
        let suspend = self.state.saved.special_codes[termios::SpecialCodeIndex::VSUSP];
        if suspend != 0 && self.input.contains(&suspend) {
            self.input.retain(|byte| *byte != suspend);
            self.pause = true;
            return Ok(());
        }
        write_buffer(&self.master, &mut self.input)?;
        self.drain()
    }
    pub fn drain(&mut self) -> io::Result<()> {
        if self.foreground()? {
            // Batch up to 64 KiB per turn without starving ownership/signal checks.
            let mut emitted = 0;
            for _ in 0..16 {
                let (eof, read) = read_buffer(&self.master, &mut self.output, true)?;
                self.eof |= eof;
                let written = write_buffer(&self.state.physical, &mut self.output)?;
                emitted += written;
                if (read == 0 && written == 0) || emitted >= 65536 {
                    break;
                }
            }
        }
        Ok(())
    }
    pub fn wait(&self, lease: &OwnedFd, active: bool) -> io::Result<()> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let mut fds = vec![
            PollFd::new(lease, PollFlags::IN),
            PollFd::new(self.control.as_ref().unwrap(), PollFlags::IN),
        ];
        if active && self.foreground()? {
            if self.output.len() < 65536 {
                fds.push(PollFd::new(&self.master, PollFlags::IN));
            }
            if !self.output.is_empty() {
                fds.push(PollFd::new(&self.state.physical, PollFlags::OUT));
            }
            if self.result_bytes.is_empty() {
                fds.push(PollFd::new(&self.state.physical, PollFlags::IN));
                if !self.input.is_empty() {
                    fds.push(PollFd::new(&self.master, PollFlags::OUT));
                }
            }
        }
        match poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 5_000_000,
            }),
        ) {
            Ok(_) | Err(Errno::INTR) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn pause_requested(&mut self) -> bool {
        std::mem::take(&mut self.pause)
    }
    pub fn drained(&self) -> bool {
        self.eof && self.output.is_empty()
    }
    pub fn restore(&mut self) -> io::Result<()> {
        if self.raw {
            self.raw = false;
            self.state.restore()?;
        }
        Ok(())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
fn read_buffer(fd: &OwnedFd, target: &mut Vec<u8>, pty: bool) -> io::Result<(bool, usize)> {
    if target.len() >= 65536 {
        return Ok((false, 0));
    }
    let mut bytes = [0; 4096];
    let size = bytes.len().min(65536 - target.len());
    match rio::read(fd, &mut bytes[..size]) {
        Ok(0) if pty => Ok((true, 0)),
        Ok(0) => Err(io::Error::other("terminal input ended")),
        Ok(count) => {
            target.extend_from_slice(&bytes[..count]);
            Ok((false, count))
        }
        Err(Errno::IO) if pty => Ok((true, 0)),
        Err(Errno::AGAIN | Errno::INTR) => Ok((false, 0)),
        Err(error) => Err(error.into()),
    }
}
fn write_buffer(fd: &OwnedFd, source: &mut Vec<u8>) -> io::Result<usize> {
    if !source.is_empty() {
        match rio::write(fd, source) {
            Ok(count) => {
                source.drain(..count);
                return Ok(count);
            }
            Err(Errno::AGAIN | Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(0)
}

pub fn exec(args: &[OsString]) -> io::Result<u8> {
    let [raw, control, deadline, command, rest @ ..] = args else {
        return Err(io::Error::other("invalid terminal bootstrap"));
    };
    let raw = raw
        .to_str()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| *v > 2)
        .ok_or_else(|| io::Error::other("invalid terminal endpoint"))?;
    let control_raw = control
        .to_str()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| *v > 2 && *v != raw)
        .ok_or_else(|| io::Error::other("invalid terminal control endpoint"))?;
    let control_fd = unsafe { BorrowedFd::borrow_raw(control_raw) };
    if rustix::net::sockopt::socket_type(control_fd)? != rustix::net::SocketType::STREAM
        || rustix::net::getsockname(control_fd)?.address_family()
            != rustix::net::AddressFamily::UNIX
    {
        return Err(io::Error::other("invalid terminal control transport"));
    }
    rio::fcntl_setfd(control_fd, FdFlags::CLOEXEC)?;
    let control = unsafe { std::os::unix::net::UnixStream::from_raw_fd(control_raw) };
    control.set_nonblocking(true)?;
    let deadline = deadline
        .to_str()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(std::time::Duration::from_millis)
        .ok_or_else(|| io::Error::other("invalid terminal drain deadline"))?;
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
    run_keeper(control, deadline, command, rest)
}

fn reset_payload_signals() -> io::Result<()> {
    // Native handlers/masks and shell-ignored job-control dispositions must not reach payload.
    unsafe {
        let mut empty = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        for signal in [libc::SIGTTIN, libc::SIGTTOU, libc::SIGTSTP, libc::SIGPIPE] {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
    Ok(())
}

pub fn exec_ready(args: &[OsString]) -> io::Result<u8> {
    let [grant, command, rest @ ..] = args else {
        return Err(io::Error::other("invalid terminal launch invocation"));
    };
    let grant = crate::owner::inherited_fd(grant, OFlags::RDONLY)?;
    let expected = std::env::var("PROCWARD_OWNER_PID")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .and_then(process::Pid::from_raw)
        .ok_or_else(|| io::Error::other("invalid terminal launch parent"))?;
    loop {
        let mut byte = [0];
        match rio::read(&grant, &mut byte) {
            Ok(1) if byte[0] == b'R' => break,
            Err(Errno::AGAIN | Errno::INTR) => {
                std::thread::sleep(std::time::Duration::from_millis(1))
            }
            _ => return Err(io::Error::other("terminal launch permission ended")),
        }
    }
    if process::getppid() != Some(expected) {
        return Err(io::Error::other("terminal launch parent lost"));
    }
    drop(grant);
    reset_payload_signals()?;
    let result = exec_payload(command, rest);
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    result
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

fn run_keeper(
    mut control: std::os::unix::net::UnixStream,
    deadline: std::time::Duration,
    command: &OsStr,
    args: &[OsString],
) -> io::Result<u8> {
    use process_wrap::std::{CommandWrap, ProcessGroup};
    use signal_hook::{
        consts::{SIGCHLD, SIGHUP, SIGINT, SIGTERM, SIGTTIN, SIGTTOU},
        iterator::Signals,
    };
    use std::{
        io::{Read, Write},
        os::unix::process::ExitStatusExt,
        time::Instant,
    };
    let mut signals = Signals::new([SIGCHLD, SIGHUP, SIGINT, SIGTERM, SIGTTIN, SIGTTOU])?;
    let (grant_read, grant_write) = crate::owner::pipe()?;
    let raw = grant_read.as_raw_fd();
    let mut handoff = crate::contract::Handoff::new()?;
    let mut payload = Command::new(std::env::current_exe()?);
    payload
        .arg("__terminal_exec")
        .arg(raw.to_string())
        .arg(command)
        .args(args);
    crate::contract::clear(&mut payload);
    payload.envs(handoff.environment()?);
    unsafe {
        payload.pre_exec(move || {
            let _keep_grant = &grant_read;
            rio::fcntl_setfd(BorrowedFd::borrow_raw(raw), FdFlags::empty())?;
            Ok(())
        });
    }
    let mut child = CommandWrap::from(payload)
        .wrap(ProcessGroup::leader())
        .spawn()?;
    let primary = process::Pid::from_raw(child.id() as i32).unwrap();
    termios::tcsetpgrp(unsafe { BorrowedFd::borrow_raw(0) }, primary)?;
    handoff.ready(child.id())?;
    let mut byte = [0];
    match control.read(&mut byte) {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        _ => return Err(io::Error::other("terminal relay lost before launch grant")),
    }
    if signals
        .pending()
        .any(|signal| matches!(signal, SIGHUP | SIGINT | SIGTERM))
    {
        return Err(io::Error::other("terminal launch cancelled"));
    }
    rio::write(&grant_write, b"R")?;
    drop(grant_write);
    let mut result = None;
    let mut closing = false;
    let mut started: Option<Instant> = None;
    loop {
        if result.is_none() {
            if let Some(status) = child.inner_mut().try_wait()? {
                let code = status
                    .code()
                    .or_else(|| status.signal().map(|v| 128 + v))
                    .ok_or_else(|| io::Error::other("terminal primary result missing"))?
                    as u8;
                control.write_all(&[b'X', code])?;
                result = Some(code);
                started.get_or_insert_with(Instant::now);
            }
        }
        let request = signals
            .pending()
            .find(|signal| matches!(*signal, SIGHUP | SIGINT | SIGTERM));
        let mut drained = false;
        let relay_alive = match control.read(&mut byte) {
            Ok(0) => false,
            Ok(1) if byte[0] == b'D' && result.is_some() => {
                drained = true;
                true
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                true
            }
            Err(error) => return Err(error),
            _ => return Err(io::Error::other("invalid terminal drain acknowledgement")),
        };
        if !closing && (!relay_alive || request.is_some()) {
            closing = true;
            started.get_or_insert_with(Instant::now);
            handoff.owner_lost()?;
            if result.is_none() {
                let mut forward = Vec::new();
                if let Some(signal) = request.filter(|v| *v != SIGTERM) {
                    forward.push(process::Signal::from_named_raw(signal).unwrap());
                }
                forward.extend([process::Signal::TERM, process::Signal::CONT]);
                for signal in forward {
                    match process::kill_process_group(primary, signal) {
                        Ok(()) | Err(Errno::SRCH) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        if let Some(code) = result {
            if drained || closing {
                return Ok(code);
            }
        }
        if started.is_some_and(|start| start.elapsed() >= deadline) {
            return Err(io::Error::other("terminal drain deadline expired"));
        }
        if relay_alive {
            let mut readiness = [rustix::event::PollFd::new(
                &control,
                rustix::event::PollFlags::IN,
            )];
            match rustix::event::poll(
                &mut readiness,
                Some(&rustix::event::Timespec {
                    tv_sec: 0,
                    tv_nsec: 2_000_000,
                }),
            ) {
                Ok(_) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

pub(crate) fn interrupt_job_control() -> io::Result<()> {
    // signal-hook uses SA_RESTART; tty authority denial must return to ownership checks.
    unsafe {
        for signal in [libc::SIGTTIN, libc::SIGTTOU] {
            let mut action = std::mem::zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut action) != 0 {
                return Err(io::Error::last_os_error());
            }
            action.sa_flags &= !libc::SA_RESTART;
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn background_read_after_foreground_reclaim_returns_to_cleanup() {
        let test = "terminal::tests::background_read_after_foreground_reclaim_returns_to_cleanup";
        let child_command = || {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", test, "--nocapture"]);
            command
        };
        if let Some(dir) = std::env::var_os("PROCWARD_READ_RACE") {
            let dir = std::path::PathBuf::from(dir);
            if std::env::var_os("PROCWARD_BACKGROUND_READER").is_some() {
                let mut signals =
                    signal_hook::iterator::Signals::new([libc::SIGTTIN, libc::SIGTTOU]).unwrap();
                interrupt_job_control().unwrap();
                let physical =
                    rustix::io::fcntl_dupfd_cloexec(unsafe { BorrowedFd::borrow_raw(0) }, 3)
                        .unwrap();
                while termios::tcgetpgrp(&physical).unwrap() != process::getpgrp() {
                    thread::yield_now();
                }
                fs::write(dir.join("foreground"), b"checked before read").unwrap();
                while !dir.join("reclaimed").exists() {
                    thread::yield_now();
                }
                let mut input = Vec::new();
                assert_eq!(
                    read_buffer(&physical, &mut input, false).unwrap(),
                    (false, 0)
                );
                assert!(signals.pending().any(|signal| signal == libc::SIGTTIN));
                return;
            }
            rustix_openpty::login_tty(
                rustix::io::fcntl_dupfd_cloexec(unsafe { BorrowedFd::borrow_raw(0) }, 3).unwrap(),
            )
            .unwrap();
            // Shell fixture alone may change foreground while in background.
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            }
            let mut reader = child_command()
                .env("PROCWARD_BACKGROUND_READER", "1")
                .process_group(0)
                .spawn()
                .unwrap();
            let tty = unsafe { BorrowedFd::borrow_raw(0) };
            termios::tcsetpgrp(tty, process::Pid::from_raw(reader.id() as i32).unwrap()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !dir.join("foreground").exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            termios::tcsetpgrp(tty, process::getpgrp()).unwrap();
            fs::write(dir.join("reclaimed"), b"read must now return EINTR").unwrap();
            let status = loop {
                if let Some(status) = reader.try_wait().unwrap() {
                    break Some(status);
                }
                if Instant::now() >= deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(1));
            };
            if status.is_none() {
                reader.kill().unwrap();
                reader.wait().unwrap();
            }
            assert!(
                status.is_some_and(|status| status.success()),
                "background read did not return after foreground reclaim: {status:?}"
            );
            return;
        }
        let dir = std::env::temp_dir().join(format!("procward-read-race-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let pty = rustix_openpty::openpty(None, None).unwrap();
        let mut host = child_command()
            .env("PROCWARD_READ_RACE", &dir)
            .stdin(Stdio::from(pty.user))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = host.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            thread::sleep(Duration::from_millis(2));
        };
        if status.is_none() {
            host.kill().unwrap();
            host.wait().unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
        assert!(
            status.is_some_and(|status| status.success()),
            "foreground reclaim read regression: {status:?}"
        );
    }
}
