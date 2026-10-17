# Roadmap

zbm-rs is in early development. The next release needs to make supported boot
layouts and recovery dependable on real hardware. Current test coverage is
listed in [verification](verification.md); design details live in
[architecture](architecture.md) and [verified boot](secure-boot-model.md).

## Working features

- Generic Linux kernel/initramfs discovery and ordinary ZFS-root boot.
- Explicit hidden passphrase unlock, including encrypted Arch root and snapshot-clone boot.
- NixOS Bootspec generation selection with `/nix` in the selected root.
- Snapshot inspection, owned writable boot clones, and ordinary-mode rollback,
  persistent clones and promotion.
- Search, target details, keyboard help and compact-console layouts.
- PID 1 supervision, console restoration and bounded recovery after failure.
- Reproducible Nix images, portable/host-only profiles and a persistent VM harness.
- Enforced boot inputs and protected recovery, with signed OVMF acceptance for
  synthetic targets, installed NixOS/Arch roots and authorized snapshot clones.
- Read-only TPM readiness and verified prepared-target measurement in PCR15.
- Explicit Framework/Insyde recovery-module image profile; physical acceptance remains separate.

These features have source and VM validation. They do not establish physical
deployment acceptance or full ZFSBootMenu feature parity.

## Secure Boot and TPM milestone

The enforced NixOS and Arch paths are implemented. Remaining work includes:

- Automated embedded-certificate inventory and signing-profile checks.
- PCR15 event/plan transport across kexec for independent replay in the OS.
- OS-owned signed NvPCR policy integration with a systemd v262 target.
- Physical Secure Boot, trusted snapshot boot and recovery on the intended hardware.
- Physical acceptance of DMI-scoped Insyde EFI-variable enumeration recovery.

The loader does not initialize SRK/NvPCR objects or extend OS phases in PCR11.
Those operations belong to the selected OS. See [TPM ownership](tpm.md),
[configuration](configuration.md#secure-boot-configuration) and
[acceptance gates](verification.md#secure-boot-acceptance-matrix).

## Boot support and release work

- Automatic key-source handling and raw/hex-key input beyond passphrase unlock.
- NixOS specialisations, separate `/nix` layouts and a defined `initrdSecrets` flow.
- Locked/degraded pool recovery and clear diagnostics for unsupported layouts.
- Explicit autoboot policy and timeout behavior.
- Deployment, signing-key rotation and trusted recovery packaging.
- Published compatibility matrix and physical boot acceptance.

Each new boot path should include a reusable disposable-VM scenario that reaches
the selected OS and verifies its root and system closure.

## Deferred ideas

- Owner-authorized administrative recovery in enforced images.
- An explicit UI action for booting an untrusted target.
- Optional development-image packaging.
- TPM unlock, remote attestation and rollback-resistant state.
- Whole-root integrity and shim/MOK integration.

There is no automatic policy downgrade when firmware Secure Boot is disabled,
and no requirement to ship two images.
