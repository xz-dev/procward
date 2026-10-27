## Purpose

Provide four-platform ownership with standard facilities, explicit startup failures and honest boundaries rather than assuming that process groups or drop guards supply universal containment.

## ADDED Requirements

### Requirement: Complete all four platforms for v0.1.0
Procward v0.1.0 SHALL support Linux, FreeBSD, macOS and Windows. Linux-first development SHALL be an ordering choice, not a released reduction of scope. An unfinished backend SHALL appear as failed/incomplete development CI, not a false passing release result.

#### Scenario: Early skeleton lacks a backend
- **WHEN** the first compiling skeleton reaches the full CI matrix before all backends work
- **THEN** failing rows remain visible for fix-forward work and v0.1.0 is not tagged until applicable coverage passes

### Requirement: Retain Linux reaper and identity guarantees
Linux SHALL provide child-subreaper ownership, safe stable-target signalling and a reliable owner-loss wake for paused work. Missing pidfd freeze support SHALL be visible and SHALL NOT silently disable reaper cleanup. Required wake/setup failure SHALL refuse managed startup.

#### Scenario: pidfd freeze support is unavailable
- **WHEN** the kernel denies or lacks required nondirect freezing capability
- **THEN** procward reports that limitation while keeping identity-safe reaper cleanup active

#### Scenario: Owner dies during startup
- **WHEN** the parent disappears between arming its death wake and granting payload execution
- **THEN** startup denies payload side effects and cleans up already created roles

### Requirement: Use FreeBSD reaper ownership and targeted wakes
FreeBSD SHALL provide kernel reaper ownership including nested reapers, fresh owned-child enumeration/signalling and a reliable targeted wake for paused cleanup. Wake setup failure SHALL refuse startup without host sysctl/module changes; query/signal failures SHALL remain failures.

#### Scenario: Stopped descendant is adopted
- **WHEN** a FreeBSD reaper adopts a stopped descendant before grace expires
- **THEN** TERM precedes any permitted CONT and the existing escalation deadline remains unchanged

#### Scenario: Required wake cannot be established
- **WHEN** startup cannot arm the necessary targeted owner-loss wake
- **THEN** it fails visibly before payload execution and changes no host configuration

### Requirement: State macOS best-effort limits
macOS SHALL retain observed process identities across reparenting and validate PID, birth and relevant credentials before signals. An independent observer SHALL preserve owner-loss cleanup progress. Completion SHALL mean a best-effort sweep with no known survivors, not kernel containment. Errors SHALL NOT count as absence.

#### Scenario: Observed descendant detaches and reparents
- **WHEN** a previously observed descendant changes session/parent without changing its validated identity
- **THEN** it remains a cleanup target while unrelated processes are excluded

#### Scenario: Descendant escapes before observation
- **WHEN** a fixture detaches and reparents to launchd before the first observation
- **THEN** the test/documentation identify the best-effort escape window and attribute fixture rescue separately rather than claim universal no-leak containment

#### Scenario: Observer is lost
- **WHEN** a required observer dies while normal owners remain
- **THEN** the session fails closed rather than silently running without its cleanup anchor

#### Scenario: Observer dies before the outer inventory's first sweep
- **WHEN** observer loss reparents a descendant before the surviving owner ever observed its identity
- **THEN** procward reports failure 125 and the explicit pre-observation best-effort limitation
- **AND** a fixture may clean known test PIDs separately as labelled harness cleanup, without crediting product containment

#### Scenario: Observed inventory survives observer loss
- **WHEN** a native test establishes retained identities before killing its observer
- **THEN** the same production fallback path cleans those observed identities and returns the defined failure status without harness rescue

### Requirement: Establish Windows Job containment before payload work
Windows SHALL assign the command to a non-inheritable kill-on-last-handle-close Job before untrusted payload work. Dropping/killing an owner SHALL not leave a leaked Job handle that keeps descendants alive. Normal cleanup SHALL confirm Job completion; Unix process-group assumptions SHALL NOT substitute for this guarantee.

#### Scenario: Owner is killed after launch
- **WHEN** a Job-owning procward process is killed while descendants run
- **THEN** the last ownership handle closes and Job members terminate without executing a Rust destructor

#### Scenario: Assignment or launch permission fails
- **WHEN** ownership assignment fails or the owner disappears before launch permission
- **THEN** payload side effects are absent and suspended children/handles are cleaned up

### Requirement: Document unsupported failure boundaries
Procward SHALL disclose privilege transitions that clear owner-death wakes, loss of all independent cleanup roles, host/kernel failure and macOS unseen escape. Standard crates SHALL NOT be represented as removing these OS limitations.

#### Scenario: Runtime transition clears death notification
- **WHEN** a set-ID/capability/credential transition invalidates the required wake
- **THEN** documentation excludes the corresponding paused-owner guarantee instead of promising unsupported recovery
