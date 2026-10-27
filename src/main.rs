// SPDX-License-Identifier: BSD-3-Clause
use std::{env, ffi::OsString, process::ExitCode};

#[cfg(target_os = "linux")]
#[path = "linux_inventory.rs"]
mod inventory;
#[cfg(target_os = "freebsd")]
#[path = "freebsd_inventory.rs"]
mod inventory;
#[cfg(target_os = "macos")]
#[path = "darwin.rs"]
mod inventory;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod owner;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod policy;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod reaper;
#[cfg(windows)]
mod windows;

const USAGE: &str = "procward 0.1.0\n\nUsage: procward run -- <command> [args...]\n       procward --help\n       procward --version\n\nNoninteractive development build; interactive job control is not yet supported.";

fn main() -> ExitCode {
    match cli(env::args_os().skip(1).collect()) {
        Ok(code) => {
            #[cfg(windows)]
            std::process::exit(code as i32);
            #[cfg(not(windows))]
            ExitCode::from(code as u8)
        }
        Err(message) => {
            eprintln!("procward: {message}");
            ExitCode::from(125)
        }
    }
}

fn cli(args: Vec<OsString>) -> Result<u32, String> {
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            Ok(0)
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("procward {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
        [role, rest @ ..] if role == "__anchor" => owner::anchor(rest)
            .map(u32::from)
            .map_err(|error| error.to_string()),
        [run, separator, command, rest @ ..]
            if run == "run" && separator == "--" && !command.is_empty() =>
        {
            #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
            return owner::run(command, rest)
                .map(u32::from)
                .map_err(|error| error.to_string());
            #[cfg(windows)]
            return windows::run(command, rest).map_err(|error| error.to_string());
            #[cfg(not(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "macos",
                windows
            )))]
            {
                let _ = rest;
                Err("this development slice supports Linux and FreeBSD only".into())
            }
        }
        _ => Err("expected run -- <command> [args...]; see --help".into()),
    }
}
