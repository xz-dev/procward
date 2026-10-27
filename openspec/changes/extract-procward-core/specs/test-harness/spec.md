## Purpose

Develop procward through small fresh fixture programs and observable behaviour, with early real-platform feedback instead of copying pi-env tests or reproducing its release ceremony.

## ADDED Requirements

### Requirement: Author independent fixtures without copying source
Procward SHALL use newly written fixture programs for argv/status, descendants, detached/stopped work, orphan reaping, hooks and terminal interaction. Pi-env SHALL be a behavioural reference and source of scenarios only; its implementation, tests and fixture source SHALL NOT be copied or translated. Core tests SHALL never launch pi or require Node/Bun/model credentials.

#### Scenario: Developer reproduces a historical orphan bug
- **WHEN** a release lesson describes zombies retained while the primary remained healthy
- **THEN** a fresh small fixture expresses that behaviour without importing pi-env's test implementation

#### Scenario: Standalone checkout runs tests
- **WHEN** the Rust project and platform tooling are available but pi is absent
- **THEN** its full applicable core suite executes through procward and its own fixtures

### Requirement: Sequence interaction by response
Interaction tests SHALL act, observe effect/output/state, and retry while an explicit response means not ready. Fixed sleeps SHALL NOT sequence input. Silence/no effect SHALL fail by the response deadline, and repeated hints SHALL not extend the overall deadline indefinitely. Observation polling and simulated work SHALL not be mistaken for elapsed-time readiness.

#### Scenario: Quit arrives before ready
- **WHEN** a fixture returns a not-ready hint to a quit attempt
- **THEN** the harness observes that response and retries within the original deadline rather than assuming a delay or terminal setup escape proves readiness

#### Scenario: Program stops responding
- **WHEN** an action produces neither its expected effect nor a not-ready response
- **THEN** the test fails on its bounded deadline with the interaction transcript

### Requirement: Check safety and completion facts
Fixtures SHALL cover startup/owner-loss races, stopped/detached descendants, live identity/PID reuse, adopted zombies, terminal authority, control truncation and leaked descriptors/handles. Stopped means alive; zombie means not yet reaped; query failure means unknown/error, not absence. Tests SHALL demonstrate unrelated work survives and primary status remains intact.

#### Scenario: Cleanup claims success with a stopped child
- **WHEN** a stopped owned identity remains after completion is claimed
- **THEN** the row fails instead of treating the child as exited

#### Scenario: Grandchild is adopted while primary stays healthy
- **WHEN** an orphan fixture exits a grandchild before its primary
- **THEN** tests observe reaping during healthy operation, not only eventual disappearance at shutdown

### Requirement: Preserve deadline assertions and report rescue honestly
Fault tests SHALL measure at most 8 seconds from injection to platform completion with default settings, with default 6/3-second grace and 7.5-second cleanup policy checked separately where applicable. Non-default policy/failover tests SHALL enforce the original configured deadline from first cleanup start, not restart timing when a native role dies. Startup/emulation time SHALL not relax native fault deadlines. If qemu-user execution needs a wider emulated-row budget, its explicit scale factor SHALL be documented and confined to emulated rows; identity/order/liveness/completion predicates and configured-deadline-relative checks SHALL remain intact. Harness rescue SHALL be separate from product cleanup and SHALL not turn failed assertions into success.

#### Scenario: Slow runner reaches startup late
- **WHEN** readiness consumes a larger setup budget before the fault
- **THEN** the fault-relative teardown budget remains unchanged

#### Scenario: Harness kills a leaked fixture
- **WHEN** test cleanup must rescue a known survivor
- **THEN** the failed run stays failed and identifies the rescue

### Requirement: Use early CI and ordinary fix-forward work
As soon as the Linux walking skeleton compiles, development SHALL publish it and enable the full push/PR platform matrix rather than waiting for all backends. Failing applicable platform tests SHALL remain visible as work lands. Developers SHALL be able to rerun flaky jobs normally and fix forward; once-only packets, sealed manifests, pre-rerun classification approvals and per-step independent reviews SHALL NOT be required. Reruns SHALL not alter assertions or erase previous logs.

#### Scenario: First Windows CI row is red
- **WHEN** the early skeleton reaches Windows before its backend is complete
- **THEN** developers keep the row visible and implement/fix it through normal commits rather than disable required v0.1.0 coverage

#### Scenario: Suspected flaky job is rerun
- **WHEN** a developer reruns a flaky CI job
- **THEN** the rerun is permitted without a special packet or review ritual and original logs remain attributable

### Requirement: Release only complete applicable coverage
Before v0.1.0, the full applicable suite SHALL pass for Linux GNU/musl x64/arm64/riscv64, FreeBSD x64/arm64 guests, macOS x64/arm64 and Windows x64/arm64. The two riscv64 rows SHALL execute under explicitly labelled, pinned qemu-user/container emulation; host-kernel syscall semantics and nonrepresentative timings SHALL be disclosed. Test output SHALL identify actual source/target/executor and results without a custom sealed evidence system. Build-only success SHALL not replace runtime acceptance.

#### Scenario: An emulated capability is absent
- **WHEN** runtime probing in an explicitly labelled qemu-user executor finds seccomp installation ENOSYS or broken std spawn error reporting for a known-missing path
- **THEN** that case emits `EMULATOR-UNSUPPORTED` and its reason is counted/listed in the job summary, never called a product pass
- **AND** there is no architecture-based skip or weakened native/other assertion, and genuine primary status 127 remains tested
- **AND** full-system riscv64 VM coverage is a documented follow-up for unsupported cases

#### Scenario: Target compiles but fixtures fail
- **WHEN** a target builds successfully without passing its applicable fixtures
- **THEN** it remains incomplete for release even though early development pushes are allowed
