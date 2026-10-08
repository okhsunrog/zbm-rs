# OS-owned TPM setup and loader PCR15 measurement — 2026-10-08

The loader now performs only a read-only TPM2/PCR15 readiness probe and records
verified prepared targets in PCR15. SRK/NvPCR setup and PCR11 OS phases belong to
the selected OS. The separate systemd v262 setup derivation, NvPCR configuration,
loader PCR policy signing and target NvPCR masks have been removed.

All source checks pass: 36 workspace tests, formatting, warnings-denied
all-targets/all-features Clippy, six owner-tool/PCR replay boundary tests, Cargo
config input tracking and Nix default/bounds/legacy/optional/required parity.
The protected loader and authorized NixOS fixture reuse the tested configured
Linux 6.18.55/ZFS pair; no host kernel, TPM, firmware keys or boot files were changed.

| Run under `target/vm/` | Result |
| --- | --- |
| `tpm-os-owned-verified-002` | OVMF/TCG Secure Boot; complete signed input/rejection, protected recovery and actual second-kernel handoff; 12 firmware PCR11 events replay, zero loader PCR11 events; exact prepared-plan PCR15 replay |
| `tpm-os-owned-required-absent-001` | Required TPM absent: no manager, protected recovery and clean poweroff |
| `tpm-os-owned-optional-absent-001` | Optional TPM absent: unavailable evidence, every boot-input rejection/recovery check and actual verified handoff still pass |
| `tpm-os-owned-nixos-001` | OVMF/KVM Secure Boot; actual broker/menu ZFS-root boot, unauthorized arguments rejected; native unmasked target TPM setup succeeds |
| `tpm-os-owned-snapshot-001` | OVMF/KVM Secure Boot; authorized writable clone boot, original dataset/snapshot preserved; native unmasked target TPM setup succeeds |
| `tpm-os-owned-off-001` | Ordinary OVMF/TCG ZFS/input/encryption smoke and full PID1 lifecycle regression pass |

Read-only guest queries explicitly use `/dev/tpmrm0`. NV and persistent handle
lists are empty at loader startup and after protected recovery; no setup helper,
NvPCR definition or initialization metadata is present. PCR11 remains unchanged
during input verification and manager crash/restart. The independent replay test
rejects injected loader phases and changed prepared-target records.

Both installed-OS journals show a **new SRK created by the target initramfs**, then
reused and saved publicly by target userspace. The target's native systemd v261
initializes four NvPCRs in its late setup; no loader-owned index/policy conflicts
occur and no NvPCR masking is needed. This is native v261 compatibility evidence,
not acceptance of the newer signed-policy v262 API or every NvPCR consumer.
All QEMU/swtpm children were reaped. The initial `tpm-os-owned-verified-001` failure
is preserved: a test tool was packaged as a Nix shell wrapper rather than its ELF;
packaging now stages the underlying ELF with the correct multicall name.

Still pending: owner-managed target v262 signed NvPCR policy integration,
PCR15 event-log transport across kexec, generic-Linux and encrypted-root acceptance,
and physical Framework Secure Boot/recovery. Prepared-target measurements are
not proof of OS execution, remote attestation or rollback resistance.

# Earlier broker-authorized NixOS and trusted snapshot boot — 2026-10-08

The persistent `boot-smoke` harness now supports enrolled OVMF variables and an
owned disposable swtpm, using the same lifetime helper as `secure-smoke`.
It provisions a real ZFS root, navigates the actual TUI, and invokes the root
broker rather than the synthetic executor probe. Owner publishing signs 7297
target-root modules, the preboot modules, kernel and final IMA initramfs outside
Nix. Two generations share image bytes but have distinct signed argument indexes.

Passed under OVMF/KVM Secure Boot:

- `verified-nixos-002`: rejects a valid-looking generation with unauthorized
  arguments and leaves the kexec slot empty; manager restart and foreign-mount
  protection pass; the explicitly selected older generation boots its exact
  NixOS toplevel on `zbm_fixture/nixos` and reaches multi-user with a new boot ID.
- `verified-nixos-snapshot-001`: read-only snapshot discovery, authorized clone
  preparation, discard cancellation/foreign-child refusal, foreign-owner refusal,
  abort/restart reconciliation and actual writable clone boot pass. The original
  dataset and source snapshot remain unchanged.
- The selection and prepared-clone PNGs from both runs were inspected.
- `verified-arguments-v2-001`: the complete signed synthetic input/rejection,
  privilege/recovery, TPM/PCR replay and real handoff scenario passes under TCG
  using the new argument-index layout.
- `verified-root-off-regression-001`: ordinary `off` OVMF/TCG smoke and full
  PID-1 lifecycle pass. Nix default/bounds/legacy configuration parity passes.
- `nixos-row-off-regression-001`: ordinary unsigned-loader NixOS boot with the
  updated fixture proof and row-identity selection still passes.
- 36 workspace tests, formatting, warnings-denied all-targets/all-features Clippy
  and five owner-tool boundary tests pass.

The initial `verified-nixos-001` refused the unauthorized target correctly but
exposed the harness's fixed Down-key count skipping over an inspectable rejected
generation. Selection now waits for the real UI row identity and confirms the
domain generation. Failed artifacts remain preserved.

`verified-nixos-diag-001` captured a distinct target TPM failure: ordinary SRK
load/persistence succeed, then target systemd v261 attempts an incompatible
anchor-secret initialization over the loader v262 hardware NvPCR and allocates
three unwanted default NvPCRs. The ordinary-target TPM fixture at that time masked those
definitions, retained SRK services and required an explicit target SRK marker
before boot success. `verified-nixos-srk-001` and
`verified-nixos-snapshot-srk-001` pass that final gate: early and late target setup
reuse the resident SRK, persist its public part, and report no NvPCR conflict or
extra allocation. The required TPM driver is in the signed target initramfs.
Target NvPCR consumers/log transfer remain separate gates;
root boot acceptance is not proof of that integration or physical Framework boot.

# Earlier signed OVMF mechanism acceptance — 2026-10-08

`target/vm/verified-004/security-report.json` records a passing signed OVMF/TCG
scenario with the separately configured Linux 6.18.55 loader and matching ZFS.
The owner publisher signs IMA policy, independently verifies staged module
signatures and signs the final UKI outside Nix. Disposable fixture keys never
enter Git, Nix derivations or the guest. The default image remains `off`.

Passed: actual firmware Secure Boot, forced module/kexec signatures and lockdown,
restricted IMA certificate/policy loading, pinned CMS authorization, sealed
read-only memfd appraisal, unsigned-kernel/bad-IMA/changed-CMS/changed-content/
extra-argument rejection and legacy-kexec refusal. The UI has all IDs 65534,
zero effective/permitted/inheritable/ambient capabilities and `NO_NEW_PRIVS`.
Shell refusal, manager crash/restart, bounded protected recovery and retry pass.
The signed synthetic target actually boots with a new boot ID, final expected
arguments and `SecureBoot=1`, then powers off cleanly.

This tests the loader and native synthetic executor inputs, not the real broker
catalog, installed Linux/NixOS roots, snapshot clones or a physical Framework.
TPM capability setup, owner PCR signing and a reusable swtpm path passed this
initial matrix; NV exhaustion, stale indices and interrupted setup remain pending.
No TPM disk unlock, remote
attestation or across-kexec event-log transport is claimed. Reproduction and
remaining gates are in [the signing/test workflow](security-testing.md).

Source checks pass: 35 workspace tests, formatting, all-targets/all-features
warnings-denied Clippy and three build-tool boundary tests. The earlier unsigned
regression `security-foundation-off-001` passed real ZFS and full lifecycle.

## Earlier loader-owned NvPCR experiment (superseded)

These historical runs exercised loader-owned SRK/NvPCR setup and PCR11 phases.
That design has been removed: the selected OS now owns setup and its phases.
The old bad/missing NvPCR policy images are not current loader acceptance gates.
All runs below used disposable OVMF trust and fresh swtpm state when present.
`systemd-tpm2-setup`, `systemd-pcrextend` and owner `systemd-measure` come from a
separate v262 provider at that time; the host systemd and host TPM were not used.

| Run under `target/vm/` | Result |
| --- | --- |
| `verified-tpm-002` | OVMF/TCG Secure Boot; SRK, signed-policy hardware NvPCR, product identity, PCR15 prepared plan; complete verification/recovery/handoff scenario |
| `verified-tpm-replay-002` | OVMF/KVM; same scenario plus independent firmware/userspace replay matching actual SHA-256 PCR11 and PCR15; 14 firmware PCR11 events and one enter-initrd |
| `verified-tpm-missing-002` | Required hardware NvPCR with absent PCR signature: no manager, protected recovery, clean poweroff; SRK/ordinary phase still succeed |
| `verified-tpm-invalid-002` | Required hardware NvPCR with invalid signed policy: same refusal; EFI signing and measured section bytes remain valid |
| `verified-tpm-absent-002` | Required TPM without a TPM device: no manager, protected recovery, clean poweroff |
| `verified-tpm-optional-absent-001` | Optional TPM without a device: unavailable evidence; all boot-input rejection/recovery checks and actual verified handoff still pass |
| `verified-tpm-optional-invalid-001` | Optional NvPCR with invalid policy: degraded evidence; kernel/initramfs/CMS/argument checks stay mandatory and actual handoff passes |
| `verified-tpm-final-001` | Final source OVMF/TCG image: full verified handoff, TPM/PCR replay and protected recovery; updated menu/recovery PNGs inspected |

The final ordinary `off` image also passes fresh OVMF/TCG regression in
`security-off-final-002`, including real ZFS, visibility, local-key encryption,
input/shell/poweroff and the complete PID-1 fault/lifecycle suite. Nix module
default/bounds/legacy-option JSON parity still passes.

Initial `verified-tpm-001` exposed `ukify --join-pcrsig` not adding an absent
section. The publisher at that time built and verified that section explicitly before
EFI signing; the current publisher no longer embeds NvPCR policies. Initial KVM negative/replay runs exposed the harness reaching SSH
before the test NIC's first link-up; transport readiness is now awaited separately.
All failed run artifacts remain available. Firmware/target proofs and PCR replay
are independent observations; the prepared PCR15 event is not proof of execution.

# Earlier Secure Boot implementation foundation — 2026-10-07

Local source checks pass: formatting, 35 workspace tests with all features,
warnings-denied all-targets/all-features Clippy and whitespace checks. Coverage
includes pinned CMS acceptance/rejection, changed signed payload, foreign signer,
trailing DER, ambiguous JSON, typed-root/argument/source binding, missing owned
snapshot-clone evidence, immutable sealed bytes, malformed broker frames and
existing-kernel IMA keyring lookup. These tests do not invoke privileged kexec or
prove IMA appraisal under the configured loader kernel.

The first updated unsigned portable test image builds and passes the size policy:
EFI 26,373,120 bytes, initramfs 12,356,310 bytes, kernel 13,840,896 bytes. A rebuild
with the final source changes succeeds (EFI 26,374,656 bytes). A fresh ordinary
OVMF/TCG regression passes in `target/vm/security-foundation-off-001`: real ZFS,
discovery/visibility, local-key native encryption, keyboard/rendering, shell return,
poweroff and the complete PID-1 lifecycle/fault scenario. `report.json` and
`lifecycle.json` both record success. The actual help/compact PNGs were inspected.
The separate enforced-kernel configuration evaluates using a generated disposable
public fixture certificate; its effective configuration has forced kexec/module
signatures, early integrity lockdown, signed IMA policy, built-in efivarfs and the
`ima` LSM. Its full kernel build is running. No production key or host boot setting
is used. The new protected broker, enforced IMA/memfd handoff and TPM profiles have
no signed OVMF acceptance yet. TPM profile execution is unimplemented and refuses
startup when requested. No physical zbm-rs Secure Boot result is claimed.

# Linux/ZBM compatibility and snapshot operations — 2026-10-07

Ordinary Linux kernel/initramfs pairs are now boot targets alongside NixOS
Bootspec generations. Snapshot rollback and persistent clone/promotion operate
on the selected ZFS snapshot independently of either backend. NixOS snapshot
boot recognizes the actual systemd initrd; no custom capability flag is needed.
The old snapshotBoot Nix option is retained as a deprecated compatibility no-op.

Passed local VM acceptance, with fresh disposable disks and run folders:

| Run under `target/vm/` | Verified behavior |
| --- | --- |
| `arch-live-002` | Actual Arch userspace through kexec; original ZFS root and Linux LTS 6.18.53-1-lts |
| `arch-snapshot-002` | Writable temporary snapshot clone; original root and snapshot unchanged |
| `arch-clone-001` | Persistent ordinary BE clone appears in the list and boots |
| `arch-promote-002` | Clone+promote selects and boots the new BE, with no remaining origin |
| `arch-rollback-003` | Dependent-clone and read-only refusals; manager restart; exact typed confirmation; rollback restores data, removes newer snapshot and boots |
| `nixos-generic-001` | Selected non-default NixOS generation reaches multi-user |
| `nixos-auto-snapshot-002` | NixOS snapshot clone boot without custom Bootspec metadata |
| `generic-tcg-002` | Final-image OVMF/TCG smoke, supervisor lifecycle, failure isolation and interface scenarios |

Arch acceptance uses the real sanitized `archinstall_zfs` mkarchiso staging tree
from image 2026.09.23, builds its normal ZFS-root mkinitcpio in the VM and deploys
it on disposable storage. It is not a full installer-wizard run. The fixture
archive hash is `34cb3ac0d52669f953bf8404e7190407990e15469a930013ac85489e192f1160`.
No host disks, cache, keys or hostid are attached; the source repository/staging
tree remains unchanged. Proof services check the exact kernel, root dataset,
restored data, writable clone and preserved source before guest poweroff.
Linux tests exercise parent commandline expansion, per-kernel `.kcl`, and removal
of conflicting root arguments. Reports and screenshots remain in each run folder.

Intermediate failures are preserved: `arch-live-001` reached Arch userspace but
the proof collector missed its final serial marker; `arch-snapshot-001` exposed
mkinitcpio's inability to mount a legacy temporary clone without a matching fstab
entry; `arch-promote-001` exposed stale UI history after promotion. Fresh passing
runs validate the collector fix, native Linux clone mountpoint and history reset.
`arch-rollback-002` correctly rejected a dependent clone but exposed a lost
restart-required error state; that state assignment has been corrected and the
fresh `arch-rollback-003` run passed both refusals, recovery and actual Arch boot.
Bare Enter, modified Enter and Escape cannot authorize the rollback. Both final
VM runs exited successfully; all owned VM processes were reaped.

Rust fmt, 27 workspace tests with all features, and all-targets/all-features
Clippy pass. Production, regular-test, snapshot-test and NixOS module-config Nix
outputs build. Latest production/snapshot-test EFI sizes are 24,730,112 and
26,300,928 bytes; image size policy passes.

Actual PNGs inspected include `arch-snapshot-002/arch-kernel.png`,
`arch-promote-002/persistent-clone.png` and `generic-tcg-001/ui-help.png`.
The final `arch-rollback-003/rollback-confirmation.png` and
`rollback-complete.png` were also inspected: the exact confirmation and updated
BE list are visible. The Linux kernel list, clone selection, preview and complete action inventory
are visible. This is VM acceptance, not physical boot or Secure Boot validation.
Encrypted-BE unlock/keysource, send/receive duplication, specialisations and
other missing ZBM functions remain roadmap work; full ZBM parity is not claimed.

# Historical Arch compatibility probe — 2026-10-06

At the time of this probe, Arch boot was unsupported. This limitation is superseded
by the Linux backend verified above. A successful compatibility probe means that
this limitation was reproduced and diagnosed, not that an installed Arch OS
reached userspace.

`target/vm/arch-probe-003` used the real `archinstall_zfs` mkarchiso staging tree
from its 2026.09.23 Arch image, with Linux LTS 6.18.53-1-lts and its ZFS module.
The root was deployed onto a fresh disposable ZFS dataset, following the normal
ZFS-root mkinitcpio path of `gen_iso/deploy-zfs-be.sh`. This was a staged BE test,
not a complete installer-wizard run. The archive excluded identity files, keys,
caches and home directories. The guest hostid was synthetic; no host storage or
host cache was attached. The source repository and staging tree were unchanged.

The guest successfully built `vmlinuz-linux-lts` / `initramfs-linux-lts.img`;
`lsinitcpio` confirmed the ZFS runtime hook, ZFS module and synthetic hostid.
The manager discovers the BE, marks pool bootfs and retains `rootprefix=zfs=`
and the commandline property. However, it reports no supported NixOS generations
because generic Linux kernel/initramfs discovery is unimplemented. The real
snapshot is visible, but opening it similarly finds no NixOS generations.
Clone preparation remains unavailable, no clone is created and no kernel is
loaded. The manager survives and guest poweroff completes. The report explicitly
sets `arch_boot_supported=false`, `snapshot_boot_supported=false` and
`arch_boot_attempted=false`.

Evidence: `arch-report.json`, `arch-inputs.txt`, `arch-initramfs-build.log`,
`arch-safety.txt` and real inspected PNGs `arch-environment.png` /
`arch-snapshot-unavailable.png` in that run folder. The first actual VM run,
`arch-probe-002`, is preserved: the fixture built and BE discovery passed, but
the harness incorrectly checked `snapshot.name` rather than `snapshot.source.name`.
The corrected fresh run passed. Rust fmt, 22 workspace tests and all-targets
Clippy pass. No loader behavior was changed to make this negative test pass.

Reproduce with the reviewed staging kernel (the builder currently checks that
exact kernel version):

```sh
uv run --no-project python xtask/fixtures/build_arch.py \
  ~/code/archinstall_zfs/gen_iso/workdir/x86_64/airootfs \
  target/arch-fixture-new --sudo
# Historical command, replaced by arch-smoke:
cargo xtask arch-probe --image result-snapshot \
  --fixture target/arch-fixture-new --run target/vm/arch-probe-new
```

`--sudo` gives tar read access to restricted staging files; it never modifies
the source. Build `result-snapshot` with `nix build .#zbm-rs-efi-test-snapshot
--out-link result-snapshot` first. No physical boot or Arch kexec acceptance is
claimed.

# Two-pane interface verification — 2026-10-06

The manager now shows a candidate list beside a read-only target preview on wide
consoles. At 80x25 it keeps the list and recovery controls visible and offers full
details in a scrollable panel. Search, keyboard help and the complete action menu
share the existing boot/recovery dispatcher. Unavailable generations remain
inspectable but cannot boot. Discarding an owned snapshot clone requires an
explicit confirmation; Escape cancels it without changing the clone.

Fresh runs on the final images passed:

- `target/vm/ui-final-tcg-002`: OVMF/TCG smoke, full supervisor lifecycle and
  failure isolation/deadline scenarios. `interface-report.json` records help,
  actions, search isolation, 160x50 and 80x25 layouts, compact details, rejected
  generation inspection/boot blocking and Bootspec previews. Opening and closing
  help during search preserves search focus; typed command letters stay text.
- `target/vm/ui-nixos-002`: a selected non-default NixOS generation reaches
  multi-user through `kexec_file_load`; restart reconciliation and foreign-mount
  protection still pass.
- `target/vm/ui-snapshot-002`: snapshot browsing, clone preparation/reuse,
  ownership protection, non-recursive discard and clone boot pass. Full details
  show the actual prepared clone and overridden command line. Cancelling the
  discard confirmation preserves the clone. The source dataset and snapshot
  remain unchanged.
- `target/vm/ui-scsi-002`: host-only SCSI discovery, native local-key encryption,
  read-only candidate mounts, keyboard/compact-layout checks, shell return and
  guest poweroff pass.
- Rust fmt, 22 workspace tests with default and all features, and Clippy with
  default and all features pass. Nix production, test, snapshot, host-only SCSI
  and NixOS module-config outputs build successfully. Production/test EFI sizes
  are 24,572,416 / 26,145,280 bytes; the image size policy passes.

Actual final PNGs inspected: `ui-nixos-002/generations.png`,
`ui-snapshot-002/ui-snapshot-details.png` and `ui-discard-confirmation.png`, plus
`ui-final-tcg-002/ui-help.png` and `ui-compact.png`, all under `target/vm/`.
The compact VT occupies the upper-left 640x400 region of the unchanged 1280x800
framebuffer. The candidate list, diagnostics, borders and footer controls are
present. All scenario-owned VM processes exited and were reaped.

These are local source/image/VM checks. Physical boot, Secure Boot and remote CI
execution were not tested. Existing backend limits, including encrypted-BE and
generic Linux boot support, are still displayed as unavailable.

# Review fixes and boot regressions — 2026-10-06

The hardware probe now includes block frontend and parent transport modules;
synthetic sysfs fixtures cover AHCI, USB storage and a built-in SCSI frontend.
The image exposes its copied udev rules through `/etc/udev/rules.d` so eudev can
find them outside its original Nix-store vendor prefix. Coldplug now loads the
SCSI transport and disk frontend without adding manual modprobe calls to setup.

Individual BE mount failures and malformed/unsupported generation Bootspecs
produce local diagnostics while usable peers remain selectable. Snapshot
generation browsing uses the same isolation. Boot operations run outside input
handling, with a 30-second deadline and recovery keys available while pending.
Timeout cancellation requires a new manager before more boot actions; ownership
intent remains available for reconciliation.

Passed with fresh run folders and the corrected Nix images:

- `fixes-failures-tcg-002`: full OVMF/TCG lifecycle plus deliberate BE mount
  failure, valid/broken generation mix, visible generation diagnostic, Shell
  during hung mount, cancelled-child reaping, real operation timeout, Restart
  reconciliation and Power off during a pending mount. See `report.json`,
  `failure-report.json` and `lifecycle.json` in `target/vm/` under that run.
- `fixes-scsi-002` (KVM) and `fixes-scsi-tcg-003` (TCG): explicit host-only SCSI
  image, automatic disk discovery, disposable ZFS operations, native local-key
  encryption, read-only BE mounts, Shell return and guest poweroff. The fixture
  disk is identified by its unique serial rather than an assumed `/dev/sdX`.
- `fixes-nixos-002`: non-default NixOS generation 1 reaches multi-user through
  `kexec_file_load` despite a malformed neighbouring generation 3. Existing
  restart reconciliation and foreign-mount protection checks also pass.
- `fixes-snapshot-002`: snapshot generation 1 boots on its writable owned clone,
  with malformed generation 3 rejected; original dataset and source snapshot
  remain unchanged. Clone ownership/discard/restart checks also pass.
- Rust fmt, 18 workspace tests with default and all features, Clippy with default
  and all features, config-input checks and wrong-libc runtime rejection.
- Nix production, test, snapshot and host-only SCSI builds; hardware-probe and
  NixOS module-config checks. Production/test EFI: 24,548,352 / 26,118,144 bytes;
  the image size policy passes.

The earlier `fixes-scsi-001` failure is preserved: the image contained the modules
but no SCSI disk appeared because eudev was reading zero rules. A disposable
diagnostic VM confirmed that exposing the rules via `/etc` enabled coldplug;
the fresh SCSI runs above validate the packaged correction.

Real PNGs were rechecked for BE failure isolation, mixed generations, a pending
operation and a prepared snapshot clone. The inspected files contain the list
and Status borders and the complete footer controls. Direct checks of the PNG
pixels confirm these regions are present. The earlier claim that these captures
omitted borders or controls was incorrect and is withdrawn; these artifacts do
not establish a graphics discrepancy. Reports establish functional VM acceptance;
physical boot and Secure Boot were not tested. CI scenarios were updated but no
remote CI execution is claimed.

## Previous lean userspace verification

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

## 2026-10-06: NixOS snapshot clone boot

The persistent `boot-smoke --snapshot` scenario now provisions a source with
live generation 2 and snapshot generation 1, prepares a clone, tests read-only
import and org.zfsbootmenu:readonly rejection, protects foreign children during
non-recursive discard, rejects a changed ownership token, aborts the manager,
and verifies reuse of exactly one prepared clone after restart.

QEMU/KVM acceptance passed in `target/vm/snapshot-004/snapshot-report.json`.
The selected snapshot generation reached multi-user.target on the clone with a
new kernel boot ID. The target wrote to its clone and independently mounted the
original dataset and source snapshot read-only to verify their data remained
unchanged, then powered off. Screenshots and serial evidence are in that run.
Ordinary image smoke (`snapshot-normal-001`) and exact-generation boot
(`snapshot-normal-boot-001`) also passed with the new core code.

The tested image is `target/snapshot-corrected-image`; its normal-policy companion
is `target/snapshot-corrected-image-1`. Fixture:
`target/snapshot-fixture-diagnostic`. The fixture now captures sysroot mount
failure diagnostics automatically. A legacy clone must not use `zfsutil` in
its root mount options; the initial failed boot exposed that helper constraint.

One successful-boot run (`snapshot-003`) exceeded the former 30-second poweroff
wait. Its cause remains unconfirmed; the bounded wait is now 120 seconds and the
subsequent full run completed without target mount or shutdown errors. Failed
runs are preserved. Rust tests, formatting and Clippy passed; Nix module-config
evaluation also passed. These are VM checks, not physical/Secure Boot acceptance.

## Planned Secure Boot acceptance

This is a future acceptance plan for [the accepted design](secure-boot-model.md),
not an executed report. Current unsigned boot/lifecycle tests remain useful but
cannot establish the signed chain, IMA appraisal or protected recovery.

Use fresh disposable OVMF variable stores with enrolled **fixture** keys and actual
Secure Boot enforcement. Build through the canonical image pipeline, then sign
output copies in an isolated fixture deployment step. Never use host disks, host
firmware variables, host signing/encryption keys or host configuration. Fixture
private keys must stay outside production image outputs; production has no test
SSH or fault hooks. Preserve artifact digests, public certificate fingerprints,
firmware state, startup enforcement evidence, serial/UI logs and target boot IDs.

| Boundary | Required scenarios / outcome |
| --- | --- |
| Firmware -> loader UKI | Trusted signed UKI boots; unsigned, changed and untrusted/revoked loader signatures are refused by firmware, not merely by the loader UI. |
| Image configuration / startup | Missing/invalid policy, missing trust roots, inactive required enforcement and insecure loader arguments refuse handoff without falling back to development defaults. |
| Firmware-state policy | `enforce` with optional firmware requirement continues target checks when Secure Boot is disabled; requiring firmware refuses disabled/unknown state. Unknown state never downgrades. |
| BootAuthorization | Valid Linux/NixOS targets boot; bad signature, unknown authority, out-of-scope authority, malformed/duplicate fields, wrong architecture/version and altered digests fail. |
| Target kernel | Unsigned, altered or untrusted signed kernel fails in `kexec_file_load`, including when a fixture authorization signs its digest. Accepted kernel identity matches the selected target handshake. |
| Target initramfs | Altered, unsigned, missing required or incorrectly signed initramfs fails appraisal. Independently exercise broker digest rejection and kernel IMA rejection with an otherwise authorized digest. |
| Arguments | Approved exact tokens and permitted root slot succeed; extra/repeated/conflicting arguments, changed `init=`, unauthorized root and unapproved `kernelArgs` fail. |
| Immutable input preparation | Symlink/path swaps and source/prepared-inode mutation races cannot change the bytes consumed after verification. Cover detached IMA signatures on private staged copies and snapshots without xattrs. |
| Snapshots / clones | Authorized snapshot/owned-clone boot reports the correct root and boot ID; unsigned historical targets, mismatched origins, ownership reuse and unauthorized selection fail without changing read-only snapshots. |
| Module trust / bypasses | Unsigned ZFS/other preboot modules fail; trusted modules load. Direct legacy kexec and unauthorized manager/broker requests cannot bypass policy; a signed kernel alone cannot authorize arbitrary initramfs/arguments. |
| Failure / recovery | Manager panic/abort, PID-1 failure/last-ditch recovery, failed appraisal and returned kexec before/after unlock offer no unrestricted shell. Diagnostics/restart/reboot/poweroff remain usable. |
| Lifecycle / cleanup | Cancelled or failed preparation unloads the owned kernel and cleans owned mounts/imports/clones only; IPC closure, overlapping requests and manager restart do not retain stale authorization. |
| Deployment / rotation | Deployed bytes match signed outputs. Exercise certificate overlap, unknown/revoked authorities and a trusted recovery target without assuming a new loader revokes older signed loader images. |
| UI evidence | Firmware state, policy, authorization, kernel acceptance and initramfs appraisal are distinct. No claim that the selected root's contents are attested. |

Positive boot acceptance must reach the selected OS, not stop at a loader “PASS”
message. Exercise both Linux and NixOS targets and authorized snapshot/clone root
variation through real kexec. Negative tests must identify which independent
layer rejected the input; broker rejection alone does not prove kernel appraisal.

Physical acceptance is a separate gate on the intended hardware, firmware trust
configuration and deployed image: verify enrolled keys, trusted boot and recovery,
loader ZFS/module operation, target module/DKMS trust and authorized snapshot boot.
Record precisely what was observed after reboot. VM acceptance does not establish
Framework firmware behavior or recovery accessibility on a physical console.
Target-wide IMA, TPM unlock/attestation and strong antirollback remain outside this
initial gate. The deferred untrusted-boot UI action has no current acceptance claim.
