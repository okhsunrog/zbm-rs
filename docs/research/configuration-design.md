# Configuration representation

This records the comparison behind the current immutable JSON schema. The
[configuration guide](../configuration.md) describes the supported API.


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
