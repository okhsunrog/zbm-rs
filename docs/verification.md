# Lean userspace default — 2026-10-06

The normal production/test outputs and the NixOS module now use eudev and ZFS
without URL fetching. The previous lean names are aliases. Named full-userspace
outputs retain systemdMinimal and URL fetching for explicit use. Native encryption
with local keys remains supported; HTTPS keylocations are unavailable by default.

Production/test EFI: 24,410,112 / 25,980,928 bytes. Regression baselines were lowered
to these measured sizes; existing allowances/caps were retained.

Passed: normal Nix production/test/size-check builds, fmt, workspace tests,
all-targets/all-features clippy, and a fresh full TCG/OVMF lifecycle in
`target/vm/lean-default-001`, including local-key native encryption and poweroff.
NixOS evaluation confirms eudev, preservation of boot.zfs.package version 2.4.4,
and a matched 6.18.55 kernel/ZFS module pair. GitHub CI for the previous optimized
commit 099f7dd is also green (run 37401606613).

## Prior size optimization verification

# Published repository and size optimization verification — 2026-10-06

Public repository: https://github.com/okhsunrog/zbm-rs. README documents the project
origin, goals and the boundary between the working foundation and roadmap work.
The initial GitHub Actions run 37400311280 passed all Rust/Nix/TCG lifecycle checks.

Rust release defaults are opt-level z, FatLTO and one codegen unit; panic unwinding
is retained. Nix defaults to zstd-19 and standard systemdMinimal/ZFS userspace.
Production EFI is 28,632,576 bytes (27.31 MiB), down from 31,821,312 bytes.
See size-experiments.md for the complete controlled profile/compression matrices.

Passed locally on the new images:

- fmt, 11 focused workspace tests, all-targets/all-features clippy and Cargo config input checks.
- size-standard-fixed-001: standard userspace + zstd + optimized Rust, full UEFI lifecycle.
- size-static-udev-fixed-001: standalone-linked modern udev, full UEFI lifecycle.
- size-lean-fixed-001: eudev + ZFS without URL fetch, full UEFI lifecycle.
- size-xz-001: XZ initramfs, ordinary UEFI discovery/encryption/shell/poweroff smoke.
- Scenarios now exercise native encryption with a disposable local passphrase key.
- Complete-record serial telemetry fixes were exercised in the successful reruns.

The lean profiles preserve local-key native encryption but cannot fetch HTTPS keylocations.
They are opt-in; the default retains URL fetching. Standalone-linked udev is also an
experiment, with only a small compressed-size win and larger individual helper ELFs.
No physical boot or successful installed-OS kexec is claimed.

## Previous foundation verification

# Current Nix / PID-1 / immutable JSON verification — 2026-10-06

The canonical Nix production and test EFI builds pass, with kernel 6.18.55 and
matching OpenZFS 2.4.4 modules/userspace. The production size check passes.
Production EFI: 31,821,312 bytes; test EFI: 33,806,848 bytes.

Passed:

- cargo fmt, workspace tests (default and all features), and all-targets/all-features clippy.
- 11 focused tests, including typed/default/alternate JSON and semantic errors.
- uv run --no-project nix/check-config.py: build-host validation, changed file/env
  invalidation, defaults, malformed enum, and absence of generated Rust config.
- NixOS option evaluation produces the expected JSON attrset (timeout 7,
  restart threshold 3, read-only policy, custom title).
- AArch64 check and release cross-build with aarch64-linux-gnu-gcc; no AArch64 boot.
- cargo xtask smoke --run target/vm/immutable-json-uefi-001 --lifecycle (KVM + OVMF).
  Real ZFS snapshot/clone/rename/promote/export/discovery, input, shell and poweroff;
  private IPC CLOEXEC, orphan reaping, dirty-console abort, controlled restart,
  leaked descriptor, signal forwarding, panic, fatal SIGSEGV, crash-loop recovery
  and failed-kexec classification. PID 1 survives all manager failures.
- cargo xtask smoke --run target/vm/immutable-json-tcg-002 --port 2230 --tcg --lifecycle
  passes the same complete scenario with software CPU emulation (CI mode).
- Pool and shell-return screenshots from the Nix image were visually checked and
  show the complete TUI, configured title, pool, status and controls.

The first synthetic kill -SEGV is consumed by Rust std's stack-overflow handler.
The test-only fatal-signal hook resets that handler before raising SIGSEGV. The
supervisor log confirms signal=Some(11). Production does not include this hook.

The runtime dependency package set is unchanged. Host release ELF grew from
2,918,400 to 3,023,960 bytes (+105,560 bytes, 3.62%) for typed JSON loading/validation.
There is no const-gen/databake/custom codegen in Cargo.lock. Serde and serde_json
are intentionally runtime dependencies and also build-only validator dependencies.

Full Linux OS boot/kexec, BE/NixOS discovery, imports, snapshot boot, encryption
unlock and Secure Boot are future work. VM results do not establish physical boot.
The GitHub workflow is prepared; no remote CI execution is claimed.

## Historical host-derived prototype (retired image builder)

# Foundation verification — 2026-10-06

Local source: zfskit 6389c1be98b7721eec9858cc4916cb866d09d5c2.
Rust 1.98.1, kernel 7.2.8-1-okhsunrog, guest ZFS 2.4.4-1.
Reference repositories were inspected and remain clean and unchanged.

Passed locally:

- zfskit locked unit/CLI-fixture/doc tests (real-host integration feature not run).
- cargo fmt --all --check.
- cargo test --workspace --locked (5 focused tests).
- cargo clippy --workspace --all-targets --locked -- -D warnings.
- shellcheck boot/build-image.sh boot/init.
- Unsigned EFI image construction without root privileges.
- cargo xtask smoke --run target/vm/foundation-final (KVM, OVMF EFI).
- cargo xtask smoke --run target/vm/foundation-direct --direct (KVM, Linux directly).
- Ctrl-C interruption in target/vm/cancel-001: owned QEMU terminated/reaped;
  diagnostics retained, no passing report emitted.
- Host --boot invocation rejected outside a zbm-rs initramfs.
- No zbm-rs QEMU process remained after verification.

Both final scenarios exercise real ZFS modules and a disposable guest disk,
key-only SSH, snapshot/clone/rename/promote, non-mutating discovery through
zfskit, QMP keyboard rescan and recovery shell round trip, real VT text assertions,
PNG capture and guest poweroff. Reports, logs and disks remain in their run dirs.

Visual inspection covered empty, discovered-pool and shell-return states at
1280x800 (160x50 console cells). Known limitation: PNGs immediately after Rescan
can omit unchanged lower controls while /dev/vcs1 text has them. VT switching
and a different virtual GPU did not reliably resolve it. The graphics-path cause
is not isolated. The functional smoke report does not certify every PNG as a
complete rendering of the VT. Use screen --text alongside screenshots.

Linux OS selection/kexec, BE discovery, imports, NixOS, snapshot boot, encryption
unlock and Secure Boot remain unimplemented. No physical boot was tested.
The GitHub workflow is a prepared Rust-check scaffold, not a claimed remote CI run.
