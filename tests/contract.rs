// SPDX-License-Identifier: BSD-3-Clause
use std::{
    fs,
    io::{self, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
fn fixture() -> std::path::PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("examples/fixture{}", std::env::consts::EXE_SUFFIX))
}
fn finish(mut child: std::process::Child) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("contract response deadline; HARNESS RESCUE, not product cleanup");
        }
        thread::sleep(Duration::from_millis(2));
    }
    child.wait_with_output().unwrap()
}
#[cfg(unix)]
fn pipe() -> (fs::File, fs::File, &'static str, String) {
    use std::os::fd::AsRawFd;
    let (read, write) = rustix::pipe::pipe().unwrap();
    rustix::io::fcntl_setfd(&write, rustix::io::FdFlags::CLOEXEC).unwrap();
    let raw = read.as_raw_fd().to_string();
    (read.into(), write.into(), "PROCWARD_LIFECYCLE_FD", raw)
}
#[cfg(windows)]
fn pipe() -> (fs::File, fs::File, &'static str, String) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
        Security::SECURITY_ATTRIBUTES,
        System::Pipes::CreatePipe,
    };
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
    assert_ne!(
        unsafe { CreatePipe(&mut read, &mut write, &security, 4096) },
        0
    );
    let read = unsafe { OwnedHandle::from_raw_handle(read) };
    let write = unsafe { OwnedHandle::from_raw_handle(write) };
    assert_ne!(
        unsafe { SetHandleInformation(write.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) },
        0
    );
    let raw = (read.as_raw_handle() as usize).to_string();
    (read.into(), write.into(), "PROCWARD_LIFECYCLE_HANDLE", raw)
}
#[cfg(unix)]
fn identity() -> String {
    "parent".into()
}
#[cfg(windows)]
fn identity() -> String {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut times: [FILETIME; 4] = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut times[0],
                &mut times[1],
                &mut times[2],
                &mut times[3],
            )
        },
        0
    );
    format!(
        "{:016x}",
        (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime)
    )
}

#[test]
fn managed_handoff_is_direct_bounded_one_shot_and_separate_from_native_ownership() {
    let dir = std::env::temp_dir().join(format!("procward-contract-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let output = finish(
        Command::new(env!("CARGO_BIN_EXE_procward"))
            .args(["run", "--"])
            .arg(fixture())
            .arg("managed-probe")
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("ORDINARY-CLEAN"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("MANAGED "));
    assert!(dir.join("accepted").exists());
    fs::remove_file(dir.join("accepted")).unwrap();
    for case in [
        "owner",
        "primary",
        "launch",
        "identity",
        "version",
        "oversized",
        "truncated",
        "EOF",
        "unsupported-env",
    ] {
        let (read, mut write, variable, raw) = pipe();
        let owner = std::process::id() + u32::from(case == "owner");
        let id = "0123456789abcdef0123456789abcdef";
        let child = Command::new(fixture())
            .arg("managed-probe")
            .arg(&dir)
            .env(
                "PROCWARD_PROTOCOL",
                if case == "unsupported-env" { "2" } else { "1" },
            )
            .env("PROCWARD_OWNER_PID", owner.to_string())
            .env("PROCWARD_CONTRACT_ID", id)
            .env(variable, raw)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(read);
        let pid = child.id() + u32::from(case == "primary");
        let mut record = format!(
            "PROCWARD/{} READY {owner} {pid} {} {}\n",
            if case == "version" { 2 } else { 1 },
            if case == "identity" {
                "stale".into()
            } else {
                identity()
            },
            if case == "launch" {
                "ffffffffffffffffffffffffffffffff"
            } else {
                id
            }
        );
        match case {
            "oversized" => record = format!("{}\n", "a".repeat(257)),
            "truncated" => record = "PROCWARD/1 READY".into(),
            "EOF" => record.clear(),
            _ => {}
        }
        if let Err(error) = write.write_all(record.as_bytes()) {
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        }
        drop(write);
        let output = finish(child);
        assert_eq!(output.status.code(), Some(125), "{case}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("REJECTED"),
            "{case}: {output:?}"
        );
        assert!(
            !dir.join("accepted").exists(),
            "{case}: application side effect before validation"
        );
    }
    fs::remove_dir_all(dir).unwrap();
}
