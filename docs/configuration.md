# Immutable image configuration

Configuration is an image input. Nix serializes settings to JSON, and the shared
[Rust schema](../crates/tui/src/config/schema.rs) validates them during the build
and at process startup. Rebuild the EFI image to change normal settings.

The image contains a read-only Nix-store configuration file at
`/etc/zbm-rs/config.json`. PID 1 reads it before starting a manager; each manager
reads it before starting its async runtime. Supervised roles ignore developer
`ZBM_RS_CONFIG` overrides. There is no runtime writer or live reload.

The security mode is also compiled into the executable and must match the JSON.
An enforced executable cannot downgrade by removing or changing its runtime
configuration. File permissions alone are not a security boundary: unrestricted
root recovery can alter a RAM-backed initramfs, which is why enforced recovery
limits administrative access.

## Development

Local preview uses [the default configuration](../config/default.json) unless
an explicit file is supplied:

```sh
cargo run -p zbm-rs -- --manager --preview
ZBM_RS_CONFIG=config/test.json cargo run -p zbm-rs -- --manager --preview
cargo run -p zbm-rs -- --validate-config config/test.json
cargo run -p zbm-rs -- --print-config
uv run --no-project nix/check-config.py
```

`ZBM_RS_CONFIG` participates in Cargo input tracking and build validation as well
as local execution. Missing or invalid explicit files fail. `--print-config`
normalizes defaults; `--validate-config` prints validated JSON.

## Schema

| JSON field | Default | Constraint / current behavior |
| --- | --- | --- |
| `ui.timeout_secs` | `5` | 0..300; reserved for future autoboot |
| `ui.show_snapshots` | `true` | enable snapshot browsing from the BE list |
| `ui.title` | `null` | optional heading; <=256 bytes, no control characters |
| `manager.restart_limit` | `2` | 0..8; rapid failure threshold entering recovery |
| `zfs.import_policy` | `host-id` | host-id or read-only; explicit import only, never force |
| `nixos.generation_limit` | `20` | 1..512; maximum generations per root or snapshot |
| `kernel_args` | `[]` | <=64 strings, <=4096 bytes each, no NUL; additional target arguments |
| `security.mode` | `off` | off/enforce; compiled image mode must agree at runtime |
| `security.require_firmware_secure_boot` | `false` | enforced mode only; enabled firmware must be positively observed |
| `security.target_authorities` | `[]` | <=32 image-owned public certificate paths; nonempty for enforce |
| `security.ima_certificate` | `null` | image-owned public DER certificate path; required for enforce |
| `security.tpm.policy` | `off` | off/optional/required; required TPM2/PCR15 read or prepared-target extend failures stop startup/handoff |

A threshold of 2 restarts on the first rapid failure and enters recovery on the
second. Values 0 and 1 enter recovery on the first failure. Controlled restarts
and explicit shell returns reset the counter; an interval of 30 seconds between
failures resets it. Unknown fields and enum values are errors. JSON uses snake_case
field names and kebab-case enum variants, with defaults for omitted fields/sections.

## Nix integration

Runtime options are under `programs.zbm-rs.settings`: `ui.timeout`,
`ui.showSnapshots`, `ui.title`, `manager.restartLimit`, `zfs.importPolicy`,
`nixos.generationLimit` and `kernelArgs`. Image packaging uses `image.profile`
and `image.hardwareManifest`. The former top-level names remain renamed aliases.
lib.mkImage accepts loaderConfig as a JSON-compatible Nix attrset. It goes through
the same Rust schema validation as checked-in fixtures. Production uses
default.json, the SSH/lifecycle test image uses test.json unless overridden.
Image userspace defaults to eudev and ZFS without optional URL fetching, including
NixOS integration. Local encryption keys work; HTTPS keylocations are unavailable.
NixOS derives this userspace from boot.zfs.package, preserving its version and
matching module selection. Nix generates JSON only, never Rust source. JSON is not read from host /etc or /sys.

A future small kernel-command-line emergency override layer belongs above this
base configuration; it has not been implemented. It must not weaken the signed
security policy of an enforced image. Dynamic pools/Bootspec state
remain separate. Features select code such as vm-test, never ordinary values.

## Secure Boot configuration

The options below configure the enforced loader. See
[the trust model](secure-boot-model.md) for enforcement and key roles, and
[verification](verification.md) for supported targets and acceptance limits. Ordinary policy stays in
Nix-generated, shared-schema-validated immutable JSON; it does not become Cargo features.

All paths below are relative to `programs.zbm-rs`:

| Nix option | Default / requirement | Meaning |
| --- | --- | --- |
| `settings.security.mode` | `off`; `off` or `enforce` | Image policy; `enforce` requires the complete target verification chain. |
| `settings.security.requireFirmwareSecureBoot` | `false` | Also require confirmed enabled firmware Secure Boot before OS handoff. Does not control target verification. |
| `settings.security.targetAuthorities` | Public certificate list; nonempty for `enforce` | Authorities allowed to sign BootAuthorization; copied into the immutable trust store. |
| `image.kernelPolicy` | `validate`; `validate` or `configure` | Validate the selected final loader kernel, or build a separate configured loader-kernel variant. |
| `image.kernelPackages` | Module defaults to `boot.kernelPackages` | Explicit loader kernel/ZFS package set; independent override does not change the host kernel. |
| `image.kernelTrustedCertificates` | Public certificate list, sufficient for selected policy | Required target-kernel/module/IMA certificate trust; validate actual kernel integration. |
| `image.imaCertificate` | Public certificate required for initial `enforce` profile | Authenticated certificate used for target-initramfs appraisal. |
| `settings.security.tpm.policy` | `off`; `optional` or `required` | Read-only TPM2/PCR15 readiness and prepared-target measurement policy; never weakens verified boot. |

All these settings are immutable image inputs, not arbitrary boot-time overrides.
Nix certificate paths are materialized as public trust-store resources; runtime JSON uses packaged
paths/identities and snake_case names such as `security.target_authorities`.
No production private key is a Nix option/path input or stored in that JSON.
TPM policy does not initialize SRK or NvPCRs or extend PCR11 phases. The selected
OS's native initramfs/userspace services own these operations. Former loader
options `settings.security.tpm.nvpcrs`, `settings.security.tpm.requiredNvpcrs`
and `image.pcrPublicKey` are removed; old configurations must be updated and
rebuilt, rather than silently retaining or ignoring those settings.

Example (public certificates and their signing-chain trust must match):

```nix
programs.zbm-rs = {
  enable = true;
  image = {
    kernelPackages = pkgs.linuxPackages;
    kernelPolicy = "configure";
    kernelTrustedCertificates = [
      ./kernel-signing.pem
      ./module-signing.pem
      ./ima-ca.pem
    ];
    imaCertificate = ./ima-signing.pem;
  };
  settings.security = {
    mode = "enforce";
    requireFirmwareSecureBoot = false;
    targetAuthorities = [ ./boot-policy-signing.pem ];
  };
};
```

All example PEM files contain public certificates only. Firmware enrollment and
UKI signing are separate owner deployment operations. An enforced image keeps
verifying targets when firmware Secure Boot is disabled; setting the requirement
to true additionally refuses that handoff. Missing checks, invalid configuration
or unknown firmware state never silently select `off`.

`ima-signing.pem` is a non-CA leaf with digitalSignature usage, issued by the
embedded `ima-ca.pem` trust. The PCR15 measurement helper uses the ordinary Nix
systemd package; no separate v262 setup provider is needed. This helper currently
supports the glibc image profile; TPM-enabled musl packaging is rejected explicitly.
See [the TPM guide](tpm.md#loader-policy) for policy behavior, ownership,
removed-option migration and runtime evidence; see
[owner signing and TPM tests](security-testing.md) for reproduction.

There are no independent `skipInitramfs`, `skipCommandLine` or legacy-kexec fallback
options. `settings.kernelArgs` must fit the signed target authorization. Protected
recovery defaults to diagnostics/restart/reboot/poweroff, including PID-1 failures;
the current emergency shell behavior is not the protected-mode contract. Runtime
permission to boot without a trusted signature is explicitly deferred, not a
current configuration switch.

## Design rationale

The configuration uses typed runtime JSON instead of generated Rust constants.
See [the schema design comparison](research/configuration-design.md) for the
implementation tradeoffs. Complete AArch64 image packaging/boot is not yet
supported by the x86_64 flake/UKI layout.
