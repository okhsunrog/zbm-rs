# Arch and Framework deployment

Building, owner signing, target authorization and installing an EFI entry are
separate steps. The default portable image intentionally remains unsigned and
uses `security.mode=off`. It is not a Secure Boot deployment artifact.
Keep a known-good signed ZFSBootMenu and its firmware entry throughout testing.
Do not replace it until the additional loader has passed physical boot, snapshot
boot, restart/recovery, keyboard and target-OS TPM checks.

## Encrypted roots

Selected-pool inspection displays locked boot environments without loading keys.
Enter on a mountable passphrase-encrypted environment opens hidden input. Escape
cancels; a wrong passphrase leaves the key unavailable and permits another try.
The broker rechecks the selected pool, dataset, encryption-root ancestry and key
format. It overrides `keylocation` with `prompt` and sends the supplied bytes via
zfskit's secret stdin support. It never opens a keylocation, loads keys recursively
or changes persistent encryption properties. Buffers owned by the UI and RPC
framing are cleared on drop; this is not a claim that every library/allocator
copy is erased. Passphrases are absent from UI telemetry, argv and disk files.

An already available key permits normal read-only discovery, including snapshot
inspection and explicit writable clone preparation. Pool export before handoff
does not transfer the loaded key into the next kernel. The selected OS initramfs
must still support its own ZFS unlock, either by prompting again or through its
existing trusted key arrangement. Never put a real disk key in a public EFI image
or the Nix store. Raw/hex keys and ZBM `keysource` discovery are not implemented.

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
