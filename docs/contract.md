# Managed-child contract, version 1

`procward run -- <command>` grants its **direct primary** an application-only
lifecycle pipe. This is generic application integration, not a pi mode or a
public Rust library. Environment alone never proves managed state.

The consumer discipline follows OS-standard socket activation: validate the
intended process, consume environment once, and make the endpoint close-on-exec
before starting children. See [sd_listen_fds(3)](https://www.freedesktop.org/software/systemd/man/latest/sd_listen_fds.html).
Unlike socket activation, this pipe carries bounded lifecycle records, not
application sockets.

## Environment

| Variable | Value |
| --- | --- |
| `PROCWARD_PROTOCOL` | Exactly `1` |
| `PROCWARD_OWNER_PID` | Decimal immediate native parent's PID |
| `PROCWARD_CONTRACT_ID` | Fresh 128-bit launch ID, 32 lowercase hexadecimal characters |
| `PROCWARD_LIFECYCLE_FD` | Unix read-only pipe fd, decimal integer greater than 2 |
| `PROCWARD_LIFECYCLE_HANDLE` | Windows inherited read-pipe HANDLE, unsigned decimal value |

Exactly one platform transport variable is present. `PROCWARD_PRIMARY_PID` is
**not** emitted: primary PID is in READY on every platform. Windows must prepare
its environment before CreateProcess returns a PID; no environment injection or
extra process layer is used. Existing contract variables are replaced, not
forwarded as a reusable claim.

## Wire records

UTF-8/ASCII lines, LF terminated. Each complete line, including LF, is at most
**256 bytes**. Decimal PIDs are positive native PIDs. Unknown versions, wrong
field counts, malformed values, wrong launch IDs, overlong records and partial
records at EOF are invalid.

```text
PROCWARD/1 READY <owner_pid> <primary_pid> <owner_identity> <launch_id>\n
PROCWARD/1 OWNER-LOSS <launch_id>\n
```

READY is first and appears once. Its owner PID and launch ID must match the
environment. Its primary PID must equal the consumer's own live PID, not a PID
inherited from an ancestor.

On Unix, `owner_identity` is `parent`: `getppid()` must match the announced owner.
The kernel's immediate-parent relationship binds that identity while the
primary lives; a departed parent reparents the child rather than authorizing a
reused numeric PID. Check this relationship at acceptance, not only when reading
environment.

On Windows, `owner_identity` is the owner's 64-bit creation FILETIME, encoded as
16 lowercase hexadecimal characters. Check the primary's native parent PID,
open that owner with query/synchronize rights, compare GetProcessTimes creation
time and confirm the handle is still live. A reused numeric parent PID cannot
pass the creation-time check. The application receives **no Job handle**.

Use a startup response deadline. Poll/retry only while transport is not ready;
neither silence nor an endless partial record can extend that deadline. EOF
before READY is failed readiness. The reference fixture uses a two-second
startup response deadline; that is fixture policy, not a procward hook budget.

## Consume before starting tools

After validating READY:

1. Set `FD_CLOEXEC` on Unix, or clear `HANDLE_FLAG_INHERIT` on Windows.
2. Unset every contract environment variable above (and discard any obsolete
   `PROCWARD_PRIMARY_PID` metadata).
3. Only then launch ordinary children or load code that can launch them.
4. Retain the read endpoint for owner-loss notification; close it at shutdown.

A consumer that deliberately leaves its application endpoint inheritable can
leak that endpoint. It cannot keep a native ownership lease or Job alive:
application transport is separate from those private non-inheritable handles.
An ordinary descendant cannot reuse the claim: its own PID and immediate-parent
identity differ, and a pipe's consumed READY cannot be rewound. This is a
lifecycle convention, not a security boundary against a malicious parent or
application forging its own state.

## Owner loss and shutdown

The surviving native owner queues OWNER-LOSS before starting its termination
signals, then closes the application writer. Accept a notice only for the
validated launch ID. EOF without a notice **after** valid READY also means owner
loss, never successful cleanup. Do not confuse the startup EOF rejection with
this active-lifecycle failure path.

An application can drain the notice before running its own signal hooks and
cooperate during the selected grace. The application must handle TERM if it
needs that opportunity: default signal termination can preempt application
callbacks. Procward's independent TERM/CONT/KILL policy does not await a hook,
an application acknowledgement or terminal output. Its existing `--grace`,
`--fatal-grace` and `--cleanup-deadline` options select the bound; notification
never restarts or extends it. Fixed application hook/watchdog durations are not
part of this contract.

Windows kill-on-last-Job-handle-close can kill the primary before it observes a
notice or EOF. Notification/cooperation there is best-effort; Job containment
remains kernel-enforced. On macOS, READY does not prove descendant observation;
cleanup retains the documented best-effort/pre-observation limitation.

## Standalone reference consumer

`tests/fixtures/managed.rs` is a test-only consumer, not an installed API.
`tests/contract.rs` checks real procward startup, ordinary-child claim/endpoint
removal, parent/primary/launch identity mismatches, unsupported versions,
truncation, oversized records and EOF before readiness. Applications, including
pi's later adapter, implement the same documented boundary independently.
