# Development

This checkout implements the noninteractive lifecycle/platform slice of change
`extract-procward-core`. It is not a complete v0.1.0 release: Unix terminal job
control, application handoff/task control, final documentation and release setup
remain later tasks. No tests launch pi or use pi-env source/fixtures.

## Build and checks

```sh
cargo build --locked --bins --examples
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Build binaries and examples explicitly before focused or `--all-targets` tests:
the example's test harness is not its executable image, and the native Darwin
observer test launches the real procward binary. The example `fixture` is
test-only, not an installed binary. Cargo declares Rust 1.87 (process-wrap 10's MSRV); GNU x64
CI installs that compiler and checks all Linux targets alongside stable runtime
tests. The lockfile includes target-specific dependency resolution.

## Runtime fixtures

Linux fixtures cover native argv/cwd/environment/stdio/status, rejected startup,
exec failures without shell fallback, healthy adopted-orphan reaping, unrelated
work, detached/stopped descendants, stale pidfds, launch-gate EOF/stale parents,
owner/anchor loss and targeted wake. Stable handles and observable file/input
responses sequence actions; polling has fixed deadlines, not readiness sleeps.
Tests distinguish stopped, zombie, gone and query failure. The stale-pidfd test
checks stable-handle safety; it does not force numeric PID reuse in the kernel.

FreeBSD guest fixtures exercise nested reapers, stopped adoption, healthy orphan
reaping, targeted parent-death SIGCONT and kernel query errors. Windows fixtures
check kill-on-close Job membership at payload startup, launch restrictions,
owner termination without destructors, primary versus Job completion, native
32-bit results, console Ctrl-C/Break and real ConPTY readiness/quit exchanges.
Darwin tests cover retained identity after setsid/reparenting, denied identity
queries, birth/UID/PID mismatches, owner loss, a native explicitly inventoried
observer-loss fallback and a separately rescued pre-observation escape
demonstration. The CLI early-observer-loss case asserts failure 125 and the
published limitation; any survivors are labelled test harness cleanup, not
product cleanup. PID-file readiness does not establish observation. Darwin
completion is best-effort.

With defaults, Linux/FreeBSD use one immutable cleanup policy: TERM then one CONT
per newly adopted child during grace, KILL afterwards, no late rescue CONT and a
7.5-second cap.
EOF-first selects 6 seconds. Only already-saved fatal helper status selects 3.
Tiny deterministic policy tests check exact boundaries; runtime ignoring-TERM
and stopped-adoption fixtures preserve the default fault-relative 8-second assertion.
Cleanup is generic/configurable: `--grace 6s --fatal-grace 3s
--cleanup-deadline 7500ms` are defaults, with validation described in
[platforms.md](platforms.md). A one-shot private CLOEXEC record publishes the
system-wide monotonic start/selection before cleanup actions so outer fallback
cannot restart either configured grace or deadline. The non-default late-loss
fixture uses 2s/1s/2500ms; discarding the record made its negative control fail at
2.821 seconds. Loss before publication is a separate checked case.

Linux runs from this agent use task-owned `pw-*` tmux sessions outside the
installed pi subreaper. The existing server has PPID 1; no `wakeup` session was
modified. Example:

```sh
tmux new-session -d -s pw-tests \
  'cd /path/to/procward; cargo build --locked --bins --examples && cargo test --locked --all-targets > /tmp/pw-tests.log 2>&1; echo $? > /tmp/pw-tests.status'
```

Test-only rescue is explicit, identity-safe and leaves failed runs failed.
Pipe drain and child exit are observed separately with deadlines, so a dead
shell-facing owner cannot hide a stopped helper holding output open. Negative
controls disabled detached cleanup, replaced the parent-death CONT wake, and
omitted cleanup CONT; corresponding fresh fixtures failed. Restored code passed.
An early negative-control attempt exposed an unbounded test pipe drain and
required separately logged manual rescue; that harness was fixed, then its
negative control failed cleanly on deadline and reported its own rescue.
A child-only Linux seccomp filter also denies PR_SET_PDEATHSIG to verify required
wake setup refuses payload execution. Omitting arming made that new negative
control fail with payload status 0 instead of 125; the syscall was restored.

## Every-event CI and normal reruns

Repository metadata was verified with
`gh repo view --json url,description,repositoryTopics`: canonical URL
`https://github.com/xz-dev/procward`, exact design description and all 13 topics.
No settings, tags or releases were changed during backend implementation.

All pushes and pull requests now require twelve rows: GNU/musl Linux
x64/arm64/riscv64, FreeBSD x64/arm64 guests, macOS x64/arm64 and Windows x64/arm64.
The riscv64 rows use pinned qemu-user/binfmt and digest-pinned target containers;
see [platforms.md](platforms.md) for the host-kernel/timing limitation. No
path/branch filters,
fail-fast or newer-event cancellation. Actual source SHA, executor, compiler host
and fixture results appear in ordinary logs. Alpine's `*-alpine-linux-musl`
vendor alias maps to its native `*-unknown-linux-musl` row label.

Early unimplemented backends ran the fixture assertion and reported
`BACKEND INCOMPLETE (expected-unsupported during early development, NOT release coverage)`
as red, never a fake release pass. Initial full matrix: `bec61c5`, run
`40523401056`; docs-only event: `db91523`, run `40523519264`. Ordinary failed-job
rerun of the first run retained attempt 1 logs. A genuine fixture-publication
race occurred in GNU x64 at `b6fde08`, run `40528359033`: forced KILL could catch
`fs::write` after truncate. Its ordinary job rerun kept the old source/assertions;
fix-forward `c98a0c4` publishes the observation with atomic rename instead.
GNU x64 attempt 2 passed with old source/assertions, confirming normal flaky-job
reruns work without deleting attempt 1. One historical macOS arm64 row was not
acquired by a hosted runner (GitHub annotations cite capacity constraints), not
cancelled by matrix/new-event policy; current required rows are watched/rerun
normally until complete.

## Narrow dependency fixes

The real fixtures found two upstream bugs, handled through separately committed
pristine vendors and minimal documented patches, not new OS implementations:

- process-wrap 10.0.1 returned Job completion after any completion packet.
  Pristine deterministic regression: run `40532870970`, both Windows arches red;
  fix `44ed740` waits for this Job's ACTIVE_PROCESS_ZERO and propagates errors.
- rustix 1.1.6 converted FreeBSD's no-failed-PID sentinel -1 as a positive Pid.
  Pristine corrected regression: run `40536071916`; fix `ebdb103` maps the
  sentinel to None. No procctl shim or disabled assertion.

See `third_party/*/PROCWARD-PATCH.md` and their unfiled `UPSTREAM.md` reports/diffs.
Remote regression branches were deleted after recording red evidence; source
commits and ordinary Actions logs remain attributable. No upstream issue/PR was
filed. Parent/user approval is needed before filing, and vendors should be
removed once upstream ships fixes. Original licences and modification notices
are preserved; [licensing.md](licensing.md) lists all 64 locked crate/versions.

## Original ten-row section 2–3 checkpoint

Code commit `eb8ca7d`: [run 40545740279](https://github.com/xz-dev/procward/actions/runs/40545740279)
completed **success on all ten rows**, with real fixture execution:

| Runtime target | Result |
| --- | --- |
| x86_64-unknown-linux-gnu | success, including Rust 1.87 check |
| aarch64-unknown-linux-gnu | success |
| x86_64-unknown-linux-musl | success |
| aarch64-unknown-linux-musl | success |
| x86_64-unknown-freebsd | success, FreeBSD 14.3 guest |
| aarch64-unknown-freebsd | success, FreeBSD 14.3 guest |
| x86_64-apple-darwin | success |
| aarch64-apple-darwin | success |
| x86_64-pc-windows-msvc | success, console and ConPTY |
| aarch64-pc-windows-msvc | success, console and ConPTY |

This is an ordinary development checkpoint, not release acceptance or a sealed
artifact packet. The later binding riscv64 addition expands the requirement to
12 rows and reopens task 2.2 until both emulated GNU/musl fixture rows pass.
Subsequent documentation-only pushes still run the full matrix.
At that ten-row checkpoint, tasks 1–3 were complete (14/34); sections 4–7 remained, including interactive Unix
job control, application/task contracts, release setup/review, owner-controlled
release prerequisites and later pi integration. No tag, release, repository
setting or pi/pi-env code was changed in this slice.

## Expanded riscv64 checkpoint

The binding riscv64 addition is implemented at `41210c0` in
[run 40557371203](https://github.com/xz-dev/procward/actions/runs/40557371203).
All twelve workflow rows concluded successfully after the ordinary failed-row
rerun (attempt 2). This closes the reopened task 2.2; tasks 1–3 are again complete
(14/34), with sections 4–7 still pending. This is not release acceptance.

| Target | Result in 40557371203 |
| --- | --- |
| x86_64-unknown-linux-gnu | Passed, including Rust 1.87 check |
| aarch64-unknown-linux-gnu | Passed |
| x86_64-unknown-linux-musl | Passed |
| aarch64-unknown-linux-musl | Passed |
| riscv64gc-unknown-linux-gnu | Applicable assertions passed; **2 EMULATOR-UNSUPPORTED** cases |
| riscv64gc-unknown-linux-musl | Applicable assertions passed; **1 EMULATOR-UNSUPPORTED** case |
| x86_64-unknown-freebsd | Passed |
| aarch64-unknown-freebsd | Passed |
| x86_64-apple-darwin | Passed |
| aarch64-apple-darwin | Passed on unchanged-source ordinary rerun |
| x86_64-pc-windows-msvc | Passed, including console/ConPTY |
| aarch64-pc-windows-msvc | Passed, including console/ConPTY |

Both riscv64 jobs use the official rustup target, cross-build release-profile
binaries/tests and execute the full invoked suite in pinned target containers.
GNU uses the Ubuntu cross GCC/sysroot; musl uses shipped rust-lld plus
`-C target-feature=+crt-static -C link-self-contained=yes`. ELF machine checks
verify RISC-V outputs. Each exercised 7 unit, 19 CLI and 3 portable cases, but the
Rust harness's successful return does **not** make the explicitly unsupported
cases product passes. Job summaries count/list them: seccomp installation
`ENOSYS` in both rows, plus known-missing-path std spawn returning a child exiting
127 through qemu's broken vfork/posix_spawn error path in GNU. Musl's exec-error
assertions ran normally. Genuine primary status 127 remains strict in both.
See [platforms.md](platforms.md) for exact limits and full-system VM follow-up.
Timing scale stays **1×**, with no wider budgets or weakened native assertions.
No product code was changed for emulation.

Initial expansion commit `e42091c`,
[run 40552699322](https://github.com/xz-dev/procward/actions/runs/40552699322),
kept both red riscv64 rows visible. GNU exposed the two emulator limitations;
Alpine's distro Rust reported `riscv64-alpine-linux-musl`, not the required exact
triple. Rather than alias that compiler or work around emulation in product code,
`41210c0` switched both variants to official exact-target release builds and
explicit runtime-probed unsupported results.

Run 40557371203 attempt 1 also failed Darwin arm64's one-second post-normal-exit
absence check and labelled its known-survivor harness rescue. The unchanged-source
rerun passed; no assertion or timing was changed. Root cause is not established
by those logs, so the nondeterministic observation/completion failure remains a
risk, not a fixed product defect. Both attempts' logs remain available.

Local follow-up gates passed outside the installed pi subreaper in task-owned
`pw-riscv-*` tmux sessions: full Linux tests, clippy `-D warnings`, formatting,
plus an official self-contained riscv64 musl release cross-build. Setting the
emulator flag alone on native Linux still ran both original capability assertions
and emitted zero unsupported markers. Strict OpenSpec validation passed.
