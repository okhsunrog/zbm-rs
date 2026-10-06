# NixOS generations with upstream ZFSBootMenu

This document describes an alternative way to select NixOS generations: publish
each generation as a kernel/initrd entry for upstream ZFSBootMenu, with a separate
`.kcl` file containing that generation's command line. A NixOS installation hook
can produce these entries from Bootspec without changing ZFSBootMenu itself.

Status on 2026-10-06: the required upstream mechanisms were checked against the
documentation and source. Host-side checks using the inspected upstream helpers
confirmed the example kernel/initrd name matching and separate `.kcl` reads.
Those checks used placeholder files and did not execute kexec. The adapter
described below is a proposal; this repository does not implement it, and it has
not passed an upstream-ZFSBootMenu VM or physical boot test. Existing zbm-rs boot
tests exercise a different loader.
This note records an alternative, without changing the project's implementation
or roadmap.

The inspected ZFSBootMenu source revision is
[`e15503228f40b3c95ded551fab86e91f3e3d230f`][zbm-source]. NixOS source links use
[`494ce7fd23ff6a5dff39e1fb11e9b6f2ac74bf25`][nixos-bootspec], the nixpkgs revision
in this project's lock file when the note was written.

## A generation contains more than a kernel

A NixOS generation identifies a system closure: the configuration and programs
needed to start that system, including services and their settings. Its boot
data includes a kernel, an initrd when required, a stage-2 `init` executable and
kernel parameters. Bootspec records these associations in `boot.json`.
See [NixOS RFC 0125][bootspec-rfc].

Two generations can share the same kernel while differing in service
configuration, system packages, initrd contents or parameters:

| Generation | Kernel | Stage-2 init | Example configuration difference |
| --- | --- | --- | --- |
| 42 | Kernel K | Closure A `/init` | Previous service settings |
| 43 | Kernel K | Closure B `/init` | Updated service settings |

Changing only the kernel while retaining generation 43's `init=` still selects
generation 43's system configuration. A generation selector must keep the
kernel, initrd, `init` and arguments associated with the selected generation.
Generation selection preserves the dataset's mutable contents; reverting the
filesystem requires a separate snapshot/clone/rollback policy.

ZFSBootMenu's kernel menu can represent this selection. Each menu entry must
carry a distinct generation identity, even when its kernel bytes are identical
to another entry's. The menu's name describes its interface, not a limit on the
system configuration that its command line can select.

## What the published NixOS integration does

Sirn Thanabulpong's [ZFSBootMenu on NixOS][original-article] uses
`boot.loader.external.installHook` to prepare a conventional `/boot` layout.
The published script removes previous kernel/initramfs entries, chooses the
newest generation, copies its artifacts and updates the dataset's
`org.zfsbootmenu:commandline` with that generation's `init=`. Its generation loop
ends with `break`, so the script publishes one generation.

The proposed adapter keeps multiple generations and gives each its own command
line. The article's single-generation restriction therefore describes that
integration, rather than the full capability of current upstream ZFSBootMenu.

## Entry format

ZFSBootMenu documents a command-line file named exactly like its kernel, with
`.kcl` appended. A per-kernel file supplies that kernel's arguments. A file may
include `%{parent}` to incorporate arguments from the dataset property.
See [Passing Kernel Command Lines to Boot Environments][zbm-man].

An illustrative publication for two generations is:

```text
/boot/vmlinuz-nixos-g0000000042
/boot/initramfs-nixos-g0000000042.img
/boot/vmlinuz-nixos-g0000000042.kcl

/boot/vmlinuz-nixos-g0000000043
/boot/initramfs-nixos-g0000000043.img
/boot/vmlinuz-nixos-g0000000043.kcl
```

The example names follow upstream's kernel/initrd matching rules. In particular,
`initramfs-nixos-g0000000042.img` matches the suffix of
`vmlinuz-nixos-g0000000042`. Discovery and matching are documented in the
[boot-environment primer][zbm-primer] and implemented by `find_be_kernels()` and
`find_be_initramfs()` in the [inspected source][zbm-source].

The generation-42 `.kcl` could contain one line shaped like:

```text
init=/nix/store/aaaaaaaa-nixos-system-demo-g42/init loglevel=4
```

Generation 43 would use its own `init` and parameters. These store paths are
illustrative placeholders, and this line is not a complete root-mount recipe.
The real publisher must use the selected Bootspec and a validated root policy.

Prefer the immutable store path from Bootspec's `init` field. Avoid using the
mutable `/nix/var/nix/profiles/system` link as the identity of every entry.

Write the complete command line on one line: upstream's `read_kcl_value()` reads
the first line. It selects the file when present, and otherwise falls back to
the property. See the [command-line implementation][zbm-kcl-source].

Keep generation-specific `init=` and parameters in `.kcl`. If `%{parent}` is used,
reserve the inherited property for intentionally shared settings and ensure it
does not introduce another `init=` or conflicting root policy. Treat interactive
command-line edits in the ZBM menu as overrides during acceptance testing.

## Proposed installation hook

The [external bootloader interface][nixos-external] calls the installation hook
with the requested system toplevel as its argument. A publisher should use that
argument together with retained profiles, rather than infer the requested
default solely from the largest generation number. A rollback can request an
older toplevel while newer profiles still exist.

A suitable implementation would perform these steps:

1. Acquire a publisher lock and identify the intended BE and publication
   directory. Include the hook's requested toplevel in the candidate set.
2. Enumerate retained `system-N-link` profiles, resolve their closures and read
   each closure's `boot.json`. Define the retention limit explicitly.
3. Validate each candidate independently: architecture, accessible paths, regular
   kernel/initrd files, supported extensions and root policy. Preserve usable
   entries when another generation is invalid; report the rejected generation.
4. Prepare the generation's own boot artifacts and command line. Process required
   initrd-secret handling before making the entry visible.
5. Publish a distinct kernel, matching initrd and `.kcl` for every accepted
   generation. Record ownership, toplevel identity and artifact digests in an
   adapter manifest.
6. Apply the configured default policy only after publication succeeds.
7. Retire only entries owned by this publisher, following the retention and
   garbage-collection policy. Keep a previous usable entry through failures.

This is an implementation contract, not a ready-to-run shell script. A minimal
first implementation could support one profile, one architecture and a
self-contained root dataset, rejecting more complex layouts explicitly.

### Use generation-specific Bootspec data

For each entry, read `kernel`, `initrd`, `init`, `kernelParams`, `system` and
`toplevel` from its own Bootspec. Do not use the currently evaluated
`config.boot.kernelParams` for every historical generation. Generations may have
different boot requirements even when their kernel versions match.

Serialize the argument list with deliberate quoting rules. Reject NUL, embedded
newlines and unsupported argument forms; a plain join must not silently change
argument boundaries. Test the serializer against ZBM's command-line parser.

Modern NixOS generates Bootspec as part of the system closure. At the inspected
nixpkgs revision, `boot.bootspec.enable` has been removed because generation is
unconditional. Do not add that obsolete option to a new module. For historical
closures without Bootspec, deliberately support a synthesizer or reject them
with a diagnostic. See the [NixOS generator][nixos-bootspec] and
[external-backend guidance][nixos-external-doc].

### Name entries by generation identity

Do not deduplicate entries by kernel version or digest. Copies or managed hard
links may share bytes, but their paths and `.kcl` associations must remain
distinct. Hard links require the same filesystem and immutable publication;
copying is a simpler initial policy. Absolute symlinks into `/nix/store` need
special care because the loader resolves paths in its own environment.

Put the profile and generation identity early in the filename. A production
scheme should also disambiguate a reused generation number and specialisations,
for example with an immutable closure identifier. Use a safe filename alphabet
and enforce path-length limits. The shorter names above illustrate the basic
mapping only.

Upstream orders discovered names with `sort -V` and normally picks the last
usable entry; `org.zfsbootmenu:kernel` can select another one. If kernel version
comes before generation identity, a newer generation using an older kernel may
sort below an older generation. Define both ordering and default selection,
including interaction with a user's existing property. See `select_kernel()` in
the [source][zbm-source] and the [kernel-management interface][zbm-kernel-menu].

### Publish complete entries

Stage files outside the scanned kernel namespace. For a new, immutable entry
name, publish the initrd and `.kcl` first and expose the kernel last. A visible
kernel should always have its intended companions. Complete writes and required
file/directory synchronization must precede a successful installation report.

Do not overwrite an existing entry's companions independently: that could leave
an old kernel paired with a new initrd or `init`. Use a new identity when contents
change. An interrupted publisher should retain previously committed entries.

Maintain an ownership manifest instead of deleting every `/boot/vmlinuz-*` or
`/boot/initramfs-*` file. Other bootloaders, manually prepared recovery entries
and other distributions may share that directory. Define recovery of partially
published entries and serialize cleanup with publication.

### Keep the selected closure alive

Copied kernel/initrd files do not preserve the system closure referenced by
`init=`. Published entries must remain backed by retained profiles or explicit
managed GC roots until they are retired. See the [Nix garbage-collector roots
documentation][nix-gc-roots].

Order retirement so that a boot-visible entry does not lose its closure first.
Also account for a bootloader generation limit that differs from the system's
profile-retention policy. The manifest should make it possible to explain which
entry holds which closure alive.

## Root selection needs its own compatibility policy

The association of kernel, initrd and `init` solves generation selection.
Mounting the correct root remains a separate requirement.

In the inspected source, ZFSBootMenu removes `root=` from an entry's arguments,
then builds the root argument from its selected filesystem and root prefix.
Consequently, blindly copying `root=fstab` or a fixed dataset from Bootspec into
`.kcl` does not preserve its meaning. A publisher must recognize and deliberately
translate root policy. See `load_be_cmdline()`, `find_root_prefix()` and
`kexec_kernel()` in the [source][zbm-source].

Inspect the selected generation's initrd expectations. Systemd and scripted
initrds, legacy ZFS mountpoints and managed ZFS mountpoints can require different
root-prefix, filesystem and mount-option handling. The article includes
[NixOS 26.05-specific notes][original-article] for this reason; those notes should
not be treated as a recipe for every generation.

For ordinary boot, test that the requested BE is actually mounted as `/`.
Reject conflicting root parameters instead of relying on whichever duplicate
argument wins. If the adapter manages a dataset-wide root-prefix property, all
published generations must be compatible with that policy. Mixed initrd
conventions may require separate BEs or an explicitly designed hook.

### Snapshot and clone boot

Upstream provides [snapshot-management operations][zbm-snapshots], including
creating a clone. A generation-aware publication stored inside the snapshotted
filesystem can preserve its historical `.kcl`, kernel/initrd and store contents.

A proposed scenario is:

1. Publish generations and take a consistent snapshot of the BE.
2. Explicitly create a writable clone using ZFSBootMenu's snapshot tools.
3. Select an entry whose `init` and closure exist in that clone.
4. Have the target initrd mount the selected clone, rather than the original
   dataset named in its generated fstab or scripts.
5. Verify writes land on the clone and the source dataset/snapshot stay intact.

Step 4 requires an explicit, tested NixOS-root adaptation. `.kcl` by itself does
not make every NixOS initrd clone-aware. Additional initrd mounts, separate `/nix`
datasets and state outside the snapshot need a consistent-layout policy.
Defer these layouts in an initial adapter rather than claim universal snapshot
boot support.

The zbm-rs implementation uses an opt-in Bootspec capability and a systemd-root
override for a restricted layout; see [the snapshot module](../nix/snapshot-boot.nix),
[the boot-plan resolver](../crates/core/src/boot.rs) and
[the documented restrictions](../README.md#snapshots-and-recovery). That mechanism is a
reference for a possible compatibility policy, not proof that the proposed
upstream adapter already handles clones.

## Initrd secrets and specialisations

Bootspec's `initrdSecrets` field describes an executable that modifies a writable
copy of the initrd during installation. Its required host files must be available,
and failure must prevent publication of that entry. Do not write the resulting
initrd back to the Nix store. This behavior is specified in
[RFC 0125][bootspec-rfc].

The external loader module defaults `boot.loader.supportsInitrdSecrets` to false.
An adapter must implement the complete handling before declaring support. Define
permissions and storage for the resulting initrd, especially if it contains an
encryption key. A publication directory on an unencrypted ESP has different
protection from a `/boot` directory inside an encrypted BE.

Specialisations need separate entries with their own Bootspec data. Their identity
should include profile, generation and specialisation name, with an explicit
default for the base system. Reject unsupported extensions or variants before
publication; do not silently substitute the base configuration.

## Relationship to zbm-rs

| Concern | Proposed upstream adapter | Current zbm-rs NixOS backend |
| --- | --- | --- |
| Generation resolver | Installation hook reads Bootspec and publishes entries | Loader reads profiles and Bootspec at boot |
| Artifacts | Prepared kernel/initrd copies plus per-entry `.kcl` | Files accessed inside the mounted BE or snapshot |
| Command line | Serialized during publication, then interpreted by ZBM | Constructed from the selected Bootspec by core |
| Menu | Generation identities represented in the kernel menu | Explicit generation model and menu |
| Required loader | Upstream ZFSBootMenu | This project's Linux/initramfs image |
| Clone-root compatibility | Policy still needs implementation and acceptance | Restricted opt-in systemd-root path exists |

A correctly implemented adapter can select the same NixOS system closure as a
direct Bootspec-aware loader. The mechanisms differ in when metadata is resolved
and where the integration lives. Native Bootspec parsing is therefore an
architectural choice, not a prerequisite for offering a generation menu.

Upstream's existing recovery and encryption facilities are available to an adapter,
subject to its installation and root policies. Current zbm-rs still rejects
encrypted NixOS BE boot, separate `/nix`, `initrdSecrets` and other layouts listed
in [the README](../README.md#linux-and-nixos-boot-paths).

## Acceptance scenarios for an implementation

An upstream-adapter fixture should preserve screenshots, serial logs, published
files, `.kcl` contents and target-OS evidence. Menu visibility alone is insufficient.

| Scenario | Required evidence |
| --- | --- |
| Two generations sharing a kernel | Each entry reaches its own `/run/current-system`, command-line marker and a generation-specific service/configuration marker |
| Older generation selected | Its expected toplevel runs, even while a newer profile exists |
| Newer generation with an older kernel version | Explicit selection and configured default both choose the intended generation |
| Requested rollback | The hook argument is respected when newer profiles remain |
| Broken generation | Other complete entries remain bootable; the rejection names its cause |
| Publisher interrupted at every publication boundary | Previously committed entries survive; no exposed entry lacks its intended `.kcl` or initrd |
| Retention plus Nix GC | Every remaining published entry still has its complete system closure |
| Foreign boot files present | Publication and cleanup preserve them |
| Root-policy variants | Target `/` is the exact selected dataset for each supported initrd/mountpoint combination |
| Snapshot clone | Correct generation and clone root boot; writes change only the clone |
| Secrets and specialisations | Correct prepared initrd/variant boots, or unsupported inputs are explicitly rejected |

The target proof should include a changed kernel boot ID, exact toplevel and root
dataset. Run a disposable upstream-ZFSBootMenu fixture separately from zbm-rs's
existing fixture. Physical boot and Secure Boot require their own acceptance;
this note makes no claim for either.

For zbm-rs, [the accepted Secure Boot design](secure-boot-model.md) adds a common
signed BootAuthorization above both the Linux and Bootspec adapters. Publication
of Bootspec, `.kcl` files or matching kernel/initrd names alone is not owner
authorization. An enforced loader must bind the actual artifacts and allowed
arguments/root variation, including snapshot clones. This is planned zbm-rs work,
not a claim that the upstream adapter described here provides that trust chain.

## Sources

- [Original NixOS integration article][original-article].
- [ZFSBootMenu boot-environment and artifact-matching rules][zbm-primer].
- [ZFSBootMenu per-kernel command lines and root properties][zbm-man].
- [ZFSBootMenu kernel selection/default UI][zbm-kernel-menu].
- [ZFSBootMenu snapshot UI][zbm-snapshots].
- [Inspected kernel discovery, selection and kexec source][zbm-source].
- [Inspected per-kernel command-line reader][zbm-kcl-source].
- [NixOS Bootspec RFC][bootspec-rfc].
- [Pinned NixOS Bootspec generator][nixos-bootspec].
- [Pinned NixOS external loader contract][nixos-external] and
  [backend guidance][nixos-external-doc].
- [Nix GC roots][nix-gc-roots].

[original-article]: https://grid.in.th/2024/12/zfsbootmenu_on_nixos/
[zbm-primer]: https://docs.zfsbootmenu.org/en/latest/general/bootenvs-and-you.html
[zbm-man]: https://docs.zfsbootmenu.org/en/latest/man/zfsbootmenu.7.html#passing-kernel-command-lines-to-boot-environments
[zbm-kernel-menu]: https://docs.zfsbootmenu.org/en/latest/online/kernel-management.html
[zbm-snapshots]: https://docs.zfsbootmenu.org/en/latest/online/snapshot-management.html
[zbm-source]: https://github.com/zbm-dev/zfsbootmenu/blob/e15503228f40b3c95ded551fab86e91f3e3d230f/zfsbootmenu/lib/zfsbootmenu-core.sh
[zbm-kcl-source]: https://github.com/zbm-dev/zfsbootmenu/blob/e15503228f40b3c95ded551fab86e91f3e3d230f/zfsbootmenu/lib/zfsbootmenu-kcl.sh
[bootspec-rfc]: https://github.com/NixOS/rfcs/blob/master/rfcs/0125-bootspec.md
[nixos-bootspec]: https://github.com/NixOS/nixpkgs/blob/494ce7fd23ff6a5dff39e1fb11e9b6f2ac74bf25/nixos/modules/system/activation/bootspec.nix
[nixos-external]: https://github.com/NixOS/nixpkgs/blob/494ce7fd23ff6a5dff39e1fb11e9b6f2ac74bf25/nixos/modules/system/boot/loader/external/external.nix
[nixos-external-doc]: https://github.com/NixOS/nixpkgs/blob/494ce7fd23ff6a5dff39e1fb11e9b6f2ac74bf25/nixos/modules/system/boot/loader/external/external.md
[nix-gc-roots]: https://nix.dev/manual/nix/2.32/package-management/garbage-collector-roots.html
