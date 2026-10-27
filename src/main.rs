// SPDX-License-Identifier: BSD-3-Clause
use std::{env, ffi::OsString, process::ExitCode};

#[cfg(target_os = "linux")]
#[path = "linux_inventory.rs"]
mod inventory;
#[cfg(target_os = "freebsd")]
#[path = "freebsd_inventory.rs"]
mod inventory;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod owner;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod policy;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod reaper;

const USAGE: &str = "procward 0.1.0\n\nUsage: procward run -- <command> [args...]\n       procward --help\n       procward --version\n\nNoninteractive development build; interactive job control is not yet supported.";

fn main() -> ExitCode {
    match cli(env::args_os().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("procward: {message}");
            ExitCode::from(125)
        }
    }
}

fn cli(args: Vec<OsString>) -> Result<u8, String> {
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            Ok(0)
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("procward {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        [role, rest @ ..] if role == "__anchor" => {
            owner::anchor(rest).map_err(|error| error.to_string())
        }
        [run, separator, command, rest @ ..]
            if run == "run" && separator == "--" && !command.is_empty() =>
        {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            return owner::run(command, rest).map_err(|error| error.to_string());
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            {
                let _ = rest;
                Err("this development slice supports Linux and FreeBSD only".into())
            }
        }
        _ => Err("expected run -- <command> [args...]; see --help".into()),
    }
}
