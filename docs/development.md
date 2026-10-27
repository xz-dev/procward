# Developing the Linux walking skeleton

This checkout implements change `extract-procward-core`, tasks 1.1–1.4 only.
It is not a complete v0.1.0 release: detached-tree ownership, independent
owner-death cleanup, graceful TERM/CONT deadlines, interactive job control and
non-Linux backends remain later tasks. Current primary-exit cleanup immediately
KILLs the ordinary process group and reaps until no children remain; a remaining
child at the 7.5-second completion cap is an error, not cleanup success.
TERM/INT/HUP are forwarded during healthy work, but a primary that ignores them
still needs the later bounded shutdown policy.

## Build and checks

Use the host's default stable toolchain and normal Cargo cache:

```sh
cargo check --locked
cargo test
cargo clippy -- -D warnings
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The manifest declares Rust 1.87, satisfying process-wrap 10.0.1's MSRV. Cargo's
resolver selected the checked lockfile for that declaration; all dependency
MSRVs declared in Cargo metadata are at most 1.87 (`cfg_aliases` declares none).
This slice was compiled and tested with host stable Rust 1.99.1. A Rust 1.87
compiler run is not claimed here; add that compiler to ordinary CI in section 2.

## Fixtures and execution context

`tests/fixtures/main.rs` is a fresh, standard-library-only fixture, built as the
Cargo example `fixture` so it is not an installed production binary. Normal
`cargo test` builds it automatically. For focused runs, build it explicitly:

```sh
cargo test --test cli --example fixture healthy_primary_does_not_retain_adopted_zombies
```

The CLI tests assert native argv (including empty/non-UTF-8 values), cwd,
environment, separate stdio streams, exit/signal results, ordinary-tree cleanup,
unrelated-work survival, healthy orphan reaping and rejected startup/exec.
Input and file gates sequence the fixtures. Polls observe state until fixed
deadlines; no elapsed sleep substitutes for readiness. `/proc` absence alone
means gone: stopped processes and zombies still count as present, and query
errors fail.

Tests ran in the task-owned tmux session `pw-apply1`, whose existing tmux server
had PPID 1, outside the installed pi process-owner ancestry. No `wakeup` session
was modified. To reproduce from an agent with a subreaper ancestor, use your own
tmux session and capture its exit status and output:

```sh
tmux new-session -d -s procward-tests \
  'cd /path/to/procward; cargo test > /tmp/procward-tests.log 2>&1; echo $? > /tmp/procward-tests.status'
```

The test process also enables its own subreaper solely for failed-run rescue.
Known gated fixtures get pidfds; rescue signals use those handles, not stale
numeric PIDs. If rescue is needed, output identifies it and the run stays
failed. Nested procward processes remain responsible for normal orphan reaping;
tests require the orphan's live PPID to match procward before releasing it.
Negative controls were run with healthy reaping disabled and with group KILL
replaced by a non-terminating signal. The respective orphan/tree tests failed,
reported rescue honestly, and the test subreaper reaped rescued fixtures. Both
mutations were restored before the final passing suite.

Nothing in this suite launches pi, needs model credentials, or uses pi-env
implementation/test/fixture source. Tests are Linux-only for this slice; their
absence on another platform is not release coverage.
