# Licensing

Procward implementation and fixture code is newly authored under
**BSD-3-Clause**. [LICENSE](../LICENSE) contains the project licence and copyright
notice. Rust sources and the Cargo manifest carry the matching SPDX identifier.

No pi-env implementation, tests, fixtures or ABI shims were copied or translated.
Its behaviour informed the planning scenarios only. No inherited pi-env MIT
notice or relicensing permission applies to this new implementation.

## Dependencies are not BSD-licensed procward code

The following inventory comes from the checked `Cargo.lock` and published crate
archives. Where a crate offers a choice, this Linux slice uses its MIT terms.
MIT requires retention of the upstream copyright and permission notices in
copies or substantial portions. Shipping a binary does not remove those duties.

[`third-party-licenses.md`](third-party-licenses.md) preserves the selected
upstream licence texts and additional attribution files verbatim. Include it
alongside the project LICENSE in source and binary distributions. It also
retains process-wrap's Apache-2.0 text: its COPYRIGHT identifies Apache-only
Windows routines. Those routines are not enabled in this Linux slice; recheck
that exception before adding the Windows backend, including Apache licence,
attribution, change-notice and any applicable NOTICE obligations. Do not describe
all dependency source as MIT-only.

| Crate | Locked version | Published SPDX expression |
| --- | --- | --- |
| process-wrap | 10.0.1 | Apache-2.0 OR MIT (see per-file exception above) |
| rustix | 1.1.6 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| signal-hook | 0.4.5 | MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| libc | 0.2.191 | MIT OR Apache-2.0 |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| nix | 0.31.3 | MIT |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |

This includes build-time `cfg_aliases` (and its tectonic attribution) and
Windows-target transitive crates recorded by Cargo, not a claim that every
listed crate is linked into the Linux binary. No vendored implementation is
included. Dependency source remains in Cargo's registry cache.

Recheck Cargo's resolved target/features and the upstream archives whenever the
lockfile or backend changes. Refresh the inventory and notice bundle together;
this snapshot is not a substitute for the later release-package licence check.
