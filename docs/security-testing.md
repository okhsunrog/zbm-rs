# Owner signing and protected VM acceptance

Nix constructs an unsigned image from public trust inputs. Owner deployment signs
an output copy outside `/nix/store`; it never supplies private keys to a Nix
derivation. Default flake images still use `off`. An `enforce` image must be
constructed with `lib.mkImage` or the documented NixOS options and matching public
trust. Neither this workflow nor a VM report enrolls keys on the physical host.

## Publisher

An enforced Nix result includes `root.cpio`, `kernel.config`,
`kernel-capabilities.json`, `ima-policy`, `stub.efi` and `os-release` alongside
the ordinary image files. `nix/publish-loader.py` unpacks only bounded newc archives
with safe paths and inode types. It checks the packaged IMA certificate against
the owner signing certificate, signs the policy, signs and verifies each staged
module, repacks the root and builds a fresh UKI. EFI signing is the last step.
The ready output directory is renamed into place only after all steps succeed.
Signing, size-limit or verification failures leave no ready output directory.

```sh
uv run --no-project python nix/publish-loader.py result-enforce deployment-new \
  --efi-key /private/efi.key --efi-cert /public/efi.pem \
  --ima-key /private/ima.key --ima-cert /public/ima.pem \
  --module-key /private/module.key --module-cert /public/module.pem \
  --sign-file /matching-kernel-dev/lib/modules/VERSION/build/scripts/sign-file \
  --evmctl /path/to/evmctl --ukify /path/to/ukify
```

The module signer and target-kernel signer must be trusted by the loader kernel.
The IMA certificate must be a **non-CA leaf** with digitalSignature usage, signed
by a certificate accepted by the loader's restricted IMA keyring. Merely copying
an arbitrary public certificate into the image does not establish that trust.
The tested kernel refuses to add a CA certificate to `.ima`.

`evmctl --sigfile` also attempts to set an xattr. Unprivileged owner staging uses
`--xattr-user --sigfile`; the publisher independently verifies the detached
signature. Trusted early startup restores it as `security.ima` on the policy
inode, then writes its **absolute pathname** to securityfs. Writing raw policy
rules is rejected when the kernel requires signed policies.

Production publishing refuses the test/SSH profile. Disposable tests must
explicitly pass `--allow-fixture`; `--fixture-directory` additionally inserts
synthetic target inputs. Such an image is never a production recovery image.
Private fixture signing keys remain outside the guest and Git.

## Signed PCR policy

NvPCR selection requires `image.pcrPublicKey`. Publishing that image additionally
requires `--pcr-key /private/pcr.key --systemd-measure /v262/lib/systemd/systemd-measure`.
The publisher checks the key's derived public half against the packaged public
key. It signs SHA-256 PCR11 policy for `enter-initrd` with `policyref=initrd`.

Order matters:

1. Sign the IMA policy and all modules, then create the final compressed initramfs.
2. Build the UKI with its `.pcrpkey` public key.
3. Read the exact measured PE section bytes, including text termination and `.sbat`.
4. Calculate and sign their PCR11 state plus `enter-initrd`.
5. Add the unmeasured `.pcrsig` and verify that measured sections stayed identical.
6. Sign and verify the complete EFI image.

Putting `.pcrsig` into the measured initramfs would introduce a circular dependency.
systemd-stub supplies it in the additional `/.extra` initramfs instead. Private
PCR keys never enter the UKI. The v262 provider initializes selected NvPCRs using
PolicyAuthorize; no legacy anchor-secret fallback is provided. This policy
authorizes a TPM operation and does not replace boot-input verification.

`ukify --join-pcrsig` fills an existing policy-digest section. It does not create
that section when absent, even if the command succeeds. The publisher builds a
fresh `.pcrsig` section and verifies its JSON plus unchanged measured sections.
The initial missing-section failure is preserved in `verified-tpm-001`.

## Persistent scenario

`xtask/fixtures/security.py` constructs a synthetic second kernel/initramfs and
signed authorizations from explicitly supplied disposable key paths. It creates
valid inputs plus wrong IMA signature, unsigned kernel, wrong CMS, wrong arguments
and changed-initramfs cases. Run its `--help` for inputs. The target only proves
actual kernel handoff, final arguments, a changed boot ID and guest poweroff;
it is not a complete installed Linux/ZFS root.

The OVMF variables template must contain only disposable firmware trust matching
the fixture EFI signer. Use matching 4M Secure Boot CODE/VARS firmware. Each run
copies the template rather than modifying it. A production enrollment or host
TPM must never be used for this scenario.

```sh
cargo xtask secure-smoke --image deployment-test-new \
  --secure-vars /disposable/OVMF_VARS.fd --run target/vm/verified-new --tcg

# The harness owns and reaps both QEMU and a fresh disposable swtpm process:
cargo xtask secure-smoke --image deployment-tpm-test-new \
  --secure-vars /disposable/OVMF_VARS.fd --run target/vm/verified-tpm-new \
  --swtpm --tcg
```

The scenario checks actual firmware Secure Boot, signed IMA readiness, native
kernel/initramfs acceptance and rejection, exact CMS/argument/content checks,
legacy-kexec refusal, zero UI capabilities and IDs 65534, shell refusal, protected
crash recovery and a real second-kernel handoff. The optional swtpm path checks
TPM/NvPCR readiness and the target-prepared PCR15 event. Startup and prepared
measurement logs are retained; the prepared event is not an execution proof.
Guest TPM state lives in the fresh sibling `<run>.tpm` directory.

TPM-enabled scenarios capture the real firmware event log, systemd userspace
event log, exact prepared-plan JSON and SHA-256 PCR11/PCR15 values. The independent
`xtask/fixtures/check_tpm.py` replays the logs against those actual PCRs and checks
the prepared event's digest against the exact final plan. A single `enter-initrd`
event is required even after manager crash/restart. The VM test transport waits
for NIC/SSH readiness separately from visible recovery or manager readiness.

`xtask/fixtures/tpm_policy.py` creates explicitly disposable, EFI-signed negative
images with absent or invalid PCR policy while preserving every measured input.
Use `secure-smoke --expect-tpm rejected` for required-profile failures;
`--expect-tpm unavailable` and `--expect-tpm degraded` exercise optional profiles.
Optional TPM failure must still pass the boot-input rejection and actual-handoff
checks; it never selects weaker verification. A missing optional NvPCR and a
missing ordinary TPM capability have separate recorded states.

## Installed NixOS and snapshot scenarios

`xtask/fixtures/authorize_nixos.py` publishes a fresh disposable copy of a generated
NixOS root archive. It rejects traversal, devices, hardlinks and symlink ancestors,
never follows guest absolute symlinks on the host, signs/verifies all target module
files, signs the kernel and final initramfs, and uses `nix/authorize-boot.py` for
each ordered plan. Keys are passed only to signing tools; the original Nix store
is never rewritten. This helper packages test roots, not an installed-system updater.

Authorize the exact NixOS systemd-root plan, with no global snapshot-test argument:
the target proof identifies ordinary or snapshot-clone root from the actual ZFS
mount. Each generation has its own argument ID beneath the shared artifact ID.
The indexed root value stays variable while all other tokens and their order are
fixed. JSON/CMS signatures still enforce source dataset and owned-clone permission.

```sh
cargo xtask boot-smoke --image deployment-protected-loader \
  --fixture deployment-authorized-nixos --secure-vars /disposable/OVMF_VARS.fd \
  --swtpm --run target/vm/verified-nixos-new

cargo xtask boot-smoke --snapshot --image deployment-protected-loader \
  --fixture deployment-authorized-nixos --secure-vars /disposable/OVMF_VARS.fd \
  --swtpm --run target/vm/verified-snapshot-new
```

These extend the existing harness, including QMP keyboard/PNG observations,
SSH provisioning on disposable disks, manager-restart/ownership checks and the
selected OS's multi-user proof. `boot-smoke` owns/reaps QEMU and swtpm, using the
same shared swtpm lifetime helper as `secure-smoke`. The ordinary protected case
adds a syntactically valid generation with unauthorized parameters and requires
the broker to refuse it with an empty kexec slot before explicitly selecting the
trusted older generation. The snapshot case exercises authorized clone preparation,
foreign-owner refusal, explicit discard, restart reconciliation and actual clone boot.

The ordinary target TPM gate requires a ready SRK service and persisted public SRK
after kexec, not just loader readiness. These test roots include `tpm_crb` in their
preboot module list. The v261 target's vendor `.nvpcr` definitions must be masked
with empty `/etc/nvpcr/{hardware,login,cryptsetup,verity}.nvpcr` files when testing
ordinary TPM only. The loader v262 already owns its selected hardware NvPCR;
letting the older target provider allocate all default indices causes an existing
index/policy conflict and unwanted allocations. Masking those definitions does
not disable ordinary TPM/SRK or delete an index. Target NvPCR consumers and event
log handoff need the separate versioned integration contract, still pending.

The target proof always captures early/late TPM setup journals when a device is
available, then requires `ZBM_TARGET_TPM_SRK_READY` before publishing boot success.

`lib.mkProtectedBootFixture { kernelPackages = configuredLoaderKernelPackages; }`
is the canonical Nix profile for those target masks and required LSMs. Supply the
same configured kernel/ZFS set as the protected loader. Build the unsigned archive,
then publish a fresh outside-store copy with `authorize_nixos.py`:

```sh
uv run --no-project python xtask/fixtures/authorize_nixos.py \
  result-protected-fixture deployment-authorized-nixos \
  --kernel-key /private/kernel.key --kernel-cert /public/kernel.pem \
  --module-key /private/module.key --module-cert /public/module.pem \
  --ima-key /private/ima.key --ima-cert /public/ima.pem \
  --authority-key /private/authorization.key --authority-cert /public/authorization.pem \
  --sign-file /matching-kernel-dev/lib/modules/VERSION/build/scripts/sign-file \
  --evmctl /path/to/evmctl
```

## Recorded scope and remaining gates

For installed-OS acceptance, `nix/boot-fixture.nix` accepts a separate
`kernelPackages` and `extraNixosModules`. The default fixture still uses the stock
kernel; the protected profile is explicitly selected.
The protected fixture must use the tested configured kernel with matching ZFS,
and explicitly retain `ima` and `lockdown` in NixOS `security.lsm`: the ordinary
NixOS default otherwise emits a narrower `lsm=` argument despite the kernel's
configured default. Fixture preparation is not target capability approval.

`target/security-fixtures/nixos-signed-001` contains two authorized NixOS
generations sharing one image pair but distinct argument indexes. Owner publishing
signed 7297 installed-root modules, the preboot modules, kernel and final initramfs
outside Nix. `verified-nixos-002` passes real ZFS-root boot through the broker/menu;
`verified-nixos-snapshot-001` passes writable trusted clone boot with source and
snapshot preserved. Their actual selection/prepared-clone PNGs were inspected.
The final SRK-scoped target fixture additionally passes ordinary and snapshot
boot in `verified-nixos-srk-001` / `verified-nixos-snapshot-srk-001`, including
early/late target SRK reuse and public-key persistence without extra NvPCR allocation.

`target/vm/verified-004/security-report.json` passed under OVMF Secure Boot using
Linux 6.18.55 and matching ZFS. This confirmed IMA appraisal of sealed, read-only
memfd inputs and actual file-based kexec. The earlier `verified-001` preserved
the CA-certificate rejection; `verified-003` preserved a screenshot stability
failure caused by a blinking cursor. The harness now pauses QEMU while capturing
stable screenshots and resumes only if it was running.

Initial TPM successes/refusals and final source regression are recorded in
[verification](verification.md#tpm-and-signed-policy-nvpcr-acceptance).
Before production: verify generic-Linux authorization, encrypted-root passphrase
UI, expected embedded kernel
certificates, algorithm/profile restrictions, full-NV/stale-index/interruption
cases and log transport
across kexec, target-initrd consumption and physical Framework recovery. Insyde
EFI enumeration recovery is a separate DMI-scoped provider and remains pending
here. No attestation, rollback-resistance or whole-root-integrity promise follows
from this synthetic scenario.
