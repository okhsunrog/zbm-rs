# Building zbm-rs

Nix builds the EFI image, kernel/ZFS pair and initramfs. The installed operating
system's kernel and existing bootloader are separate from this build.

## EFI images

Enable the Nix nix-command/flakes features, then:

```sh
nix build .#zbm-rs-efi
nix build .#zbm-rs-efi-test --out-link result-test
```

On hosts with those features disabled, prefix commands with
`nix --extra-experimental-features 'nix-command flakes'`.
The production result contains the complete unsigned EFI artifact, kernel,
initramfs, manifest and image-size measurements:

```text
result/esp/EFI/BOOT/BOOTX64.EFI
result/vmlinuz
result/initramfs.img
result/manifest.json
result/sizes.json
```

Nix builds Rust from the locked sources, selects kernel and ZFS modules from one
kernelPackages set, checks their ABI/version pairing, includes selected tools and
ELF dependencies, creates cpio/zstd and invokes systemd ukify. Normal builds use
neither dracut nor mkinitcpio nor /boot from the development host. No image-building
code runs inside zbm-rs. `cargo xtask image [--test-ssh]` delegates to this same
Nix derivation.

The test image compiles the opt-in vm-test feature and adds loopback-forwarded SSH,
private test controls and disposable fixture credentials. Its key is public
Nix-store test data, not a security credential; never use this profile as a normal
boot image or with persistent sensitive guest data. Production omits those hooks.

## Hardware profiles and NixOS integration

Portable includes common storage/input and QEMU drivers. Host-only takes explicit
module/firmware inputs. Probe outside Nix, inspect the JSON, then pass it to mkImage:

```sh
uv run --no-project nix/probe-hardware.py hardware-manifest.json
```

`lib.mkImage { profile = "host-only"; hardwareManifest = ./hardware-manifest.json; }`
is the reusable build API for a consuming flake. Commit the reviewed manifest with
the caller's configuration; derivations never read /sys. Module names must exist
in the selected kernel, so incompatible manifests fail rather than being ignored.
`zbm-rs-efi-host-only-example` is explicitly a QEMU fixture, not a probe of your host.

Import `nixosModules.default`, enable `programs.zbm-rs.enable` and select its
`image.profile`. `system.build.zbm-rs-efi` then uses the same builder with
`boot.kernelPackages`, `boot.zfs.package`, `boot.initrd.availableKernelModules`
and `boot.initrd.kernelModules`. The module exposes an artifact; it does not
silently replace the machine's existing bootloader.

Image composition lives under `programs.zbm-rs.image`; immutable runtime policy
lives under `programs.zbm-rs.settings`, mirroring the Rust/JSON hierarchy:

```nix
programs.zbm-rs = {
  enable = true;
  image.profile = "portable";
  settings = {
    ui.timeout = 5; # Reserved until autoboot is implemented.
    manager.restartLimit = 2;
    zfs.importPolicy = "read-only";
    nixos.generationLimit = 20;
  };
};
```

Previous flat option names remain deprecated aliases. The module-config Nix check
compares generated defaults with Rust defaults, validates both range endpoints,
and checks that legacy aliases produce the same JSON.

## Userspace and image size

Release builds use size optimization, FatLTO, one codegen unit and panic unwinding.
The default image uses glibc, zstd-19, eudev and ZFS without optional URL fetching.
Native ZFS encryption with local keys is included; HTTPS keylocations require
the full-userspace profile. Interactive encrypted-BE unlock remains on the roadmap.

| Output | Purpose |
| --- | --- |
| `zbm-rs-efi` | Default unsigned image. |
| `zbm-rs-efi-test` | Disposable VM image with SSH and harness controls. |
| `zbm-rs-efi-test-snapshot` | Writable-import fixture for snapshot boot tests. |
| `zbm-rs-efi-full-userspace` | Upstream ZFS URL-fetch support and systemdMinimal. |
| `zbm-rs-efi-musl` | Experimental whole-userspace musl image. |

`lib.mkImage` accepts `initramfsCompression = "gzip" | "xz" | "zstd"` and a
`rustProfile` attrset for comparison builds. Every image writes `sizes.json`;
[`nix/image-size-policy.json`](../nix/image-size-policy.json) defines growth
allowances and absolute limits. Change a baseline together with an explanation
of the image-content change. Host-only limits also depend on the selected hardware.

See the [size measurements](research/size-experiments.md),
[musl experiment](research/musl-experiment.md) and
[immutable configuration](configuration.md).

## Signed images

The canonical Nix result is unsigned. A protected deployment supplies public trust
inputs, selects `security.mode = "enforce"` and signs a fresh output copy using
owner keys outside Nix. Follow [the signing workflow](security-testing.md).

The NixOS module exposes `system.build.zbm-rs-efi`; building it does not install
an EFI image or enroll firmware keys. Physical deployment and recovery validation
are tracked in [the verification matrix](verification.md).
