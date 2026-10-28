# CI and releases

## Every-event checks

`.github/workflows/ci.yml` runs on every push and pull request, including docs-only
changes. No path/branch filter, fail-fast or newer-event cancellation removes a
required row. Jobs have `contents: read` only. See [development](development.md)
for fixture commands and ordinary rerun history, [platforms](platforms.md) for
backend boundaries, and [contract](contract.md) for application integration.

| Target | Fixture executor |
| --- | --- |
| x86_64-unknown-linux-gnu | Ubuntu x64 |
| aarch64-unknown-linux-gnu | Ubuntu arm64 |
| x86_64-unknown-linux-musl | Native x64 Alpine container |
| aarch64-unknown-linux-musl | Native arm64 Alpine container |
| riscv64gc-unknown-linux-gnu | Official rustup target, Debian riscv64 under qemu-user |
| riscv64gc-unknown-linux-musl | Official rustup target, Alpine riscv64 under qemu-user |
| x86_64-unknown-freebsd | FreeBSD 14.3 x64 VM |
| aarch64-unknown-freebsd | FreeBSD 14.3 arm64 VM |
| x86_64-apple-darwin | macOS x64 |
| aarch64-apple-darwin | macOS arm64 |
| x86_64-pc-windows-msvc | Windows x64, including console/ConPTY |
| aarch64-pc-windows-msvc | Windows arm64, including console/ConPTY |

Riscv64 is **emulated (qemu-user) — semantics of prctl/pidfd/process groups are
passed through to the host kernel; timings are not representative**. Timing scale
is 1×. Runtime probes label seccomp `ENOSYS` and broken missing-path std-spawn
reporting `EMULATOR-UNSUPPORTED`, with counts/reasons in job summaries, not product
passes. Full-system riscv64 VM coverage remains a follow-up. Other/native
assertions and fault deadlines stay unchanged. Actions use full commit SHAs;
container and binfmt images use digests.

## Size-first distribution builds

`[profile.dist]` inherits release and explicitly sets fat LTO, one codegen unit,
symbol stripping, no debuginfo, no incremental compilation and `panic = "unwind"`.
The default `opt-level = "z"` is the smallest in the local comparison below.
Release jobs measure **2, 3, s and z independently on every target**, select the
smallest binary (first measured wins an exact size tie), then rebuild its binary
and fixtures with that setting. Startup never changes this choice. Every-event
CI reports default-dist size/startup in its job summary; release/dry-run summaries
include all four candidates, chosen setting and both zip sizes. Raw measurements
are retained in `measurements.json` alongside dependency inspection output.

[Cargo profile reference](https://doc.rust-lang.org/cargo/reference/profiles.html):
`codegen-units = 1` with default `lto = false` does not enable LTO, so fat LTO is
explicit. Tests ignore the panic setting; the shipped binary keeps unwinding.
Abort remains deferred until a controller-panic fault seam proves cleanup safety.
Debug flavour uses the unoptimized dev profile with debuginfo, not stripped dist.
Windows debug zips include `procward.pdb` beside the executable (presence/size
asserted). macOS debug builds set `split-debuginfo = "packed"` and include the
Apple-standard `procward.dSYM` bundle, checked with `dwarfdump --verify`.
Linux/FreeBSD retain embedded DWARF; `readelf` asserts debug info/line sections.
Symbol files are covered by the zip's checksum/provenance and compared byte-for-byte
at fan-in; binaries inside are also individual attestation subjects.

Local comparison, x86_64-unknown-linux-gnu, rustc 1.99.1 / LLVM 23.1.1, same
source and profile except opt-level (2026-10-28):

| opt-level | Binary bytes | Startup median ms |
| --- | ---: | ---: |
| 2 | 568544 | 0.240 |
| 3 | 573648 | 0.260 |
| s | 531248 | 0.331 |
| **z** | **521752** | **0.336** |

Startup measurement: 21 warm-cache `procward --version` subprocess launches after
3 warmups, wall-clock median, no performance gate. It measures CLI startup, not
managed-command setup or cleanup. Linux container measurements include container
launch overhead where a runner is used; qemu-user also includes emulation. These
numbers cannot compare hardware/OS performance and do not relax fixture deadlines.

Windows x64/arm64 set `+crt-static` in `.cargo/config.toml`; `dumpbin /DEPENDENTS`
rejects dynamic MSVC/UCRT runtime imports. Windows still imports OS DLLs. Musl
builds use official exact-target Rust, static CRT and self-contained linking;
`readelf` rejects any `NEEDED` library or `INTERP` segment in both flavours. GNU
and FreeBSD `readelf` permits only explicit OS runtime libraries/loaders; macOS
`otool -L` permits only `/usr/lib` and `/System/Library`. These platforms depend on
OS services and are not advertised as fully static. No third-party runtime
library may be required. Inspection output travels with dry-run artifacts.

## Non-publishing dry run

```sh
gh workflow run release.yml --ref main
gh run list --workflow release.yml
gh run download RUN_ID --name release-dry-run --dir dry-run
```

A manual dispatch always builds/tests/packages all twelve targets but cannot sign
or publish, even when dispatched on a tag. There is no publish input. Its event
eligibility output is false, signing/publishing jobs have independent tag-push
conditions, and build/fan-in jobs have read-only permissions. This exercises the
same size selection, dependency inspection, zip contents, missing-target checks
and checksum verification as a release. It does **not** exercise OIDC issuance,
attestation upload, draft creation or publication; those require a real tag.

## One-time owner prerequisite

Before the first tag, obtain the independent implementation/release review and
all twelve passing applicable rows. A repository owner must enable
**Settings → General → Releases → Enable release immutability**, record confirmation
and recheck with their existing owner login:

```sh
gh api repos/xz-dev/procward/immutable-releases
```

Confirm feature availability too: artifact attestations support public repositories;
private/internal repositories require GitHub Enterprise Cloud and GHES is not
supported. This check needs administrative read access; normal Actions
`contents: write` does not grant it. Never add an admin PAT/App secret to Actions
for this check. Implementation/dry-run work does not change this setting.

Official references: [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases),
[owner setting](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes),
[setting API](https://docs.github.com/rest/repos/repos#check-if-immutable-releases-are-enabled-for-a-repository),
[attestation availability and permissions](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds).

## Tag, rebuild, draft, publish, verify

Publication eligibility requires a pushed exact stable `vX.Y.Z` tag equal to
Cargo.toml's package version and the checked-out/event commit. Branches, PRs,
prerelease tags, manual dispatches and release edits cannot publish. An invalid
tag fails before building. No previously uploaded branch artifact is reused.

1. Rebuild all twelve targets from the tag. Compare optimization settings, build
   dist, inspect dependencies, and rerun the full applicable fixture suite against
   **dist binaries before packaging**. Any failed/missing row blocks fan-in.
2. Require the measured/tested dist bytes to remain unchanged, then build dev
   with debuginfo and inspect its dependencies/symbols. Freeze each
   `procward-<target>.zip` and `procward-<target>-debug.zip` with maximum
   DEFLATE compression. Each contains `procward`/`procward.exe`, `LICENSE`,
   `README.md` and `third-party-licenses.md`, plus PDB/dSYM in Windows/macOS debug
   zips. Distribution documents use LF checkouts on every OS via `.gitattributes`;
   fan-in compares their exact bytes, not newline-normalized text. No executable
   is repackaged later.
3. Require all 24 zips and exact embedded binary/document bytes. Generate
   `SHA256SUMS` over the 24 downloadable zips, not itself. Changing any package or
   embedded subject after freezing fails verification.
4. Per-target signing-only jobs attest each target's zips and exact inner binaries
   (including VM-returned bytes) with [actions/attest](https://github.com/actions/attest),
   GitHub OIDC and Sigstore build provenance. A separate fan-in signer attests the
   checksum manifest only after every target signer succeeds. Signing jobs have
   `contents: read`, `id-token: write` and `attestations: write`. The older
   `actions/attest-build-provenance` is a supported wrapper, but upstream recommends
   `actions/attest`.
5. The publisher alone has `contents: write` (and `attestations: read`, no OIDC).
   It rechecks frozen bytes and all provenance with expected repository, workflow,
   tag and source digest before creating anything. It creates a **draft** for the
   existing tag with all 24 zips plus `SHA256SUMS`, then publishes the draft.
6. After publication it runs `gh release verify`, `gh release verify-asset` for
   every downloadable asset, and source-pinned `gh attestation verify` again.
   Any failure fails the workflow loudly; it does not overwrite/recreate assets.

The automatic GitHub immutable-release attestation binds the tag, commit and
release assets; it complements build provenance. Assets/tag are locked, not
release title, notes, latest or prerelease metadata. If the owner setting drifts
off, publication can occur before post-publication verification detects a
mutable release. These checks **detect**, not prevent, that already-published
state. Inspect the failure and use a new version/tag for corrections, never move
a published tag or replace immutable bytes. Partial drafts from failed attempts
require owner inspection; no automatic credential fallback or clobber is used.

References: [GitHub draft creation](https://docs.github.com/rest/releases/releases#create-a-release),
[release integrity verification](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/secure-your-dependencies/verify-release-integrity),
[`gh release verify`](https://cli.github.com/manual/gh_release_verify),
[`gh release verify-asset`](https://cli.github.com/manual/gh_release_verify-asset).

## Consumer verification

Use an up-to-date GitHub CLI with release and artifact verification support.
Obtain the expected tag commit from a trusted release/source review, not an
untrusted checksum mirror. Replace `EXPECTED_TAG_COMMIT` below.

```sh
repo=xz-dev/procward
tag=v0.1.0
target=x86_64-unknown-linux-gnu
commit=EXPECTED_TAG_COMMIT
asset="procward-$target.zip"
gh release download "$tag" --repo "$repo" --pattern "$asset" --pattern SHA256SUMS

gh release verify "$tag" --repo "$repo"
for file in "$asset" SHA256SUMS; do
  gh release verify-asset "$tag" "$file" --repo "$repo"
  gh attestation verify "$file" --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release.yml" \
    --source-ref "refs/tags/$tag" --source-digest "$commit"
done
grep "  $asset$" SHA256SUMS | sha256sum --check -
unzip "$asset" -d procward-download
gh attestation verify procward-download/procward --repo "$repo" \
  --signer-workflow "$repo/.github/workflows/release.yml" \
  --source-ref "refs/tags/$tag" --source-digest "$commit"
```

For Windows, verify `procward-download/procward.exe`; for debug, download
`procward-<target>-debug.zip` (inner name stays `procward`/`procward.exe`). On macOS,
use `shasum -a 256` and compare to the selected manifest line. On PowerShell use
`Get-FileHash -Algorithm SHA256` and compare to that line, then `Expand-Archive`.
Checksums alone are not authenticated provenance. The signed manifest authenticates
all listed zip digests, while source-pinned attestations authenticate the package
and binary actually received. See [`gh attestation verify`](https://cli.github.com/manual/gh_attestation_verify)
for additional policy controls.
