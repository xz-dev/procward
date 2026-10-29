// SPDX-License-Identifier: BSD-3-Clause
use crate::policy::{Phase, Progress, SignalIdentity};
use libproc::{
    bsd_info::BSDInfo,
    proc_pid::pidinfo,
    processes::{ProcFilter, pids_by_type},
};
use rustix::{
    io::Errno,
    process::{self, Pid},
};
use std::{
    collections::{HashMap, HashSet},
    io,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    pid: u32,
    seconds: u64,
    micros: u64,
    uid: u32,
    real_uid: u32,
    saved_uid: u32,
}
impl Identity {
    fn signal_identity(self) -> SignalIdentity {
        [
            u64::from(self.pid),
            self.seconds,
            self.micros,
            (u64::from(self.uid) << 32) | u64::from(self.real_uid),
            u64::from(self.saved_uid),
        ]
    }
    fn from_info(info: &BSDInfo) -> Self {
        Self {
            pid: info.pbi_pid,
            seconds: info.pbi_start_tvsec,
            micros: info.pbi_start_tvusec,
            uid: info.pbi_uid,
            real_uid: info.pbi_ruid,
            saved_uid: info.pbi_svuid,
        }
    }
}
struct Target {
    identity: Identity,
    term_sent: bool,
    denied: bool,
}
pub struct Inventory(HashMap<Pid, Target>, HashSet<Pid>, Progress);

fn list_children(parent: u32) -> io::Result<Vec<u32>> {
    // libproc 0.14.11 checks errno on a valid empty result without clearing stale errno first.
    errno::set_errno(errno::Errno(0));
    pids_by_type(ProcFilter::ByParentProcess { ppid: parent })
}

// xnu bsd/sys/proc_info_private.h; xnu statically asserts this API struct is 56 bytes.
const PROC_PIDUNIQIDENTIFIERINFO: libc::c_int = 17;
#[repr(C)]
struct UniqueIdentifierInfo {
    uuid: [u8; 16],
    unique_id: u64,
    parent_unique_id: u64,
    id_version: i32,
    orig_ppid_version: i32,
    reserved: [u64; 2],
}
const _: () = assert!(std::mem::size_of::<UniqueIdentifierInfo>() == 56);
// Kernel start microseconds never reach this, so unique-ID and start-time births never compare equal.
const UNIQUE_ID_BIRTH: u64 = u64::MAX;

fn pid_info<T>(pid: Pid, flavor: libc::c_int) -> io::Result<T> {
    let mut info = std::mem::MaybeUninit::<T>::zeroed();
    let size = std::mem::size_of::<T>() as libc::c_int;
    // SAFETY: T is a plain-data proc_info struct; the kernel writes at most `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid.as_raw_nonzero().get(),
            flavor,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    match written {
        written if written == size => Ok(unsafe { info.assume_init() }),
        written if written <= 0 => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other("short proc_pidinfo result")),
    }
}

// xnu bsd/kern/proc_info.c proc_pidinfo_internal(): PROC_PIDTBSDINFO requires the caller's
// effective UID (EPERM otherwise); SHORTBSDINFO and UNIQIDENTIFIERINFO skip that check.
fn other_euid_query(pid: Pid) -> io::Result<(Identity, u32)> {
    let before = pid_info::<UniqueIdentifierInfo>(pid, PROC_PIDUNIQIDENTIFIERINFO)?;
    let info = pid_info::<libc::proc_bsdshortinfo>(pid, libc::PROC_PIDT_SHORTBSDINFO)?;
    let after = pid_info::<UniqueIdentifierInfo>(pid, PROC_PIDUNIQIDENTIFIERINFO)?;
    // The never-reused per-boot unique ID on both sides binds the credentials to one process.
    if before.unique_id != after.unique_id {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("process {pid} changed during its identity query"),
        ));
    }
    if info.pbsi_pid != pid.as_raw_nonzero().get() as u32 {
        return Err(io::Error::other(
            "proc_pidinfo returned a mismatched process PID",
        ));
    }
    Ok((
        Identity {
            pid: info.pbsi_pid,
            seconds: before.unique_id,
            micros: UNIQUE_ID_BIRTH,
            uid: info.pbsi_uid,
            real_uid: info.pbsi_ruid,
            saved_uid: info.pbsi_svuid,
        },
        info.pbsi_ppid,
    ))
}

fn query(pid: Pid) -> io::Result<Option<(Identity, u32)>> {
    let raw = pid.as_raw_nonzero().get();
    let error = match pidinfo::<BSDInfo>(raw, 0) {
        Ok(info) if info.pbi_pid == raw as u32 => {
            return Ok(Some((Identity::from_info(&info), info.pbi_ppid)));
        }
        Ok(_) => {
            return Err(io::Error::other(
                "libproc returned a mismatched process PID",
            ));
        }
        Err(_) => io::Error::from_raw_os_error(errno::errno().0),
    };
    let error = if error.raw_os_error() == Some(Errno::PERM.raw_os_error()) {
        match other_euid_query(pid) {
            Ok(found) => return Ok(Some(found)),
            Err(error) if error.raw_os_error().is_none() => return Err(error),
            Err(error) => error,
        }
    } else {
        error
    };
    match process::test_kill_process(pid) {
        Err(Errno::SRCH) => Ok(None),
        // EPERM proves existence of a process this user may not signal.
        Ok(()) | Err(Errno::PERM) if error.raw_os_error() == Some(Errno::SRCH.raw_os_error()) => {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("process {pid} still exists but has no identity yet"),
            ))
        }
        _ => Err(io::Error::other(format!(
            "macOS identity query for {pid} failed: {error}"
        ))),
    }
}

fn current(pid: Pid) -> io::Result<Option<Identity>> {
    Ok(query(pid)?.map(|(identity, _)| identity))
}

fn signal_if_current(
    expected: Identity,
    current: io::Result<Option<Identity>>,
    send: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    if current? != Some(expected) {
        return Ok(false);
    }
    send()?;
    Ok(true)
}

impl Inventory {
    pub fn new() -> Self {
        Self(HashMap::new(), HashSet::new(), Progress::default())
    }
    pub fn progress(&mut self) -> &mut Progress {
        &mut self.2
    }
    pub fn refresh(&mut self) -> io::Result<()> {
        let root = process::getpid();
        // Unknown observations remain completion blockers, never signalling authority.
        let mut absent = Vec::new();
        for pid in &self.1 {
            match query(*pid) {
                Ok(None) => absent.push(*pid),
                Ok(Some(_)) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        for pid in absent {
            self.1.remove(&pid);
        }
        let mut pending: Vec<_> = self.0.keys().copied().chain([root]).collect();
        let mut visited = HashSet::new();
        while let Some(parent) = pending.pop() {
            if !visited.insert(parent) {
                continue;
            }
            let before = match current(parent) {
                Ok(Some(identity)) => identity,
                Ok(None) => {
                    self.0.remove(&parent);
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            if parent != root
                && self
                    .0
                    .get(&parent)
                    .is_none_or(|target| target.identity != before)
            {
                self.0.remove(&parent);
                continue;
            }
            let mut observed = Vec::new();
            for raw in list_children(before.pid)? {
                let pid = Pid::from_raw(raw as i32)
                    .ok_or_else(|| io::Error::other("invalid libproc child PID"))?;
                match query(pid) {
                    Ok(Some((identity, ppid))) if ppid == before.pid => {
                        observed.push((pid, identity))
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        self.1.insert(pid);
                    }
                    Err(error) => return Err(error),
                }
            }
            let after = match current(parent) {
                Ok(identity) => identity,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            if after != Some(before) {
                continue;
            }
            for (pid, identity) in observed {
                self.1.remove(&pid);
                let entry = self.0.entry(pid).or_insert(Target {
                    identity,
                    term_sent: false,
                    denied: false,
                });
                if entry.identity != identity {
                    *entry = Target {
                        identity,
                        term_sent: false,
                        denied: false,
                    };
                }
                pending.push(pid);
            }
        }
        Ok(())
    }
    pub fn advance(&mut self, phase: Phase, protected: Option<Pid>) -> io::Result<()> {
        let mut gone = Vec::new();
        for (pid, target) in &mut self.0 {
            if Some(*pid) == protected {
                continue;
            }
            let now = match current(*pid) {
                Ok(identity) => identity,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            if now != Some(target.identity) {
                gone.push(*pid);
                continue;
            }
            for signal in self.2.signals(
                phase,
                target.identity.signal_identity(),
                &mut target.term_sent,
            )? {
                // macOS has an unavoidable final identity-check/use race, not kernel containment.
                let now = match current(*pid) {
                    Ok(identity) => identity,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error),
                };
                match signal_if_current(target.identity, Ok(now), || {
                    process::kill_process(*pid, *signal).map_err(io::Error::from)
                }) {
                    Ok(true) => {}
                    Ok(false) => {
                        gone.push(*pid);
                        break;
                    }
                    Err(error) if error.raw_os_error() == Some(Errno::SRCH.raw_os_error()) => {
                        gone.push(*pid);
                        break;
                    }
                    // Real/saved UIDs no longer match ours (e.g. after sudo): not signalable, still owned.
                    Err(error) if error.raw_os_error() == Some(Errno::PERM.raw_os_error()) => {
                        if !std::mem::replace(&mut target.denied, true) {
                            crate::diagnostic(format!(
                                "macOS denied signals to privileged descendant {pid}; it remains a known survivor"
                            ));
                        }
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        for pid in gone {
            self.0.remove(&pid);
        }
        Ok(())
    }
    pub fn empty_except(&self, pid: Pid) -> io::Result<bool> {
        Ok(self.1.is_empty() && self.0.keys().all(|target| *target == pid))
    }

    pub fn freeze(&mut self, paused: bool) -> io::Result<()> {
        let signal = if paused {
            process::Signal::STOP
        } else {
            process::Signal::CONT
        };
        self.refresh()?;
        for (pid, target) in &self.0 {
            let now = match current(*pid) {
                Ok(identity) => identity,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            signal_if_current(target.identity, Ok(now), || {
                match process::kill_process(*pid, signal) {
                    // Job control cannot stop a descendant the OS forbids this user to signal.
                    Ok(()) | Err(Errno::SRCH) | Err(Errno::PERM) => Ok(()),
                    Err(error) => Err(error.into()),
                }
            })?;
        }
        Ok(())
    }

    pub fn forget(&mut self, pid: Pid) {
        self.0.remove(&pid);
        self.1.remove(&pid);
    }
    #[cfg(test)]
    pub(crate) fn knows(&self, pid: Pid) -> bool {
        self.0.contains_key(&pid)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty() && self.1.is_empty()
    }
}

pub fn children() -> io::Result<Vec<Pid>> {
    list_children(std::process::id())?
        .into_iter()
        .map(|pid| {
            Pid::from_raw(pid as i32)
                .ok_or_else(|| io::Error::other("invalid libproc direct child"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    fn poll(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }
    struct Fixture {
        child: Child,
        dir: PathBuf,
        worker: Option<Identity>,
        expected_escape: bool,
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("procward-darwin-{}-{mode}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let fixture = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("examples/fixture");
            let child = Command::new(fixture)
                .arg(mode)
                .arg(&dir)
                .stdin(Stdio::null())
                .spawn()
                .unwrap();
            Self {
                child,
                dir,
                worker: None,
                expected_escape: false,
            }
        }
        fn remember(&mut self, name: &str) -> Pid {
            let path = self.dir.join(format!("{name}.pid"));
            poll(|| {
                path.try_exists().unwrap()
                    && fs::read_to_string(&path).unwrap().parse::<i32>().is_ok()
            });
            let pid = Pid::from_raw(fs::read_to_string(path).unwrap().parse().unwrap()).unwrap();
            self.worker = Some(current(pid).unwrap().unwrap());
            pid
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(identity) = self.worker {
                let pid = Pid::from_raw(identity.pid as i32).unwrap();
                if current(pid).unwrap() == Some(identity) {
                    eprintln!(
                        "HARNESS RESCUE: Darwin known survivor {}, expected unseen-escape demonstration: {}; NOT product cleanup",
                        identity.pid, self.expected_escape
                    );
                    process::kill_process(pid, process::Signal::KILL).unwrap();
                    poll(|| match query(pid) {
                        Ok(None) => true,
                        Ok(Some(_)) => false,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => false,
                        Err(error) => panic!("fixture reaping query failed: {error}"),
                    });
                    assert!(
                        self.expected_escape || thread::panicking(),
                        "observed fixture needed rescue"
                    );
                }
            }
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    fn effective_uid(pid: Pid) -> Option<u32> {
        pid_info::<libc::proc_bsdshortinfo>(pid, libc::PROC_PIDT_SHORTBSDINFO)
            .ok()
            .map(|info| info.pbsi_uid)
    }
    #[test]
    fn setuid_root_descendant_is_observed_without_failing_refresh() {
        let _guard = crate::owner::tests::PROCESS_LOCK.lock().unwrap();
        // ps prints this argv, larger than any pipe buffer, so setuid-root ps stays blocked and observable.
        let mut holder = Command::new("/bin/sh")
            .args(["-c", "read line", "sh", &"x".repeat(256 * 1024)])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ps = Command::new("/bin/ps")
            .args(["-ww", "-o", "command=", "-p", &holder.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = Pid::from_raw(ps.id() as i32).unwrap();
        poll(|| effective_uid(pid) == Some(0));
        let mut inventory = Inventory::new();
        let refreshed = inventory.refresh();
        let observed = inventory.0.get(&pid).map(|target| target.identity);
        let still_blocked = ps.try_wait().unwrap().is_none();
        ps.kill().unwrap();
        ps.wait().unwrap();
        holder.kill().unwrap();
        holder.wait().unwrap();
        refreshed.expect("setuid-root descendant made inventory refresh fail");
        assert!(still_blocked, "setuid-root ps finished before observation");
        let observed = observed.expect("setuid-root descendant missing from inventory");
        assert_eq!(
            (observed.pid, observed.uid, observed.real_uid),
            (
                pid.as_raw_nonzero().get() as u32,
                0,
                process::getuid().as_raw()
            )
        );
        assert_eq!(
            observed.micros, UNIQUE_ID_BIRTH,
            "birth must be the unique ID"
        );
    }
    #[test]
    fn denied_signal_keeps_privileged_target_as_known_survivor() {
        // xnu bsd/kern/kern_sig.c cansignal_nomac(): nobody, not even root, may signal initproc.
        let launchd = Pid::from_raw(1).unwrap();
        let identity = current(launchd).unwrap().unwrap();
        let mut inventory = Inventory::new();
        inventory.0.insert(
            launchd,
            Target {
                identity,
                term_sent: true,
                denied: false,
            },
        );
        inventory.advance(Phase::Kill, None).unwrap();
        assert!(inventory.0[&launchd].denied);
        assert!(!inventory.is_empty(), "unsignalable survivor was dropped");
    }
    #[test]
    fn unknown_observation_blocks_completion_without_signal_authority() {
        let mut inventory = Inventory::new();
        inventory.1.insert(process::getpid());
        inventory.advance(Phase::Grace, None).unwrap();
        assert!(!inventory.is_empty());
        assert!(query(process::getpid()).unwrap().is_some());
    }
    #[test]
    fn changed_birth_pid_or_credentials_and_unknown_queries_never_authorize_signal() {
        let saved = Identity {
            pid: 42,
            seconds: 100,
            micros: 10,
            uid: 501,
            real_uid: 501,
            saved_uid: 501,
        };
        for current in [
            Identity { pid: 43, ..saved },
            Identity {
                micros: 11,
                ..saved
            },
            Identity {
                seconds: 101,
                ..saved
            },
            Identity { uid: 0, ..saved },
            Identity {
                real_uid: 0,
                ..saved
            },
            Identity {
                saved_uid: 0,
                ..saved
            },
        ] {
            assert!(
                !signal_if_current(saved, Ok(Some(current)), || panic!(
                    "stale observation signalled unrelated work"
                ))
                .unwrap()
            );
        }
        assert!(
            signal_if_current(
                saved,
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
                || panic!("unknown query authorized a signal")
            )
            .is_err()
        );
        let mut sent = false;
        assert!(
            signal_if_current(saved, Ok(Some(saved)), || {
                sent = true;
                Ok(())
            })
            .unwrap()
        );
        assert!(sent);
    }
    #[test]
    fn retained_observation_survives_setsid_and_reparenting_but_unseen_escape_is_explicit() {
        let _guard = crate::owner::tests::PROCESS_LOCK.lock().unwrap();
        {
            let mut fixture = Fixture::new("observed-tree");
            let worker = fixture.remember("observed-worker");
            let mut inventory = Inventory::new();
            poll(|| {
                inventory.refresh().unwrap();
                inventory.0.contains_key(&worker)
            });
            fs::write(fixture.dir.join("detach"), b"go").unwrap();
            poll(|| fixture.child.try_wait().unwrap().is_some());
            assert_eq!(fixture.child.wait().unwrap().code(), Some(23));
            poll(|| fs::metadata(fixture.dir.join("observed-detached")).is_ok());
            inventory.refresh().unwrap();
            assert!(
                inventory.0.contains_key(&worker),
                "reparenting lost observed identity"
            );
            inventory.advance(Phase::Grace, None).unwrap();
            poll(|| {
                inventory.refresh().unwrap();
                inventory.is_empty()
            });
        }
        {
            let mut fixture = Fixture::new("fast-escape");
            let worker = fixture.remember("escape-worker");
            poll(|| fixture.dir.join("escaped").try_exists().unwrap());
            fs::write(fixture.dir.join("exit-escape"), b"go").unwrap();
            poll(|| fixture.child.try_wait().unwrap().is_some());
            fixture.child.wait().unwrap();
            let mut inventory = Inventory::new();
            inventory.refresh().unwrap();
            assert!(!inventory.0.contains_key(&worker));
            assert!(
                query(worker).unwrap().is_some(),
                "escape fixture was not alive"
            );
            fixture.expected_escape = true;
            eprintln!(
                "EXPECTED MACOS LIMITATION: setsid/reparent before first observation escapes best-effort inventory; rescue follows separately"
            );
        }
        // Keep real child-owning cases sequential: concurrent inventories would own each other's fixtures.
        crate::owner::tests::retained_darwin_inventory_cleans_after_observer_loss();
    }
}
