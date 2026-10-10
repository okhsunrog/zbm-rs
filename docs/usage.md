# Boot environments and keyboard controls

The interface follows pools, boot environments, boot targets and snapshots.
Selection and inspection are separate from booting or changing ZFS data.

## Linux and NixOS boot paths

Enter on a pool explicitly imports it without force and discovers boot environments.
The list follows ZFSBootMenu visibility: `mountpoint=/` unless
`org.zfsbootmenu:active=off`, or `mountpoint=legacy` with `active=on` (including
inherited values). `bootfs` marks and selects the default BE; it never triggers
automatic boot. Eligible unencrypted datasets are mounted read-only; hidden and
non-root datasets are not mounted. Encrypted or `canmount=off` candidates remain
visible with a diagnostic. Enter on a BE opens its Linux kernels or NixOS generations; B goes back.
An individual mount or Bootspec failure leaves other BEs and generations
available. Rejected generations retain their generation number and diagnostic.
Enter on a generation resolves its Bootspec,
loads its kernel/initrd, unmounts the roots owned by this manager and exports its
owned pools before executing kexec. Only pools imported by zbm-rs are eligible for export;
restart reconciliation checks actual pool GUIDs and existing mountpoints.

The Linux backend follows ZBM kernel/initramfs naming in `/boot`, naturally sorts
versions, honors `org.zfsbootmenu:kernel`, and reads `commandline` (including
`%{parent}` expansion) or a per-kernel `.kcl` file. `rootprefix` is explicit or
inferred from `ID`/`ID_LIKE` without executing os-release. The loader suppresses
conflicting `root=`/`zfs=` arguments and names the selected dataset, including
clones. Existing initramfs images are used unchanged. Paths and matching pairs
are checked again before handoff; ZFS properties are freshly read.

The NixOS backend assumes that /nix/store and /nix/var/nix/profiles are in the
selected root filesystem. Separate /nix datasets, encrypted roots, initrdSecrets,
specialisations and kernel arguments requiring whitespace/quoting are unsupported.
A configured root different from the selected dataset requires an automatically
recognized systemd initrd for root override; scripted NixOS initrd root override
is unsupported. No target initrd is rewritten.
Discovery itself remains non-mutating. Boot actions are unavailable in local
preview mode. Autoboot is not implemented.

Linux ZBM properties do not override NixOS Bootspec. Pool
`org.zfsbootmenu:readonly` and the image import policy govern mutations.
Encrypted BE unlock/keysource, duplicate via send/receive and some other ZBM
features are still unimplemented; this is not full ZBM feature parity.

## Snapshots and recovery

From the BE list, T opens snapshots when `settings.ui.showSnapshots` is enabled.
Enter inspects a snapshot's Linux targets or NixOS generations read-only.
C prepares an owned boot clone, D discards a prepared clone, and Enter on a target
prepares/reuses a clone and boots it. Snapshot listings are capped at the newest
256 per BE.

O creates a persistent ordinary boot environment from the selected snapshot;
M creates and promotes it, after confirmation. These operations do not require
a discoverable boot target or NixOS metadata. The clone is selected in the normal
BE list; its boot properties are preserved and pool bootfs is unchanged.
U rolls the original dataset back to the selected snapshot, after typing
`ROLLBACK`. This discards current changes and newer snapshots (`zfs rollback -r`),
but never force-destroys dependent clones or recursively rolls back child datasets.
Owned inspection mounts are removed first; foreign mounts or read-only policy
block the operation. Boot targets are rediscovered afterward.

No installed-system flag or custom Bootspec extension is required. Linux clone
boot uses the selected dataset via the native root prefix. NixOS clone boot
inspects the actual `/init` in a bounded newc initramfs (gzip, xz or zstd) and
requires systemd initrd, with the Nix store in the root dataset; additional initrd
mounts such as a separate /usr remain unsupported. The legacy
`programs.zbm-rs.snapshotBoot.enable` option is a compatibility no-op.
Rollback and persistent clone/promotion do not depend on this initrd check.

Mutations require a writable owned pool import. Read-only policy is never
silently upgraded; `org.zfsbootmenu:readonly` also forbids mutations.
Prepared boot clones are separate from ordinary persistent BEs.
Prepared clones have `canmount=noauto`, `active=off` and an ownership token.
Linux boot clones use `mountpoint=/` for native initramfs compatibility; NixOS
boot clones use `mountpoint=legacy` with the explicit systemd root override. An ephemeral `/run` journal records intent before creation and verifies
the token, origin and snapshot GUID before restart reuse or explicit discard.
Discard uses non-recursive destruction and refuses mounted or retained clones.

Clones are marked `org.zbm-rs:state=retained` before handoff. They remain after
boot, reboot or a failed handoff, preserving writes from the target OS. They
are hidden from ordinary BE discovery and are never automatically deleted.
The user can inspect their origin and ownership properties and manage retained
clones with ZFS tooling. Reuse of prepared clones is currently limited to
manager restarts within the same loader boot; cross-reboot cleanup/adoption
is intentionally deferred.

Administrative rollback, persistent clone creation and promotion are available
in `off` mode. Enforced images allow authorized snapshot boot through the broker
and keep administrative actions unavailable until an owner-authentication flow
is implemented.

## Keyboard controls

R rescans, N requests a clean manager restart and P asks PID 1 to power off.
In `off`, S asks PID 1 for a shell; enforced images refuse that administrative
shell request. Exiting the supervised shell starts a fresh manager with coherent ZFS
state. Q exits manager; without an intent PID 1 treats that as unexpected exit.
Outside supervision S runs a local shell and Q exits normally.
Boot operations run individually while recovery keys remain available. A
30-second operation timeout cancels the async command and requires Shell or
Restart before further boot actions; the next manager reconciles recorded
resource intent. Blocking OS-root reads run outside the input loop, and shutdown
does not wait indefinitely for those reads.

The TUI uses a candidate list and a concise selection summary on wide consoles
(110 columns or more). The summary shows the boot source, generation and snapshot
clone state; full store paths and arguments stay in the I/F3 details panel.
Smaller consoles keep the list, details shortcut and recovery controls.
The layout uses base ANSI background colors for Linux-console compatibility,
with cyan selection, magenta snapshot labels and explicit text for every state.
The header reports the configured boot policy, not a verification result.
Bootspec previews show the generation's actual kernel, initrd, init and arguments. Execution still
revalidates the boot inputs. Rejected generations can be inspected but cannot boot.

- `/` starts fuzzy search in the current list. Enter accepts the filter without
  booting; Esc clears it. Filters and selection survive navigation back to a list.
- F1/`?` shows keyboard help; F2 opens all actions, including unavailable actions
  with their reasons. Both use the same command inventory as dispatch.
- Existing Enter/B/T/C/D/R/S/N/P/Q bindings remain available outside search.
  F4 Shell, F5 Rescan, F6 Restart manager and F10 Power off provide function-key
  alternatives; recovery remains available during search and pending operations.
- D opens a confirmation before discarding a prepared owned clone. Esc cancels;
  Enter confirms. Clone ownership and non-recursive destruction stay in core.

N restarts only the manager; Q exits it and leaves recovery to PID 1. These actions
do not reboot the machine. The interface does not yet provide chroot, full pool
status, kernel-argument editing, diff or other unimplemented ZBM actions.

## Supported layouts

| Layout or feature | Support |
| --- | --- |
| Generic Linux kernel/initramfs pairs in `/boot` | Discovery and ordinary boot. |
| NixOS Bootspec generations with `/nix` in the selected root | Ordinary boot and verified VM boot. |
| Snapshot clone with a recognized systemd initramfs | Explicit writable-clone boot for the supported root layout. |
| Encrypted boot environment requiring interactive unlock | Pending. |
| Separate `/nix`, NixOS specialisations or `initrdSecrets` | Pending. |
| Automatic boot timeout | Reserved configuration; not implemented. |

Read [verified boot](secure-boot-model.md) for protected-mode policy and
[the roadmap](roadmap.md) for planned layouts and actions.
