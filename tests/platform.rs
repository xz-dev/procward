// SPDX-License-Identifier: BSD-3-Clause
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[test]
fn backend_runs_real_fixture_and_preserves_primary_status() {
    let fixture = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("examples/fixture{}", std::env::consts::EXE_SUFFIX));
    let mut child = Command::new(env!("CARGO_BIN_EXE_procward"))
        .args(["run", "--"])
        .arg(fixture)
        .args(["exit", "23"])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(
                status.code(),
                Some(23),
                "BACKEND INCOMPLETE (expected-unsupported during early development, NOT release coverage): {} / {}: {status}",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture exceeded deadline; HARNESS RESCUE, not product cleanup");
        }
        thread::sleep(Duration::from_millis(5));
    }
}
