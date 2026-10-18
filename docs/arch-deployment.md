# Arch and Framework deployment

Building, owner signing, target authorization and installing an EFI entry are
separate steps. The default portable image intentionally remains unsigned and
uses `security.mode=off`. It is not a Secure Boot deployment artifact.
Keep a known-good signed ZFSBootMenu and its firmware entry throughout testing.
Do not replace it until the additional loader has passed physical boot, snapshot
boot, restart/recovery, keyboard and target-OS TPM checks.

## Encrypted roots

The broker validates the selected pool GUID, boot dataset and encryption-root
ownership. Unlock may ask for a dependent keysource root's passphrase. Success
resumes the original boot/browse action; incorrect input keeps the prompt open,
and Escape cancels. One common root is requested at startup, independent roots
only on demand. Supplied passphrases use secret stdin and are not tried on peers.

Local file:// keylocations and inherited org.zfsbootmenu:keysource are supported.
An encrypted source on an owned imported pool is unlocked and mounted read-only.
Paths are resolved relative to its normal mountpoint, then its dataset root,
without symlink escape. Keys must be regular files of at most 4 KiB. File keys,
including OpenZFS raw/hex formats, briefly use a root-only RAM cache removed after
use and on protected manager restart. Missing files and dependency cycles offer a
direct passphrase where keyformat allows it. Network keys and hooks are unsupported.
Persistent encryption properties remain unchanged. No secret appears in UI
telemetry, argv, Nix inputs, ESP diagnostics or persistent storage.

Keys loaded into ZFS do not transfer across kexec. The selected OS initramfs must
unlock its root with its own trusted arrangement. Never put production disk keys
in a public EFI or Nix store. Unlock does not authorize a boot or a recovery shell.

## Authorize Arch boot inputs

The authorizer accepts Arch's `zfs=DATASET` selector as well as `root=ZFS=`,
`root=zfs:` and explicit `root=` plus `rootfstype=zfs`. Mixed/duplicate root
selectors are refused. Only the typed root value varies for owned snapshot clones;
all other arguments and their order remain signed.

Use the exact immutable configuration of the intended loader. Read-only planning
uses the same Rust discovery/planning code as booting, including inherited ZBM
properties, per-kernel `.kcl` files, root-prefix choice and extra image arguments:

```sh
zbm-rs --linux-plans / POOL/ROOT/arch /public/image-config.json
uv run --no-project python nix/authorize-linux.py \
  --loader /path/to/zbm-rs --root / --dataset POOL/ROOT/arch \
  --config /public/image-config.json --plan-only
```

After reviewing the plans, publish content-addressed authorizations:

```sh
sudo uv run --no-project python nix/authorize-linux.py \
  --loader /path/to/zbm-rs --root / --dataset POOL/ROOT/arch \
  --config /public/image-config.json \
  --kernel-cert /public/kernel.pem \
  --ima-key /private/ima.key --ima-cert /public/ima.pem \
  --authority-key /private/authority.key --authority-cert /public/authority.pem \
  --evmctl /path/to/evmctl --allow-snapshot-clones \
  --output /boot/zbm-rs/authorizations
```

The helper requires the requested ZFS dataset to be mounted at the selected root.
It verifies the existing kernel's PE signature, signs a temporary copy of the
unchanged initramfs and publishes JSON/CMS only after preparing all pairs. Signing
keys are consumed only by external tools, outside Nix. Existing matching valid
authorizations are retained. Missing/corrupt signatures and differing existing
payloads fail closed; no old authorization directories are garbage-collected.

Run this after kernel signing and the final initramfs rebuild in the existing
distribution update workflow, before publishing new boot snapshots. On Arch it
belongs after mkinitcpio/dracut and existing kernel-signing hooks, not before them.
An update-hook failure must be visible to the administrator. The loader refuses
changed bytes until a new matching authorization exists. Existing ZFSBootMenu
`.zbm-policy` signatures are a different format and are not implicitly trusted.
This repository does not install a hook or alter `/boot` automatically.

## Framework EFI enumeration recovery

Pass the installed host's explicit eight-digit `hostId` to `lib.mkImage` or the
Framework profile (for example, `hostId = "1234abcd";`). It creates `/etc/hostid`
and the matching `spl.spl_hostid` loader argument. No host file is read implicitly,
and this does not enable forced pool import. The NixOS module defaults
`image.hostId` to `networking.hostId`. Keep the same identity across alternative
loaders and the installed OS; zero and malformed IDs are refused.

`nix/framework-image.nix` is an explicit production profile for Framework Laptop
13 Intel Core Ultra Series 1 with Insyde firmware. Supply reviewed hardware JSON,
public trust certificates and `moduleSource`, the public source directory of the
separately maintained `insyde-efivarfs` module. Only its five build/source files
enter the derivation; no DKMS state, private signing keys or host ZFS cache enters
the image. Pin the source revision in the consuming configuration.

The profile uses the pinned flake's latest kernel series (the module source needs
Linux 7.2 Rust APIs), configures enforced kernel trust and builds the module against
the **final loader kernel**. Its DMI guard rejects other machines. Modprobe receives
`expose=1` through image-owned configuration and runs before Secure Boot/IMA
readiness. A required-module failure stops bootstrap. Recovery changes in-memory
efivarfs visibility, not firmware trust or variable contents.

General image consumers can instead supply `extraModulePackages = kernelPackages:
[ matchingPackage ]`, `forcedModules` and `moduleOptions`; the NixOS module exposes
`image.extraModulePackages` and `image.moduleOptions` too. Modules are included in
the ordinary owner publisher's signing/verification inventory. Copying a `.ko`
from the running OS is not supported: its ABI and trust may differ.

Publish the image through [owner signing](security-testing.md), using private keys
outside Nix. Separately preserve the target OS's signed NvPCR policies when changing
the loader UKI; [TPM ownership](tpm.md#os-owned-signed-pcr-policies) explains that
independent OS requirement. Nothing in this profile migrates or resets the TPM.

## Disposable acceptance

```sh
cargo xtask arch-smoke --encrypted --image result-snapshot \
  --fixture /disposable/arch-fixture --run /disposable/arch-encrypted-new
cargo xtask arch-smoke --encrypted --mode snapshot --image result-snapshot \
  --fixture /disposable/arch-fixture --run /disposable/arch-encrypted-snapshot-new
```

Protected cases additionally take `--secure-vars` and `--signing-profile`. The
profile JSON requires `disposable_fixture: true` and explicit `kernel_key`,
`kernel_cert`, `ima_key`, `ima_cert`, `authority_key`, `authority_cert`, `evmctl`
paths for disposable credentials only. The harness signs downloaded fixture inputs
on the host; no signing key is uploaded. It exercises argument refusal before
actual authorized Arch handoff. Never attach the host pool, disk or real keys.

VM reports establish those specific scenarios, not physical Framework acceptance.
