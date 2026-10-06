# Boot handoff and trust model

The canonical Nix artifact is an unsigned UKI. Private signing keys belong to a
separate deployment step, not a derivation or the Nix store. No signing service
or key management is implemented here.

The initial NixOS executor uses Linux kexec_file_load exclusively. It reports
kernel verification/lockdown errors to the UI and does not silently fall back to
kexec_load. A loaded kernel is unloaded if preparation fails. Before executing
the handoff, the manager unmounts its read-only OS roots and exports only pools
whose GUIDs match their import ownership records. KexecStarting tells PID 1 that a
return is failed handoff, rather than an ordinary manager crash.

This does not establish an end-to-end Secure Boot chain. Firmware validation of
the loader UKI, target kernel signature verification, authentication of the
Bootspec/initrd/command line, and trust in the target root filesystem are separate
requirements. This implementation does not authenticate the Bootspec or initrd,
and must not advertise Secure Boot acceptance based on an unsigned QEMU test.

Initial Bootspec support is org.nixos.bootspec.v1 for the loader architecture.
Specialisations, extra initrd extensions and initrdSecrets execution are outside
the first boot path. An initrdSecrets requirement is rejected; scripts found in
the selected OS root are not executed to prepare the initramfs. Kernel arguments
with quoting or whitespace are currently rejected rather than reinterpreted.

The disposable NixOS fixture uses no force import, verifies that the root is the
selected ZFS dataset, reports the selected generation and current-system path,
and powers off. OVMF tests currently run without Secure Boot enforcement. Physical
boot, signed-kernel handoff and lockdown require separate acceptance tests.

References: [NixOS Bootspec RFC](https://github.com/NixOS/rfcs/blob/master/rfcs/0125-bootspec.md),
[Linux kexec_file_load verification tests](https://github.com/torvalds/linux/blob/master/tools/testing/selftests/kexec/test_kexec_file_load.sh).
