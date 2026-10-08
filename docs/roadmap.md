# Roadmap

1. Foundation: workspace, core/TUI, unsigned EFI/initramfs, persistent QMP + SSH
   harness; real boot/discovery/shell/cleanup scenario.
2. Pool policy: safe read-only/no-mount imports by GUID, discovery search sources,
   duplicate names and degraded/unavailable fixtures.
3. Boot environments: ZFSBootMenu properties and dataset compatibility, filesystem
   visibility, encryption state, clear rejection reasons.
4. Linux boot: kernel/initrd discovery, inspectable boot plan and kexec, installed
   guest handshake. Prove the selected environment actually booted.
5. NixOS: Bootspec, generations and specialisations with real fixtures.
6. Snapshots: temporary clones, explicit ownership and cleanup after failed boot.
7. Recovery: key loading, locked/degraded pools and interrupted operations.
8. Agent adapter: optional MCP over the existing harness, not a parallel engine.
9. Verified boot: the dedicated Secure Boot milestone below.
10. Hardening/release: reproducible images, kernel/ZFS pinning, packaging,
    compatibility matrix and physical boot acceptance.

Each step adds deterministic core checks and a reusable real VM scenario. Keep
serial state, screenshots, SSH results, boot handshake and physical-device
acceptance distinct in reports.

## Secure Boot and TPM milestone (in progress)

The specification is [Secure Boot and verified boot](secure-boot-model.md), with
[configuration](configuration.md#secure-boot-configuration) and
[acceptance gates](verification.md#planned-secure-boot-acceptance).

- Implemented foundation: immutable `off`/`enforce` schema with compiled image
  mode, independent firmware requirement and incomplete-config rejection.
- Implemented: validate or separately configure the loader kernel and matching
  ZFS; inspect final configuration and independently verify staged signatures.
  Pre-deployment embedded certificate inventory remains to be automated.
- Implemented: authenticated early IMA setup and separate owner UKI signing/deployment;
  keep production private keys out of Nix and fixture artifacts.
- Implemented foundation: pinned CMS BootAuthorization, exact arguments/typed ZFS
  roots, snapshot-source permission, opaque sealed input plans, a privileged broker
  and UI privilege dropping. Enforced Linux 6.18.55 IMA/memfd interoperability and
  actual synthetic handoff passed; NixOS typed-root authorization requires systemd initrd.
- Implemented foundation: restricted PID-1/recovery/failure paths, no enforced
  root shell or automatic reboot loop; separate firmware/authorization evidence.
- Add selected off/optional/required TPM capabilities, ordinary measurements and
  signed-policy NvPCR setup. Handle scarce NV capacity independently of kernel
  verification. Define measured-target/event-log handoff across kexec; no TPM
  disk unlock. SRK, phase/PCR15 measurement and v262 signed-policy NvPCR paths are
  implemented. Initial swtpm success/PCR replay, bad/missing policy, no-TPM and
  optional degradation tests pass; scarce NV/stale-index/interruption tests and
  event-log handoff remain in progress.
- Passed: negative/positive signed OVMF synthetic-input scenario, actual broker
  catalog with installed NixOS ZFS root and authorized snapshot clones, including
  unauthorized arguments, foreign mounts/owners and restart reconciliation.
  Next: target TPM consumer/handoff contract, generic Linux, encrypted-root
  passphrase UX and physical Secure Boot/recovery acceptance.

Deferred: UI action “Boot without a trusted signature”, owner-authenticated
administrative recovery, optional development-image packaging, TPM automatic
unlock/rollback-resistant state, whole-root integrity and shim/MOK.
There is no automatic policy downgrade when firmware Secure Boot is disabled and
no requirement to ship two images. Signature encoding, broker sandbox/IPC and
IMA staging passed the configured-kernel VM experiment. TPM measurement and
NvPCR support are now part of the active milestone, not the deferred unlock work.
