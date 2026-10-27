## Purpose

Give arbitrary commands a bounded owned process-tree lifetime without signalling unrelated processes, retaining adopted zombies or losing the command's actual result.

## ADDED Requirements

### Requirement: Run an owned command
Procward SHALL support `procward run [options] -- <cmd> [args...]`, execute argv directly without adding a shell, and clean up its owned tree on primary exit, owner death, termination signals or terminal loss. On macOS this guarantee SHALL cover observed descendants within the documented best-effort boundary.

#### Scenario: Primary exits before a detached descendant
- **WHEN** a fresh fixture creates a detached descendant, ownership/observation is established and the primary exits with status 23
- **THEN** procward cleans up the owned descendant before reporting successful completion with status 23

#### Scenario: Owner dies during active work
- **WHEN** an owner is killed while its tree contains running and stopped descendants and an independent cleanup role survives
- **THEN** the tree is cleaned up within the teardown bound without requiring `fg` or cooperation from the command

### Requirement: Preserve inputs and fail startup safely
Procward SHALL preserve argument boundaries, cwd, environment and stdio destinations except for the documented managed handoff and necessary terminal isolation. Invalid options, missing commands, invalid inherited endpoints or required ownership setup failure SHALL fail visibly before payload side effects, without silently launching unmanaged.

#### Scenario: Argument contains spaces and option-like text
- **WHEN** `procward run -- <fixture> "two words" --literal` runs with stderr redirected
- **THEN** the fixture receives exactly those two arguments and stderr keeps its destination

#### Scenario: Required ownership setup fails
- **WHEN** startup cannot acquire its required backend or validate its launch permission
- **THEN** procward reports failure and the fixture's payload marker is absent

### Requirement: Use live identity as signal authority
Procward MUST NOT signal unrelated processes through stale PIDs/PGIDs. Nondirect targets SHALL be birth-validated or held by stable OS handles; unreaped direct-child ownership or a kernel-owned subtree SHALL provide corresponding authority. Query failure SHALL NOT prove absence. Stopped processes SHALL count as alive.

#### Scenario: Numeric PID is reused
- **WHEN** a previously owned process exits and its PID is reused by unrelated work
- **THEN** the stale observation cannot authorize signalling the new process

#### Scenario: Process inventory is denied
- **WHEN** an inventory or identity query fails
- **THEN** cleanup reports the failure rather than declaring the target gone

### Requirement: Keep teardown bounded and ordered
Unix cleanup SHALL deliver TERM before any teardown CONT to a stopped target, then escalate to KILL at the selected grace boundary. Linux/FreeBSD abnormal-owner cleanup SHALL allow 6 seconds, or 3 seconds only when an already saved helper wait status proves fatal signal termination at selection, within one 7.5-second cleanup deadline. EOF alone SHALL NOT select the fatal-only grace. Adoption and later status discovery SHALL NOT restart grace/deadline, and no rescue CONT SHALL occur after that policy permits it or repeatedly during active shutdown. Native fault teardown SHALL complete within 8 seconds of fault injection in acceptance tests.

#### Scenario: Fatal helper status is already known
- **WHEN** cleanup selects policy with a saved signalled helper status
- **THEN** it selects the 3-second grace, delivers TERM before permitted CONT and retains the original 7.5-second deadline

#### Scenario: EOF precedes waitable exit
- **WHEN** cleanup begins on EOF before fatal helper status is available
- **THEN** it selects the ordinary 6-second grace and does not reselect it when status later arrives

#### Scenario: Adoption occurs after grace
- **WHEN** a stopped descendant is adopted after the selected grace expires
- **THEN** cleanup continues escalation/reaping without a fresh grace or rescue CONT

### Requirement: Reap orphans during healthy work
Reaper-capable Unix backends SHALL promptly reap exited adopted children while the primary remains alive, without consuming or overwriting the primary's result. Reaping SHALL NOT wait until overall shutdown.

#### Scenario: Adopted grandchild exits first
- **WHEN** a fixture orphans a grandchild that exits while the primary continues running
- **THEN** the owning reaper removes the zombie during healthy operation and later preserves the primary's result

### Requirement: Separate primary exit from tree completion
Procward SHALL preserve primary exit status after successful cleanup, including Unix signal-derived status and native Windows 32-bit status. Primary exit, helper close or a drop guard alone SHALL NOT be accepted as proof of whole-tree cleanup. Known survivors, denied signals and expired deadlines SHALL fail cleanup; macOS completion SHALL explicitly mean a completed best-effort sweep.

#### Scenario: Primary exited but descendants remain
- **WHEN** a primary exit is observed before its descendants are gone
- **THEN** procward starts/finishes tree cleanup rather than resolving on primary exit alone

#### Scenario: Command dies from signal 15
- **WHEN** a Unix primary dies from signal 15 and cleanup succeeds
- **THEN** the CLI reports status 143

#### Scenario: Completion cannot be proved in time
- **WHEN** cleanup reaches its deadline with a known survivor or missing required completion evidence
- **THEN** procward returns cleanup failure, not a successful acknowledgement
