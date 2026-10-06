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

## Secure Boot milestone (accepted design; not implemented)

The specification is [Secure Boot and verified boot](secure-boot-model.md), with
[planned configuration](configuration.md#planned-secure-boot-configuration) and
[acceptance gates](verification.md#planned-secure-boot-acceptance).

- Add immutable `off`/`enforce` policy and independent firmware requirement to the
  shared configuration schema; reject incomplete enforced configuration.
- Validate or separately configure the loader kernel and matching ZFS; check final
  capabilities, certificate trust and all preboot module signatures.
- Add authenticated early IMA setup and separate owner UKI signing/deployment;
  keep production private keys out of Nix and fixture artifacts.
- Implement common signed BootAuthorization for Linux/NixOS and typed command-line
  rules, including authorized snapshot/clone root selection.
- Introduce a privileged broker and unprivileged manager within the existing ELF;
  use opaque verified plans and immutable prepared inputs through file-based kexec.
- Restrict every PID-1/recovery/failure path; no automatic root shell or weaker
  fallback in `enforce`. Show separate firmware/authorization/appraisal evidence.
- Pass negative and positive signed OVMF scenarios before separate physical Secure
  Boot and recovery acceptance. Existing unsigned tests do not satisfy this gate.

Deferred: UI action “Boot without a trusted signature”, owner-authenticated
administrative recovery, optional development-image packaging, TPM measured
boot/automatic unlock/rollback-resistant state, whole-root integrity and shim/MOK.
There is no automatic policy downgrade when firmware Secure Boot is disabled and
no requirement to ship two images. Signature encoding, broker sandbox/IPC and
IMA-compatible immutable staging still need implementation choices and experiments.
