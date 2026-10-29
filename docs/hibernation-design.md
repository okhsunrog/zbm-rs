# Native hibernation integration

This implementation is experimental and disabled by default. It changes neither
installed images nor host sleep settings. It does not implement ZFS hibernation
cookies, authenticate a saved RAM image, or prove that a pool was unchanged while
the machine was off. It must not be advertised as making ZFS hibernation safe.

## Existing flow

Keep the pools, environments, generations/kernels and snapshot views. There is no
new startup question, resume menu entry or shortcut. A recognized session selects
its exact environment/target by default. Menu/countdown/immediate retain their
existing timing. Enter on that target resumes; the footer says Resume. Unlock
uses the existing password panel. Cancelling the countdown or navigating does
not cancel the saved session.

The context line shows Resume pending, the target carries a RESUME tag, and
Selection/details explain the action. Compact layouts retain the context line.
Errors and operation progress keep their normal priority.

Navigation, search, help, details, snapshot inspection, rescan, manager restart
and power off preserve the session. F10 works from dialogs, including unlock and
cancellation confirmation, without another question. Letter P retains its
context-sensitive behavior; typing a password or search query never powers off.

Another target, generation or specialisation; snapshot-clone boot; clone
preparation/deletion/creation; promotion; rollback; and an unrestricted shell
require confirmed cancellation. Warn when executing, never when highlighting or
inspecting. Esc preserves the session and selection. Existing discard/promote/
rollback dialogs include the session-loss warning without a second question.
Rollback retains typed ROLLBACK. Security-disabled actions do not cancel anything.
Q/crash recovery cannot expose an unguarded shell while a session is pending.

## Internal ownership

`zbm_core::resume` models the session. `resume::linux::Swap` implements bounded
Linux header parsing, boot-record storage, artifact checks and image cancellation.
The TUI coordinator calls it directly in ordinary mode and through the existing
privileged broker in enforced mode; there is no external service or socket
protocol. PID 1 uses the same status for recovery. Target initramfs uses the same
binary and module through `--resume-restore`.

Runtime imports are read-only at pool level while a saved session exists,
including broker rediscovery. An already writable import is refused. Configured
read-only policy remains independent and is never silently upgraded. After
confirmed cancellation, a temporarily read-only owned import can be reopened for
an authorized mutation. A status read error is never treated as no session.

## Linux on-disk format

Initial support is deliberately limited to a dedicated, unencrypted GPT swap
partition with 4096-byte kernel pages and the native Linux S1SUSPEND format.
LUKS, zvols, swapfiles, other page sizes and alternate image formats are unsupported.
The image configuration selects only an explicit `resume.swap_partuuid`.

Linux reserves the first 1024 bytes of swap for bootbits and preserves them when
marking a hibernation image. Before swapon/hibernation, zbm-rs writes a versioned,
size-bounded compressed boot record there: a nonce, pool/target identity, hashes
of kernel/initramfs, and exact planned arguments. Existing nonzero foreign
bootbits are refused. A record is never added to an existing image. The record
is boot routing metadata, not a ZFS cookie or an authenticated memory image.

Image detection uses the actual kernel signature. A missing/invalid boot record
leaves a cancellable image with Resume unavailable; it never guesses a target.
Consent is bound to a hash of the current header. Cancellation reopens the block
device exclusively (rejecting active swap), rechecks the header, restores only
the original SWAPSPACE2 signature, syncs, and reads back the result. It preserves
the payload; this is invalidation, not secure erasure. Unknown signatures and
changed images are refused.

An in-progress marker in root-owned /run prevents a failed or interrupted write
from becoming apparently successful via page-cache reads after manager restart.
Power off remains available. A subsequent boot inspects the on-disk signature
again; ambiguous/partial headers remain blocked. Successful cancellation removes
the marker only after flush and verification. The marker itself never authorizes
resume or replaces image invalidation.

## NixOS integration

The optional `boot.loader.zbm-rs.hibernation` module integration adds a native
pre-hibernation record step and a target-initramfs resume gate. Its swapPartuuid
must match the loader image's resume.swap_partuuid. The swap partition must
already be configured/active; the module neither formats it nor partitions disks.
The record step temporarily swaps it off, records the booted NixOS target, and
reactivates it even if recording fails. Failure aborts hibernation. Memory pressure
can make swapoff fail; that is not overridden. This hooks direct hibernation;
suspend-then-hibernate and hybrid-sleep are disabled while this integration is
enabled. Persistent initramfs mounts must all use the gated ZFS imports;
configurations with other early persistent filesystems are rejected.

`hibernate=noresume` disables the kernel's automatic attempt; the stock systemd
resume generator and units are suppressed in the target initramfs. Plain
`noresume` is unsuitable because systemd also treats it as disabling hibernation
device selection. Every configured ZFS import requires
successful completion of zbm-resume.service, rather than merely being ordered
after it. The native gate verifies the saved arguments and refuses to run after
persistent filesystems are mounted or ZFS pools are imported. It invokes the
kernel resume interface explicitly. A returned resume attempt fails the gate;
it never silently falls back to mounting ZFS.

On a cold boot with an ordinary swap signature, the gate initializes the kernel
resume device for the next systemd hibernation. Recording verifies the currently
mounted root dataset and actual running arguments against the published plan.

The operator must explicitly opt into NixOS's unsafeAllowHibernation policy.
The loader rejects this feature with enforced security because image
authentication/lockdown integration is not implemented. ESP diagnostics must be
disabled until their writes can be coordinated with saved filesystem state.
These restrictions are not waived by the UI.

## Remaining safety boundary and validation

Header identity and artifact hashes cannot detect arbitrary intervening writes
to ZFS. A strong consistency seal needs kernel/ZFS cooperation tied to the actual
hibernation snapshot; a pre-sleep TXG reading is not sufficient. Independent
bootloaders/rescue systems can bypass loader restrictions. Raw swap stores RAM
contents without encryption. This implementation does not address those risks.

Tests cover wrong/stale image identity, cancellation preserving other bytes,
foreign bootbits, corrupt metadata, artifact changes, normal navigation, modal
Esc/F10, combined rollback confirmation, compact rendering and disabled defaults.
Source/unit checks are not physical resume acceptance. Before any installation,
exercise the exact image and target initramfs in disposable VMs, including power
loss, failed resume, restart, other generations and read-only import behavior;
then validate the actual laptop separately.
