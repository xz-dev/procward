// SPDX-License-Identifier: BSD-3-Clause
use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
use std::{
    ffi::{OsStr, OsString},
    io,
    time::Duration,
};

pub fn run(command: &OsStr, args: &[OsString]) -> io::Result<u32> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let mut command = tokio::process::Command::new(command);
        command.args(args);
        let mut child = CommandWrap::from(command)
            .wrap(KillOnDrop)
            .wrap(JobObject)
            .spawn()?;
        // JobObject::wait waits for the entire Job; observe only its immediate inner primary first.
        let status = loop {
            if let Some(status) = child.inner_mut().try_wait()? {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        child.start_kill()?;
        tokio::time::timeout(Duration::from_millis(7500), child.wait())
            .await
            .map_err(|_| io::Error::other("Job completion deadline expired"))??;
        Ok(status
            .code()
            .expect("Windows process has a native exit code") as u32)
    });
    // A timed-out completion-port blocking task must not make runtime destruction unbounded.
    runtime.shutdown_timeout(Duration::from_millis(50));
    result
}
