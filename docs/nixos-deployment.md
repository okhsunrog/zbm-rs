# Native NixOS generation installation

Import the flake's `nixosModules.default` and enable
`boot.loader.zbm-rs.enable`. This is independent of `programs.zbm-rs.enable`,
which builds an EFI image. By default generation installation leaves EFI files
and NVRAM alone. Optional `boot.loader.zbm-rs.efi.enable` installs one separately
prepared signed EFI image after successful generation publication. Neither path
changes the pool bootfs or another boot environment.

The NixOS external bootloader hook runs on `nixos-rebuild boot`. It reads the
standard generation `boot.json`, copies its kernel and initrd outside the store,
executes the generation's `initrdSecrets` program on that copy, then optionally
signs the kernel, final initrd and exact native boot plan. No secrets or private
keys become derivation inputs. Original Bootspec, kernel, initrd and init paths
remain immutable. The loader never executes `initrdSecrets` during discovery or
handoff.

Prepared files live in `/boot/zbm-rs/bundles`, with one atomic generation link
under `/boot/zbm-rs/nixos/<system-store-basename>`. `prepared.json` has version 1
and `bootspec_sha256`, binding the prepared pair to the exact source Bootspec.
Only the kernel/initrd locations are substituted; init and ordered parameters
remain source-Bootspec values. Malformed or stale records fail closed. A source
with `initrdSecrets` but no prepared record is rejected with an installation
diagnostic. A source without secrets can still use direct Nix store artifacts.

This record is discovery metadata, not permission to boot. Enforced handoff still
checks owner-pinned CMS authorization, exact kernel/initrd bytes, ordered
arguments, typed root selection, EFI kernel signature and initrd IMA signature.
Source-bound metadata alone cannot authorize replacement artifacts.

The module supplies an early initrd ZFS-root generator and a versioned marker in
that initrd. The planner keeps normal fstab processing for these images, including
`neededForBoot` home mounts, while the early generator mounts the explicitly
selected root. Ordinary older initrds retain the conservative root-rewrite path.
Only the root changes for snapshot clones; data mounts retain the NixOS
configuration's explicit destinations. `/nix` must stay in the root dataset.

```nix
{
  imports = [ inputs.zbm-rs.nixosModules.default ];
  boot.loader.zbm-rs = {
    enable = true;
    signing = {
      enable = true;
      kernelKey = "/var/lib/secureboot/db.key";
      kernelCertificate = "/var/lib/secureboot/db.pem";
      imaKey = "/var/lib/secureboot/ima.key";
      imaCertificate = "/var/lib/secureboot/ima.pem";
      authorityKey = "/var/lib/secureboot/db.key";
      authorityCertificate = "/var/lib/secureboot/db.pem";
    };
  };
  boot.initrd.secrets."/etc/zfs/pool.key" = "/etc/zfs/pool.key";
  boot.initrd.systemd.services.zfs-import-pool = {
    requires = [ "initrd-nixos-copy-secrets.service" ];
    after = [ "initrd-nixos-copy-secrets.service" ];
  };
}
```

All signing paths above are runtime strings. The loader image must trust the
chosen authority and IMA certificates. The selected OS kernel must independently
have appropriate module trust, lockdown and kexec settings. The installer does
not disable module verification or modify unsigned module derivations in place.
`nix/sign-module-tree.py` can sign a public copy of an aggregate module tree
outside Nix, independently verify every module and make it suitable as an
externally signed build input. Keep that input and its exact kernel ABI paired.

## Specialisations

Installing a generation also prepares the named children declared in its
`org.nixos.specialisation.v1` Bootspec extension. Each declaration must match the
Bootspec reached through `<parent>/specialisation/<name>`. The loader reads each
child's own Bootspec and source-bound prepared record. One invalid child is shown
as unavailable without hiding its usable parent or siblings.

The target list shows `Generation N · name` beside the main `Generation N` row.
The default remains the main configuration selected by the system profile;
choosing a variant in the menu does not rewrite that profile. The broker resolves
its own catalog and distinguishes the generation, variant and system closure,
including variants with the same kernel. Exact argument and artifact verification
still applies in enforced mode.

```nix
{ lib, ... }:
{
  specialisation.niri.configuration = {
    services.desktopManager.plasma6.enable = lib.mkForce false;
    services.displayManager.defaultSession = lib.mkForce "niri";
    programs.niri.enable = true;
    fileSystems."/home".device = lib.mkForce "pool/data/home-niri";
  };
}
```

Create the alternate home dataset separately. Specialisations do not clone ZFS
data or isolate `/var/lib`. Reboot to change a `neededForBoot` home mount. A root
snapshot clone retains the selected variant's explicit home destination.

Publication validates every child before changing prepared links. If a later
child fails preparation or signing, earlier variants' prepared links and GC roots
are restored. This is failure rollback within the installation process, not a
crash-atomic multi-file transaction. Signed authorization records can remain after
a failed installation; they do not make an unlisted or modified target bootable.
The optional EFI update runs once after all variants have been prepared.

## Optional EFI publication and independent rescue

`boot.loader.zbm-rs.efi` accepts a runtime `image` path, public `certificate`,
explicit ESP `partuuid`, relative `path` (default `EFI/zbm-rs/zbm-rs.EFI`) and
firmware `label` (default `zbm-rs`). Prepare and sign that image outside derivations
before invoking the generation hook. The hook independently verifies it, stages
and syncs it on the selected FAT partition, then replaces only that filename.
Repeated installation leaves identical bytes untouched. Missing NVRAM entries
are appended without reordering existing choices. An existing label pointing to
another partition or path is an error. No fallback EFI file is overwritten.

An independent rescue image should use `security.mode = "off"`, an ordinary
kernel without forced signature/lockdown enforcement, and a separately pinned
source/package set. Publish it explicitly through `nix/rescue-efi.py` with
`certificate: null`, its own label and path. This rejects signed files:
the recovery bypass must require the owner to disable firmware Secure Boot.
Normal generation publication never installs or updates rescue. This mechanism
does not promise that TPM-dependent OS services work in recovery; the loader's
shell and manual pool unlock remain available.

Separate pins prevent ordinary updates from replacing rescue, but do not certify
that a pinned revision can boot the current machine or read its pool. The current
installer checks PE/signature policy and stages/syncs a single file; it does not
boot-test the image or provide crash-atomic FAT filesystem updates. It also
cannot recover an unreadable ESP or a failed disk.

The rescue publisher stages an explicit candidate and retains the known-good
rescue, independent of normal loader updates. Pin the complete image
inputs, retain the exact artifact/manifest with a GC root, and test the final
unsigned EFI bytes in OVMF before installing a candidate at a distinct path.
Keep the preceding rescue reachable while the candidate is tested on hardware;
promote only after an explicit successful physical test. `rescue-efi.py PLAN`
stages only the candidate. `rescue-efi.py PLAN --promote TESTED_SHA256` requires
the exact immutable plan, candidate bytes and synced receipt to agree before
replacing the known-good slot. Its separate entry is appended without moving
existing boot choices. A missing/stale receipt after interrupted staging cannot
be promoted. Publication requires a non-test unsigned cancellation-only image,
record protocol v2 and the declared swap/credential policy. Image metadata contains
the exact EFI hash and capabilities reported by its compiled userspace.

The disposable FAT/OVMF installer test checks staging, explicit promotion,
interrupted receipts, changed candidates, wrong policy/protocol, rejection of
signed/test images, and preservation of the main loader and foreign files.
This is publication acceptance, not a boot test of a production rescue image or
an automatic claim that a candidate worked on hardware. A one-commit lag is not
a validation policy. Keep a tested copy on a separate bootable device for shared
ESP/disk failures.

An unsigned rescue remains a manual break-glass path, requiring firmware Secure
Boot to be disabled. Do not count on BootOrder automatically recovering a kernel
hang or duplicate the main image at the removable-media path and call that an
independent recovery version. VM acceptance covers the tested firmware/devices,
not physical boot or every future pool feature. Before enabling hibernation,
rescue must also implement the cancellation-only contract in the
[protected-boot architecture](hibernation-design.md#agreed-protected-boot-architecture-not-yet-implemented).

TPM SRK/NvPCR setup remains owned by the selected OS. Package the actual loader's
signed PCR policy and public key in that OS's initrd as appropriate; don't reset
the TPM or treat an EFI signature as a substitute for OS policy.

Failed publication restores the preceding prepared-generation link. Publication
does not delete older bundles. `zbm-prune-generations` previews stale entries;
`zbm-prune-generations --apply` removes recognized stale entries, their GC roots
and unshared bundles. Retained profiles, current and booted systems, and their
specialisations are protected. Unknown entries and authorizations are retained.
The automatic Nix GC service runs this reconciliation after pruning profiles,
then runs GC again. Explicit `nix-collect-garbage` does not invoke the service;
run the reconciliation separately after such manual generation deletion.
Keep `/boot` and `/nix` within the selected root so snapshots contain the system
closure, prepared files and authorizations together.

There is no ZFSBootMenu-style kernel alias, stable `init=/boot/nixos-current/...`
or command-line property. Arch's separate generic-Linux path is unchanged.

## Validation

`nix build .#checks.x86_64-linux.native-nixos` exercises an encrypted disposable
pool through the real installation hook, first boot with an early home mount,
missing-secret rollback, actual updated-generation boot and snapshot-clone boot.
The test also checks quoted Bootspec parameters and absence of duplicate sysroot
units. The protected NixOS fixture uses prepared copies while leaving the source
kernel and initrd untouched; signed OVMF/swtpm handoff verifies ordinary and
snapshot roots and rejects an altered argument plan sharing the signed images.
The encrypted Arch ordinary/snapshot scenarios remain separate regression gates.
None of these tests attach the host disk or establish physical laptop acceptance.
