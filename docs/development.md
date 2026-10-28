# Development

This checkout implements standalone lifecycle/platform ownership, Unix terminal
job control and generic application/task contracts for change
`extract-procward-core`. Release setup is described in [ci-release.md](ci-release.md);
the independent pre-tag review, owner-controlled immutable-release prerequisite,
first release and later pi integration remain separate tasks. No tests launch pi
or use pi-env source/fixtures.

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

## Dependency updates

Dependabot checks Cargo, GitHub Actions and Docker image pins daily. Cargo and
Actions allow up to 20 open version-update PRs each; Docker allows 5 per image
directory, 20 across the four images. Its PRs run the normal full `pull_request`
CI; no auto-merge is configured. SHA-pinned `uses:` references keep their version comments (`# v4`,
`# v7.0.2`, etc.) so Dependabot can update the SHA and version comment together.

`process-wrap` and `rustix` are explicitly excluded from automatic version updates
because `[patch.crates-io]` selects their patched vendors. Check their upstream
releases manually; review `third_party/*/PROCWARD-PATCH.md` and remove/update each
vendor only after its required fixes are available and the full matrix passes.
Refresh Cargo.lock and the dependency licence inventory/bundle with dependency
changes. An ignored vendor is not automatically monitored by this configuration.

### Automated container-image updates

Each `ci/images/*/Dockerfile` contains one `FROM tag@sha256:…` reference.
Dependabot's Docker ecosystem scans all four directories and updates tags/digests,
including new digests when moving tags such as Debian `unstable-slim` are rebuilt.
These Dockerfiles are reference manifests only: CI and release workflows read
`FROM` with `sed -n 's/^FROM //p'`, expose step outputs and pass the exact pinned
reference to Docker runners or the setup-qemu action. Nothing is built from them.

| Dockerfile directory | Image purpose |
| --- | --- |
| `ci/images/alpine-native` | Alpine, native x64/arm64 musl fixtures |
| `ci/images/alpine-riscv64` | Alpine, riscv64 musl fixtures |
| `ci/images/debian-riscv64` | Debian unstable-slim, riscv64 GNU fixtures |
| `ci/images/binfmt` | tonistiigi/binfmt, qemu-user registration |

Review image-update PRs with the same architecture checks and full fixture CI as
other changes; do not replace runtime execution with a build-only check.

Image contents stayed unchanged during this move. Initial `.github/ci-images`
checks discovered the manifests but GitHub rejected PR creation with
`dependency_file_not_supported` / `invalid or unauthorized changes` (glob run
`40656438537`, explicit-directories run `40657102372`). The bounded fallback moved
manifests to `ci/images` and used four singular-directory entries. All four real
checks then succeeded: Alpine native `40658230840`, Alpine riscv64 `40658231177`,
binfmt `40658231943`, Debian riscv64 `40658233432`. Dependabot opened Alpine PRs
#3 and #4 with no auto-merge request. No manual image-refresh process is required.

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
cannot restart either configured grace or deadline. The same pipe now retains
per-identity TERM/CONT reservations across takeover, preventing replay for an
already-treated identity while allowing first signals for new adopted work.
Reservations precede syscalls: a crash in between may omit that grace attempt,
never replay it; original KILL/deadline policy is unchanged. Backpressure defers
grace signals without blocking KILL. The non-default late-loss
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

## Pre-tag review fix-forward

The review at `200cc58` blocked tagging. Three independent fixes have local
red→green evidence under `/var/tmp/pwf-fixes/` (task-owned `pwf-*` tmux sessions,
server PPID 1, no installed pi subreaper ancestor):

| Concern | Regression / evidence | Result |
| --- | --- | --- |
| SIGTTIN restart loop (`c7e3e68`) | `cargo test --locked --bin procward background_read_after_foreground_reclaim_returns_to_cleanup -- --nocapture`; `ttin-red.log` / `ttin-green.log` | Before fix: denied background read never returned after shell reclaim, test failed at its unchanged 2s reader deadline. After fix: direct read returns to cleanup. |
| Repeated unchanged blocked terminal fixture (`c7e3e68`) | `PROCWARD_TERMINAL_STRESS=200 cargo test --locked --test terminal repeated_blocked_terminal_cleanup_survives_foreground_reclaim -- --nocapture`; `ttin-green.log` | All 200 passed; ordinary full-suite CI repeats eight times by default, retaining each original assertion/deadline. |
| Raw terminal on anchor loss (`d07ec8b`) | `cargo test --locked --test terminal anchor_loss_restores_raw_terminal_only_with_foreground_authority -- --nocapture`; `restore-red.log` / `restore-green.log` | Before fix: status 125 but `restored=false`, cooked flags lost. After fix: `restored=true`, status 125, all owned identities gone; unrelated-foreground variant preserves its settings. |
| Repeated teardown CONT (`11cab0a`) | `cargo test --locked --test cli early_anchor_loss_does_not_repeat_teardown_cont_for_adopted_identity -- --nocapture`; `cont-red.log` / `cont-green.log` | Before fix: `true 2`, expected `true 1`. After fix: count one, status 125, no survivors within original 2500ms deadline. FreeBSD also exercises early loss with its existing order fixture. |

After these fixes, local `cargo test --locked --all-targets -- --nocapture`
passed 55 applicable Linux tests (zero failures/ignored). Full build, clippy
`-D warnings`, formatting, strict OpenSpec validation and three release-safety
regressions passed. This local result does not replace the required twelve-row
ordinary CI or independent review approval. No tags, releases, repository settings
or pi code changed. Local commits used command-scoped signing disable after the
configured GPG pinentry timed out; no persistent Git setting changed.

Follow-up: the Darwin normal-exit absence flake remains unclassified. Tests now
label each poll phase and print retained/current PID, parent, birth, state and
UIDs, captured stderr and fixture-readiness facts before failed fixture cleanup.
No assertion/deadline changed. Harness readiness alone is not proof that product
inventory retained the identity; classify the next failure rather than treating
an unchanged-source rerun as a fix.

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

## Default nesting checkpoint (addendum-15)

The pi contract now requires a new pi-added layer under pi or any external
procward. Only the launcher's ownership flag/environment opt-out disables that
layer; only pi's private one-shot bootstrap marker plus valid READY suppresses
that bootstrap's second wrapping. Pi implementation remains section 7 work, not
code changed here. Procward itself always supports nested `run` scopes.

The new `tests/nesting.rs` executes the real
`procward run -- procward run -- <fixture>` boundary. It checks payload result
23 through both layers, outer-owner loss, inner-owner loss (Unix 137 / explicit
Windows native 71), and exclusive inner contract metadata/transport. Unix's
worker ignores TERM; Linux/FreeBSD also detach the branch and stop the worker,
and FreeBSD gives that branch a subordinate reaper. The outer policy is
2s / 1s / 2500ms against the inner defaults 6s / 3s / 7500ms. Fault timing starts
immediately before the actual kill, after identity preflight; every fault still
must complete inside the unchanged outer 2500ms deadline. Normal exit allows
the inner ordinary grace to finish before the outer primary exits. macOS uses
an attached tree and retains the documented best-effort observation boundary.

The fixtures found one product defect: an incoming lifecycle endpoint survived
into the nested payload even though its environment had been replaced.
`c207383` discards that endpoint before native roles/new handoff setup, reusing
Unix pipe/access validation and checking Windows pipe handles. Stale application
metadata is discarded rather than accepted as managed authority; explicit
native/task endpoints and consumer READY validation remain fail-closed. The first red test saw outer fd 6;
a later negative control, omitting the discard after caller-hygiene fixes, still
failed with pipes `[6, 7]` instead of only inner `[7]`. Restored code passed.

Fix-forward CI also corrected harness assumptions without weakening assertions:

- Linux reads procfs for parent/state facts instead of assuming procps `ps -p`
  exists in BusyBox/minimal target containers.
- FreeBSD traverses subordinate reaper domains because GETPIDS omits their
  descendants. Its pipe check scans the real descriptor limit; default devfs
  `/dev/fd` lists only 0, 1 and 2.
- CI runners leave unrelated pipes inheritable. The launcher makes those pipes,
  including Windows ambient stdio copies, non-inheritable before its first
  spawn. Scenarios are serialized to exclude another test's transient handles.
  The payload still enumerates all descriptors / snapshots all Windows handles
  and requires only its inner pipe beyond stdio. Windows stdio aliases are
  identified by kernel-object equality, not HANDLE numbers; unexpected handles
  retain server-PID diagnostics. Outer lifecycle/native pipes are not whitelisted.

At `ca0aaba`, [run 40677604462](https://github.com/xz-dev/procward/actions/runs/40677604462)
concluded **success on all twelve rows**, attempt 2. The ordinary unchanged-source
rerun followed an existing Darwin arm64 best-effort CLI fixture deadline failure
before nesting tests; its failed attempt and labelled harness rescue remain in
history. This does not establish a fix for that intermittent observation failure.

| Target | Result in 40677604462 |
| --- | --- |
| x86_64-unknown-linux-gnu | Passed, including Rust 1.87 check |
| aarch64-unknown-linux-gnu | Passed |
| x86_64-unknown-linux-musl | Passed |
| aarch64-unknown-linux-musl | Passed |
| riscv64gc-unknown-linux-gnu | All four nesting cases passed; **2 existing EMULATOR-UNSUPPORTED** cases |
| riscv64gc-unknown-linux-musl | All four nesting cases passed; **1 existing EMULATOR-UNSUPPORTED** case |
| x86_64-unknown-freebsd | Passed |
| aarch64-unknown-freebsd | Passed; outer/inner-loss completion 2.306s / 2.345s, both below 2.5s |
| x86_64-apple-darwin | Passed, best-effort |
| aarch64-apple-darwin | Passed on unchanged-source ordinary rerun, best-effort |
| x86_64-pc-windows-msvc | Passed; outer Job kill-on-close ended inner Job work in 2.138ms |
| aarch64-pc-windows-msvc | Passed; outer Job kill-on-close ended inner Job work in 4.739ms |

No Windows nested-Job containment defect was observed. Assignment remained
suspended-before-run with no breakaway request. Both owner-loss cases confirm
all retained inner process handles signalled; primary results and contract
isolation passed independently. The Windows pipe failures in earlier runs were
caller/stdio inheritance in the test harness, not a kernel Job failure.

Earlier fix-forward runs remain available: `40670356940` (initial harness
portability/inherited-runner-pipe failures), `40673115511` (Windows stdio copies
and FreeBSD arm64 preflight counted as fault time), `40674452842` (pipe-origin
diagnostics, plus an existing riscv64 GNU terminal fixture failure), and
`40676790053` (stdio alias diagnostics before final caller serialization).
No old failure was relabelled as a product pass or erased.

Final checkpoint local Linux suite: **48 passed**, run in a task-owned tmux session outside
the installed pi subreaper as described above. Native clippy `-D warnings`,
formatting, Rust 1.87 all-target checks, Windows/FreeBSD all-target cross-checks
and strict OpenSpec validation passed. Optional strict Windows clippy still
finds the pre-existing `unnecessary_unwrap` in `tests/fixtures/windows.rs:169`;
allowing only that known lint reports no other warnings. No unrelated lint fix
was added. FreeBSD's existing vendored process-wrap unused-import warning also
remains unchanged.

Tasks 4.7–4.9 are complete. Independent pre-tag review (5.5), owner-controlled
release prerequisites/publication (6), pi integration (7), and full-system
riscv64 coverage remain open. No tags, releases, settings or pi/pi-env code were
changed by this slice.

### Ordinary intermediaries

A late follow-up reproduced `procward run -- python3 -c 'subprocess.run(...)'`
returning 125: Python closes descriptors by default but retains the outer
application metadata. The approved correction treats stale/closed/mismatched
application metadata as non-authoritative, leaves unrelated non-pipe resources
alone, scrubs the outgoing environment, and always establishes fresh ownership.
It does not relax explicit native/task endpoint validation or a consumer's own
READY checks. Focused tests now cover both Python subprocess and `sh -c`, with a
real inner tree/result/cleanup check and a separate exhaustive contract-isolation
probe. Both intermediary cases passed locally. No architecture skip was added.

Python is a fixture prerequisite, not a procward runtime dependency. Native musl
CI installs it alongside its existing build tools; musl release and riscv64
fixture runtimes build the `ci/images` Dockerfiles from their unchanged pinned
FROM references with a Python package layer. Normal binary startup benchmarks
therefore do not include package installation. The final follow-up matrix must
confirm these added intermediary cases on all twelve rows; the earlier matrix
above remains evidence for the four direct-nesting cases, not that follow-up.
