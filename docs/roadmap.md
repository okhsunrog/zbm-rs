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
9. Hardening/release: reproducible images, kernel/ZFS pinning, packaging, signed
   boot-chain policy, compatibility matrix and physical boot acceptance.

Each step adds deterministic core checks and a reusable real VM scenario. Keep
serial state, screenshots, SSH results, boot handshake and physical-device
acceptance distinct in reports.
