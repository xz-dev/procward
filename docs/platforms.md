# Platform lifecycle guarantees

These are current noninteractive backend guarantees, not a claim that pending
terminal/contract/release tasks are complete.

| Backend | Authority and completion | Limits |
| --- | --- | --- |
| Linux | Subreaper ancestry, birth-checked pidfds or unreaped direct-child authority; wait until ECHILD | Missing pidfds visibly limit nondirect freezing, not direct-child reaper cleanup |
| FreeBSD | Acquired procctl reaper domain, fresh status/pids, nested-reaper subtree KILL and child waits | Required targeted wake/query/signal errors fail startup or cleanup; no sysctl/module changes |
| Windows | process-wrap Tokio JobObject + KillOnDrop, assignment while suspended, explicit Job completion after primary exit | No inherited/leaked private Job handle; Unix job control is not advertised |
| macOS | Independent observer, retained PID/birth/effective/real/saved-UID observations; completed sweep with no known survivors | Best-effort, not kernel containment; reparenting before outer observation (including early observer loss), fast setsid, final-scan forks and final identity-check/use races can escape |

Linux/FreeBSD have a shell-facing owner and a separate-process-group reaper
anchor. Both can reap; the surviving role cleans after the other dies. The
anchor's required parent-death SIGCONT wake is armed and its live parent checked
before launch permission. A close-on-exec pipe carries the one launch grant and
then owner liveness, not a payload-inherited ownership lease. macOS uses the same
small native arrangement, with polling and retained inventories in both roles;
its observer remains outside the stopped workload rather than claiming an
unavailable parent-death syscall.

Loss of all independent roles, host/kernel failure and privilege transitions
that clear parent-death notification are excluded. A killed process cannot run
its own destructor or terminal cleanup. Standard crates do not remove those OS
limits. Unavailable or denied queries are errors, never evidence of absence.
Stopped work is alive; zombies are not reaped completion.

## riscv64 execution and release scope

Required additional targets: `riscv64gc-unknown-linux-gnu` and
`riscv64gc-unknown-linux-musl`. Both are **emulated (qemu-user) — semantics of
prctl/pidfd/process groups are passed through to the host kernel; timings are not
representative**. This is not native riscv64 kernel/hardware validation.

GitHub has no native riscv64 runner. CI registers only riscv64 binfmt via a
full-SHA-pinned `docker/setup-qemu-action` and digest-pinned binfmt image, then
builds release-profile binaries/tests on x64 with the official rustup toolchain
for the exact `riscv64gc-unknown-linux-gnu` / `riscv64gc-unknown-linux-musl` target,
then executes them in digest-pinned Debian GNU/Alpine musl containers. GNU uses
the distro cross GCC/sysroot; musl uses Rust's shipped self-contained runtime and
linker. No Alpine compiler triple alias is substituted for the release target.
The workspace is mounted at its original absolute path so compiled-in fixture
paths remain valid. Source SHA, compiler host, selected target, RISC-V ELF machine
and executor/caveat are printed. Tests are serialized to avoid emulator CPU
contention; timing scale remains **1×**, with unchanged timing, ordering, identity,
liveness and completion assertions. Any wider budget must be explicitly
documented and confined to emulated rows, never native rows.

Two runtime capabilities can be absent in qemu-user, independent of target
architecture: seccomp installation can return `ENOSYS`, and libc's
`CLONE_VFORK`/`posix_spawn` path can return a child exiting 127 for a known-missing
executable rather than reporting the spawn error. Only an explicitly labelled
qemu-user test executor plus the matching runtime probe can report
`EMULATOR-UNSUPPORTED` for the required-wake-denial or exec-error test. Other
errors remain failures. These are **not product passes**: each job summary counts
and lists observed unsupported cases. Real exit status 127 is separately tested;
it is never relabelled as a successful spawn-error result. Native/real riscv64
runs keep all original assertions because they do not use this emulation flag.
No product code or native behavior is changed to work around emulator limits.

Follow-up: add a full-system riscv64 VM job (`qemu-system`) for complete kernel,
seccomp and spawn-error coverage. qemu-user cannot supply evidence for its
explicitly listed unsupported cases.

Both riscv64 variants are required future release binary/package assets, covered
by the same checksum, provenance and full tagged-source fixture gates as other
targets. Their inclusion does not claim a new pi runtime/bundle label.

## Cleanup timings

```sh
procward run --grace 6s --fatal-grace 3s --cleanup-deadline 7500ms -- command args
```

Defaults are ordinary grace **6s**, already-saved-fatal grace **3s**, and cleanup
deadline **7500ms**. Durations use integers with `ms` or `s` suffix;
validation requires `0 < fatal-grace <= grace < cleanup-deadline`. Malformed,
missing, duplicated or out-of-order timing values return **2 before payload
execution**. Unix uses both grace values; Windows terminates its Job on primary
exit and uses the configured completion deadline, not Unix signal grace.
Pi will pass its own chosen values via these generic options in its later adapter.

Unix cleanup selects one **global per-tree** monotonic policy. EOF-first uses
configured ordinary grace; only already-saved fatal helper status selects
configured fatal grace. Before cleanup actions, the anchor publishes one bounded
private record containing CLOCK_MONOTONIC start, configuration and selection.
A separate CLOEXEC policy pipe never reaches the payload. Outer fallback keeps
that original start/selection: late role loss, adoption and later status discovery
cannot restart grace or deadline. Without a published record, the anchor had not
started cleanup and outer selects its first policy at fault observation.

TERM precedes one permitted CONT per newly owned direct child during grace;
escalation uses KILL without fresh CONT. Unknown completion evidence or known
survivors at the configured deadline fail cleanup. Default runtime fault checks
remain **8 seconds**; a non-default 2s/1s/2500ms late-anchor-loss fixture enforces
the original configured deadline and also checks loss before publication.

macOS keeps an ESRCH-with-existing-PID identity transition as unknown, not gone:
it neither signals from that observation nor completes while it remains pending.
Actual denied/other queries remain errors. FreeBSD KILL ESRCH is tolerated only
when a fresh kernel list proves no signalable member remains; wait/reaper facts,
not that signal result, still decide completion.

Observer loss returns failure **125**, not an unmanaged success. macOS reports
that descendants reparented before the outer inventory's first observation may
escape. A native test explicitly primes the real retained inventory, kills its
observer, and exercises the same outer fallback function to prove observed-tree
cleanup and failure status. The CLI early-observer-loss case checks failure and
that disclosure; any unobserved survivors are removed by separately labelled
**harness cleanup**, not credited to procward.

The Darwin unseen-escape fixture deliberately waits until its parent exited
before the first inventory observation. Its surviving worker is reported as the
expected platform limitation and rescued separately. This passing limitation
demonstration is not evidence of universal containment or product cleanup of
that worker.
