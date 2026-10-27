## Purpose

Get full-platform feedback early on every development event and publish only reviewed version-tag releases with immutable assets, authenticated provenance and checksums.

## ADDED Requirements

### Requirement: Enable full every-event CI early
GitHub Actions SHALL be enabled when the first Linux skeleton compiles and SHALL run the full applicable standalone fixture suite on every branch `push` and `pull_request` event head. No path/branch filter, fail-fast cancellation or newer-event cancellation SHALL silently remove required coverage. A multi-commit push SHALL test its head rather than enumerate intermediate SHAs. All tests SHALL use procward fixtures, never pi.

#### Scenario: Documentation-only change is pushed
- **WHEN** a documentation-only commit is pushed to a branch or PR
- **THEN** the full applicable platform matrix runs

#### Scenario: New commit arrives before prior tests finish
- **WHEN** another event updates the same branch
- **THEN** earlier required event tests remain runnable rather than being cancelled in favor of the latest revision

### Requirement: Run the four-platform target matrix
CI SHALL execute applicable unit, CLI/contract, lifecycle and terminal tests on Linux GNU/musl x64/arm64/riscv64, FreeBSD x64/arm64 through a VM action, macOS x64/arm64 and Windows x64/arm64 including console/ConPTY: twelve required target rows. The riscv64 targets SHALL be `riscv64gc-unknown-linux-gnu` and `riscv64gc-unknown-linux-musl`, executed with pinned qemu-user/binfmt and digest-pinned target containers, not build-only substitutes. Their logs/docs SHALL say “emulated (qemu-user) — semantics of prctl/pidfd/process groups are passed through to the host kernel; timings are not representative”. Early red rows SHALL support fix-forward development and normal reruns; all applicable rows SHALL pass before the first release tag.

#### Scenario: riscv64 has no hosted runner
- **WHEN** every-event CI runs either riscv64 target
- **THEN** it cross-builds release-profile binaries/tests with the official rustup toolchain for the exact target triple and executes them inside the matching GNU/musl riscv64 container under qemu-user
- **AND** pinned actions/images and explicit emulation/host-kernel/timing labels prevent presenting the result as native riscv64 hardware validation

#### Scenario: Emulator cannot implement a native capability
- **WHEN** a runtime probe in an explicitly labelled qemu-user executor observes seccomp installation returning ENOSYS or std spawn returning a child exiting 127 for a known-missing path
- **THEN** the affected test reports `EMULATOR-UNSUPPORTED`, with its count and reason listed in the job summary rather than claiming a product pass
- **AND** every other assertion and native run stays strict, without target-architecture skips or product workarounds
- **AND** full-system riscv64 VM coverage remains a documented follow-up for those unsupported cases

#### Scenario: Early backend is unfinished
- **WHEN** a required platform is not yet complete
- **THEN** its failures are visible during development and are not relabelled as release passes

#### Scenario: Flaky test is rerun
- **WHEN** a developer requests an ordinary CI rerun
- **THEN** it runs without sealed manifests, once-only dispatch rules or per-step review gates while retaining the previous attempt's logs

### Requirement: Obtain one independent review before the first tag
Before pushing `v0.1.0`, the project SHALL obtain one independent implementation review covering ownership/terminal safety, crate/API assumptions, tests and release setup, and SHALL address its blocking findings. Per-commit or per-step independent reviews SHALL NOT be mandatory process gates.

#### Scenario: Skeleton compiles before review
- **WHEN** the first compiling skeleton is ready
- **THEN** it can be pushed for CI immediately without waiting for the pre-release review

#### Scenario: Reviewer finds a cleanup defect
- **WHEN** the independent pre-first-tag review identifies a blocking ownership defect
- **THEN** it is fixed and validated before `v0.1.0` is tagged

### Requirement: Publish only from stable version-tag pushes
Releases SHALL be cut only by pushing exact `vX.Y.Z` version tags, beginning with `v0.1.0`. The tag workflow SHALL validate package/tag agreement, build all twelve targets (including riscv64 GNU and musl) and rerun the full applicable suite from the tagged source and release binaries. Branch/PR/manual/live-release updates SHALL not publish. A failed/missing tag row SHALL block publication; corrections SHALL use a new tag rather than overwrite a published version.

#### Scenario: Branch CI passes
- **WHEN** a branch passes CI without a version-tag push
- **THEN** no public release is created or updated

#### Scenario: First version tag is pushed
- **WHEN** `v0.1.0` is pushed for package version `0.1.0`
- **THEN** all release targets are rebuilt/retested before publication is eligible

#### Scenario: riscv64 release assets are prepared
- **WHEN** a version tag is eligible for publication
- **THEN** both riscv64 GNU and musl binary/package assets are included in the frozen downloads, SHA256SUMS and provenance attestations
- **AND** missing/failed emulated fixture rows block publication just like other required targets

#### Scenario: Prior branch pass differs from tag result
- **WHEN** a required tag-workflow row fails
- **THEN** publication is blocked despite an earlier branch pass

### Requirement: Use GitHub immutable draft-first publication
A repository owner SHALL enable **Enable release immutability**, record confirmation and recheck it before the first tag. The workflow SHALL stage all final assets in a draft before publishing and then verify release/asset integrity with GitHub CLI. Published assets and tag SHALL be protected by GitHub immutability; title/notes SHALL not be falsely described as immutable. Setting drift SHALL be disclosed as a residual risk detected by post-publication verification, not hidden by an invented token permission.

#### Scenario: Owner prepares the first tag
- **WHEN** the first release is prepared
- **THEN** owner confirmation precedes the tag and all assets are attached before publication

#### Scenario: Immutability setting drifted
- **WHEN** publication produced a mutable release after the setting was disabled
- **THEN** post-publication verification fails loudly without pretending it prevented publication or automatically repairs protected assets

### Requirement: Publish native artifact provenance and checksums
Release binaries/packages and `SHA256SUMS` SHALL have GitHub OIDC/Sigstore-backed build-provenance attestations through the currently recommended `actions/attest`. `SHA256SUMS` SHALL cover exact downloadable platform assets without self-hashing. Consumers SHALL be able to run `gh attestation verify` against expected repository/workflow/tag/source and verify checksums. GitHub's automatic immutable-release attestation SHALL complement, not replace, artifact provenance. The older `actions/attest-build-provenance` wrapper SHALL be documented accurately.

#### Scenario: Downloaded package is checked
- **WHEN** a consumer downloads a package and checksum manifest
- **THEN** checksums and `gh attestation verify` can validate their bytes and expected provenance

#### Scenario: Bytes change after attestation
- **WHEN** candidate bytes are modified after checksumming/attesting
- **THEN** verification rejects publication

### Requirement: Keep credentials role-limited
Ordinary push/PR CI SHALL use read-only repository access without signing/publication credentials. Trusted tag attestation jobs SHALL use `contents: read`, `id-token: write` and `attestations: write`; only publishing jobs SHALL receive `contents: write`. Admin credentials SHALL NOT be added for the manual immutability-setting check. Reviewed immutable action revisions and digest-pinned container/binfmt images SHALL be used.

#### Scenario: Fork PR runs fixtures
- **WHEN** an untrusted PR enters CI
- **THEN** it runs tests without OIDC signing or repository write permission

#### Scenario: Tag workflow signs and publishes
- **WHEN** trusted release jobs execute
- **THEN** attestation and publishing permissions are limited to their respective jobs
