# Immutable image configuration

Nix options produce JSON using builtins.toJSON and writeText. The Rust derivation
receives that exact file as ZBM_RS_CONFIG. build.rs deserializes and validates it
on the build host; it generates no Rust and writes no checked-in files. Cargo
tracks both the environment variable and selected file explicitly.

The image contains the same JSON as a mode-0444 Nix-store file, with
/etc/zbm-rs/config.json pointing to it. PID 1 reads/validates it once for the
restart policy, before spawning a manager. Each manager generation reads it once
before Tokio starts. Supervised processes ignore developer ZBM_RS_CONFIG overrides.
There is no configuration writer, watch service or live reload. Root can alter a
RAM-backed initramfs during recovery; file mode is not a security boundary.
Normal configuration changes rebuild the EFI image.

A shared Rust schema and validator live in crates/tui/src/config/schema.rs.
Runtime deserialization uses serde/serde_json, which already belonged to the
runtime dependency graph through telemetry/core/zfskit. Runtime values are ordinary
owned structs, typed enums, String, Vec<String>, Option<String> and fixed-width u32.
No const-gen/databake/codegen dependency remains. Serde derive/proc-macro helpers
run on the build host, not as boot-time code.

## Development

cargo run -p zbm-rs -- --manager --preview

Without an override, local execution uses the checked-in config/default.json
embedded as the development default. To try another JSON file:

ZBM_RS_CONFIG=config/test.json cargo run -p zbm-rs -- --manager --preview

cargo run -p zbm-rs -- --validate-config config/test.json
cargo run -p zbm-rs -- --print-config
uv run --no-project nix/check-config.py

ZBM_RS_CONFIG participates in Cargo validation as well as local runtime loading.
It does not bake alternate values into the ELF: packaging the JSON supplies those
values. --print-config normalizes defaults; --validate-config prints validated JSON.
Missing or invalid explicit files fail instead of silently choosing defaults.

## Schema

| JSON field | Default | Constraint / current behavior |
| --- | --- | --- |
| ui.timeout_secs | 5 | 0..300; reserved for future autoboot |
| ui.show_snapshots | true | reserved for future snapshot UI |
| ui.title | null | optional heading; <=256 bytes, no control characters |
| manager.restart_limit | 2 | 0..8; rapid failure threshold entering recovery |
| zfs.import_policy | host-id | host-id or read-only; reserved, no automatic import |
| nixos.generation_limit | 20 | 1..512; reserved for future NixOS discovery |
| kernel_args | [] | <=64 strings, <=4096 bytes each, no NUL; future target kernel |

A threshold of 2 restarts on the first rapid failure and enters recovery on the
second. Values 0 and 1 enter recovery on the first failure. Controlled restarts
and explicit shell returns reset the counter; an interval of 30 seconds between
failures resets it. Unknown fields and enum values are errors. JSON uses snake_case
field names and kebab-case enum variants, with defaults for omitted fields/sections.

## Nix integration

programs.zbm-rs options: timeout, ui.showSnapshots, ui.title,
manager.restartLimit, zfs.importPolicy, configurationLimit and kernelArgs.
lib.mkImage accepts loaderConfig as a JSON-compatible Nix attrset. It goes through
the same Rust schema validation as checked-in fixtures. Production uses
default.json, the SSH/lifecycle test image uses test.json unless overridden.
Nix generates JSON only, never Rust source. JSON is not read from host /etc or /sys.

A future small kernel-command-line emergency override layer belongs above this
base configuration; it has not been implemented. Dynamic pools/Bootspec state
remain separate. Features select code such as vm-test, never ordinary values.

## Evaluated alternatives

An isolated const-gen 1.6.10 spike passed nested structs, enums, defaults, Option,
primitive values and static slice conversion through include!. Its String emitter
failed escaped quotes/backslashes; a local adapter would be needed. databake 0.2.1
emits String/Vec constructions and does not generate replacement type definitions.
After confirming serde_json already existed in the runtime, the project selected
immutable runtime JSON. No custom derive or proc macro was written.

All configuration integers are fixed-width and JSON has no host byte layout.
AArch64 cargo check and a release cross-build with aarch64-linux-gnu-gcc
exercise the shared schema and runtime; complete AArch64
image packaging/boot is not yet supported by the x86_64 flake/UKI layout.
