# Documentation

## Using and building

- [Boot environments and controls](usage.md): Linux/NixOS layouts, snapshots,
  clone ownership and keyboard actions.
- [Building images](build.md): Nix outputs, hardware profiles, NixOS integration
  and userspace choices.
- [Configuration](configuration.md): immutable JSON, Nix options and defaults.

## Architecture and security

- [Architecture](architecture.md): supervisor, manager, broker and ZFS ownership.
- [Secure Boot and verified boot](secure-boot-model.md): artifact trust,
  BootAuthorization, argument/root policy and recovery.
- [TPM and measured boot](tpm.md): loader measurements, OS-owned setup and logs.
- [Signing and protected tests](security-testing.md): owner tooling and
  disposable OVMF/swtpm scenarios.

## Development

- [Development and VM harness](development.md): local checks, QMP/SSH controls
  and execution rules.
- [Verification](verification.md): exercised boundaries and remaining gates.
- [Roadmap](roadmap.md): current work and deferred features.

## Reference material

- [Configuration representation](research/configuration-design.md).
- [Image size experiments](research/size-experiments.md).
- [Whole-userspace musl](research/musl-experiment.md).
- [NixOS generations with upstream ZFSBootMenu](research/nixos-generations-with-zfsbootmenu.md).
- [Screenshot sources](screenshots/README.md).
- [October 2026 verification records](history/verification-2026-10.md).
