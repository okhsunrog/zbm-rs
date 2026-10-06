# zbm-rs development

Read README.md and docs/architecture.md. This is an early Linux/initramfs boot
manager, not a native UEFI ZFS implementation.

- Rust edition 2024. Add dependencies with cargo add; Python uses uv run/uv add.
- crates/core owns boot-domain models and policy; crates/tui renders and handles
  input; xtask owns image and VM lifecycle, QMP, SSH and reusable scenarios.
- One main ELF: PID 1 supervises a fresh current_exe() manager; no separate init binary.
- Nix is the canonical image path. Image configuration is Nix-generated JSON,
  validated by the shared Rust schema on the build host and at process startup.
  Ordinary config values are not Cargo features. No runtime config writer/reloader.
- Use the sibling ../zfskit crate for ZFS operations. Do not duplicate its parsers.
- Discovery is non-mutating. Do not introduce automatic import, force import,
  key loading, mounts, rollback or destructive operations without explicit policy.
- Every new boot feature grows the persistent harness and its fixtures/scenarios.
  Unit tests supplement real boot checks. Screenshots and state events are
  separate evidence; inspect real screenshots when changing TUI behavior.
- VM processes run in a tracked foreground session. Never detach shell jobs.
  Smoke owns its child and reaps it on every return path. Use fresh run folders.
- Do not attach host disks or include host caches, keys, hostid or configuration.
  Test SSH is opt-in, key-only, with host forwarding bound to 127.0.0.1.
- Check cargo fmt --all --check, cargo test --workspace --locked,
  cargo clippy --workspace --all-targets --locked -- -D warnings.
  Boot/image/input changes also need cargo xtask smoke against a fresh run.
- Preserve run artifacts on failure. State what VM checks passed and what remains
  unimplemented; VM boot does not establish physical boot or Secure Boot.
- Never put assistant attribution in commits, PRs, branches or trailers.
  Never post GitHub/GitLab review comments. Do not publish without a request.
