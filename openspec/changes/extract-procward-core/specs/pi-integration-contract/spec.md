## Purpose

Let pi use an independently developed procward binary through a small managed-startup and lifecycle contract without importing pi into core testing or loading pi's full CLI twice.

## ADDED Requirements

### Requirement: Integrate after standalone release
Pi integration SHALL be a normal pi change after procward's independently tested v0.1.0 release. It SHALL bundle a matching binary and establish ownership before extensions/tasks and heavy CLI imports. The outer bootstrap SHALL remain lightweight and the full CLI graph SHALL load only in the inner managed runtime or explicit opt-out path.

#### Scenario: Default pi startup
- **WHEN** supported pi starts with ownership enabled
- **THEN** its bundled procward owns the inner runtime and instrumentation shows no full CLI import in the outer bootstrap

#### Scenario: Bundled procward is absent or incompatible
- **WHEN** pi cannot use its required binary/contract
- **THEN** startup fails visibly with the explicit opt-out diagnostic rather than silently running unmanaged

### Requirement: Avoid accidental session nesting
Pi SHALL recognize a versioned live direct-primary handoff validated against its immediate native guardian and SHALL NOT wrap that primary again. Environment strings alone SHALL NOT prove managed state. Stale/incomplete handoffs SHALL fail closed. Arbitrary-descendant discovery SHALL remain outside v0.1.0; deliberate per-task scopes SHALL remain possible.

#### Scenario: User runs procward around pi
- **WHEN** `procward run -- pi` launches pi as a validated direct managed primary
- **THEN** pi accepts the handoff and starts no second session wrapper

#### Scenario: Marker names a stale owner
- **WHEN** managed metadata disagrees with the live handoff or immediate-parent relationship
- **THEN** pi rejects it before application side effects

### Requirement: Preserve pi ownership opt-outs
Pi SHALL preserve `--no-process-ownership` for one invocation and `PI_NO_PROCESS_OWNERSHIP=1` for inherited nested invocations. Bootstrap SHALL respect option values and `--`. Opt-out SHALL skip pi-added session/task owners and managed hints, without claiming to escape an externally imposed procward scope.

#### Scenario: Environment disables nested pi ownership
- **WHEN** pi invocations inherit `PI_NO_PROCESS_OWNERSHIP=1`
- **THEN** they add no local session or task wrappers

#### Scenario: Flag-like string is an option value
- **WHEN** `--no-process-ownership` is supplied as another option's value
- **THEN** bootstrap does not mistake it for an ownership opt-out

### Requirement: Document the small lifecycle contract
The contract SHALL define version, managed identity/readiness, environment and fd/handle inheritance, owner-loss evidence, pre-hook shutdown notification, task cancellation, primary result and cleanup completion/error semantics. Applications SHALL not receive private native grant/anchor/Job handles. Primary exit or incomplete transport alone SHALL NOT mean cleanup success; Darwin acknowledgement SHALL be labelled best-effort.

#### Scenario: Task control ends without completion
- **WHEN** a task reports primary exit but its transport ends before valid cleanup completion
- **THEN** pi reports unconfirmed cleanup rather than success

#### Scenario: Managed application launches a tool
- **WHEN** pi starts an ordinary child
- **THEN** the child inherits no private native lease or reusable direct-primary claim that could keep ownership alive

### Requirement: Bound Unix owner-loss hooks independently
On Unix owner loss, pi SHALL request shutdown without restarting suspended input and allow responsive hooks up to 5 seconds. An independently armed watchdog SHALL enforce self-termination by 7 seconds from observed owner loss if the main loop cannot run. Native/observer wake SHALL allow paused work to enter this path. A surviving guardian SHALL be notified before signal hooks; re-entrant shutdown SHALL still arm owner-loss deadlines. Native fatal cleanup can preempt hooks and SHALL NOT have its deadline extended.

#### Scenario: Hook blocks the main event loop
- **WHEN** validated owner loss occurs after watchdog readiness and the main loop blocks
- **THEN** independent termination remains bounded by 7 seconds and native cleanup does not await the hook

#### Scenario: Owner disappears during an existing shutdown
- **WHEN** pi is already waiting for hooks when owner loss occurs
- **THEN** it arms the bounded owner-loss path despite the shutdown re-entry guard

### Requirement: Keep application policy and README changes in pi
Pi SHALL retain hook execution, UI redraw, packaging policy and application-specific task handling. Its normal owning patch-ref workflow SHALL carry the README process-tree ownership link to procward and other pi integration changes. Procward core CI/release SHALL not depend on those application tests.

#### Scenario: Pi feature list gains its procward link
- **WHEN** the adapter change is integrated through pi's current owning patch ref
- **THEN** the root README links its process-tree ownership feature to the released procward repository and preserves the macOS caveat

#### Scenario: Standalone core tests run
- **WHEN** procward fixtures run without pi installed
- **THEN** ownership and job-control acceptance remains executable without pi, models or extensions
