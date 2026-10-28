// SPDX-License-Identifier: BSD-3-Clause
#![cfg(target_os = "macos")]
use libproc::{bsd_info::BSDInfo, proc_pid::pidinfo};
use rustix::{
    io::Errno,
    process::{self, Pid, Signal},
};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn poll(deadline: Instant, phase: &str, mut predicate: impl FnMut() -> bool) {
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "Darwin observation deadline expired: phase={phase}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn facts(pid: u32) -> io::Result<Option<BSDInfo>> {
    match pidinfo::<BSDInfo>(pid as i32, 0) {
        Ok(info) => Ok(Some(info)),
        Err(error) => {
            let query_errno = errno::errno().0;
            match process::test_kill_process(Pid::from_raw(pid as i32).unwrap()) {
                Err(Errno::SRCH) => Ok(None),
                Ok(()) if query_errno == Errno::SRCH.raw_os_error() => Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "existing process has no libproc identity yet",
                )),
                _ => Err(io::Error::other(error)),
            }
        }
    }
}
fn same(a: &BSDInfo, b: &BSDInfo) -> bool {
    (
        a.pbi_pid,
        a.pbi_start_tvsec,
        a.pbi_start_tvusec,
        a.pbi_uid,
        a.pbi_ruid,
        a.pbi_svuid,
    ) == (
        b.pbi_pid,
        b.pbi_start_tvsec,
        b.pbi_start_tvusec,
        b.pbi_uid,
        b.pbi_ruid,
        b.pbi_svuid,
    )
}
struct Run {
    child: Child,
    dir: PathBuf,
    observed: Vec<BSDInfo>,
    expected_preobservation_cleanup: bool,
}
impl Run {
    fn start(serial: usize) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "procward-darwin-cli-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let fixture = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/fixture");
        let child = Command::new(env!("CARGO_BIN_EXE_procward"))
            .args(["run", "--"])
            .arg(fixture)
            .arg("tree")
            .arg(&dir)
            .stdin(Stdio::piped())
            .stderr(fs::File::create(dir.join("stderr.log")).unwrap())
            .spawn()
            .unwrap();
        let mut run = Self {
            child,
            dir,
            observed: Vec::new(),
            expected_preobservation_cleanup: false,
        };
        for mode in ["tree", "branch", "worker"] {
            let path = run.dir.join(format!("{mode}.pid"));
            poll(
                Instant::now() + Duration::from_secs(20),
                &format!("{mode} PID-file readiness"),
                || {
                    path.try_exists().unwrap()
                        && fs::read_to_string(&path).unwrap().parse::<u32>().is_ok()
                },
            );
            run.observed.push(
                facts(fs::read_to_string(path).unwrap().parse().unwrap())
                    .unwrap()
                    .unwrap(),
            );
        }
        run.observed
            .push(facts(run.observed[0].pbi_ppid).unwrap().unwrap());
        run
    }
    fn finish(&mut self) -> std::process::ExitStatus {
        poll(
            Instant::now() + Duration::from_secs(8),
            "shell-facing owner completion",
            || self.child.try_wait().unwrap().is_some(),
        );
        self.child.wait().unwrap()
    }
    fn diagnostics(&self) {
        eprintln!(
            "Darwin failure facts: dir={:?}; harness PID-file readiness is NOT product inventory retention; stderr={:?}",
            self.dir,
            fs::read_to_string(self.dir.join("stderr.log"))
        );
        for expected in &self.observed {
            let current = facts(expected.pbi_pid).map(|info| {
                info.map(|info| {
                    (
                        info.pbi_pid,
                        info.pbi_ppid,
                        info.pbi_start_tvsec,
                        info.pbi_start_tvusec,
                        info.pbi_status,
                        info.pbi_uid,
                        info.pbi_ruid,
                        info.pbi_svuid,
                    )
                })
            });
            eprintln!(
                "identity expected pid={} ppid={} birth={}.{} state={} uid={}/{}/{}; current={current:?}",
                expected.pbi_pid,
                expected.pbi_ppid,
                expected.pbi_start_tvsec,
                expected.pbi_start_tvusec,
                expected.pbi_status,
                expected.pbi_uid,
                expected.pbi_ruid,
                expected.pbi_svuid
            );
        }
        for mode in ["tree", "branch", "worker"] {
            eprintln!(
                "readiness {mode}.pid={:?}",
                fs::read_to_string(self.dir.join(format!("{mode}.pid")))
            );
        }
    }
    fn gone(&self) -> bool {
        self.observed.iter().all(|info| match facts(info.pbi_pid) {
            Ok(None) => true,
            Ok(Some(now)) => !same(info, &now),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("Darwin completion query failed: {error}"),
        })
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        if thread::panicking() {
            self.diagnostics();
        }
        let mut rescued = false;
        for info in &self.observed {
            let current = match facts(info.pbi_pid) {
                Ok(current) => current,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => panic!("fixture cleanup identity query failed: {error}"),
            };
            if current.is_some_and(|now| same(info, &now)) {
                process::kill_process(Pid::from_raw(info.pbi_pid as i32).unwrap(), Signal::KILL)
                    .unwrap();
                rescued = true;
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        poll(
            Instant::now() + Duration::from_secs(3),
            "harness rescue absence",
            || self.gone(),
        );
        fs::remove_dir_all(&self.dir).unwrap();
        if self.expected_preobservation_cleanup {
            eprintln!(
                "HARNESS CLEANUP: Darwin pre-observation observer-loss window; known fixtures cleaned by test, NOT product cleanup"
            );
        } else if rescued {
            eprintln!("HARNESS RESCUE: known Darwin survivor; NOT product best-effort sweep");
        }
        assert!(
            !rescued || self.expected_preobservation_cleanup || thread::panicking(),
            "observed fixture needed rescue"
        );
    }
}
#[test]
fn darwin_best_effort_tree_owner_loss_and_observer_loss() {
    {
        let mut run = Run::start(0);
        run.child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
        assert_eq!(run.finish().code(), Some(23));
        poll(
            Instant::now() + Duration::from_secs(1),
            "normal-exit retained identity absence",
            || run.gone(),
        );
    }
    {
        let mut run = Run::start(1);
        for info in &run.observed[..3] {
            process::kill_process(Pid::from_raw(info.pbi_pid as i32).unwrap(), Signal::STOP)
                .unwrap();
        }
        let fault = Instant::now();
        run.child.kill().unwrap();
        assert!(!run.finish().success());
        poll(
            fault + Duration::from_secs(8),
            "owner-loss retained identity absence",
            || run.gone(),
        );
    }
    {
        let mut run = Run::start(2);
        let anchor = run.observed.last().unwrap();
        assert!(same(anchor, &facts(anchor.pbi_pid).unwrap().unwrap()));
        let fault = Instant::now();
        process::kill_process(Pid::from_raw(anchor.pbi_pid as i32).unwrap(), Signal::KILL).unwrap();
        assert_eq!(
            run.finish().code(),
            Some(125),
            "observer loss must fail closed"
        );
        assert!(
            fs::read_to_string(run.dir.join("stderr.log"))
                .unwrap()
                .contains("pre-observation descendants may escape")
        );
        run.expected_preobservation_cleanup = true;
        assert!(fault.elapsed() <= Duration::from_secs(8));
    }
}
