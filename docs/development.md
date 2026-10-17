# Development and the VM harness

The workspace contains the boot-domain library (`crates/core`), the executable
and terminal interface (`crates/tui`) and image/VM tooling (`xtask`). Runtime and
ownership boundaries are described in [the architecture](architecture.md).

## Local checks and preview

Enter `nix develop` for the pinned Rust toolchain and native dependencies,
including OpenSSL and pkg-config. This keeps C compilation and dependency
discovery within the same Nix environment on non-NixOS hosts.

```sh
cargo fmt --all --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo run -p zbm-rs -- --manager --preview
cargo xtask smoke --run target/vm/smoke-001
cargo xtask smoke --run target/vm/lifecycle-001 --lifecycle
```

Build `result-test` first using [the test image profile](build.md#efi-images). Every run uses a fresh folder and its own disposable disk.
Smoke owns and reaps QEMU even on failure/interruption. Add `--failures` to
exercise isolated mount/Bootspec failures, Shell during a blocked mount,
operation timeout/restart reconciliation, and Power off during a blocked mount.
The lifecycle variant
also checks abort/panic/SIGSEGV, actual terminal damage/restoration, manager
restart, PID-1 signal forwarding, adopted-child reaping, CLOEXEC, deliberately
leaked descriptors and failed-kexec classification. The separate `boot-smoke`
scenario proves successful handoff into the selected installed generation.

The explicit host-only SCSI fixture exercises the disk frontend omitted by a
controller-only hardware manifest:

```sh
nix build .#zbm-rs-efi-test-host-only-scsi --out-link result-scsi
cargo xtask smoke --image result-scsi --run target/vm/scsi-new --port 2231 --scsi
```

Interactive boot stays in a tracked foreground terminal/session. Other controls
run from another terminal:

```sh
cargo xtask vm --run target/vm/dev-001 boot
cargo xtask vm --run target/vm/dev-001 status
cargo xtask vm --run target/vm/dev-001 screen
cargo xtask vm --run target/vm/dev-001 screen --text
cargo xtask vm --run target/vm/dev-001 key r
cargo xtask vm --run target/vm/dev-001 screenshot target/vm/dev-001/screen.png
cargo xtask vm --run target/vm/dev-001 logs
cargo xtask vm --run target/vm/dev-001 ssh 'cat /run/zbm-rs/supervisor.log'
cargo xtask vm --run target/vm/dev-001 stop
```

Boot defaults to KVM/OVMF, with matched firmware paths on Arch/Debian/Ubuntu.
Use --tcg without KVM, --direct to skip EFI during Linux iteration, --image to
select another Nix result, and --port to change the loopback SSH port (2228).
SSH commands have a 30-second guest and 35-second host deadline.

## Installed-OS boot acceptance

The NixOS fixture contains two real generations. The harness installs their
closures from a read-only fixture disk onto a fresh disposable ZFS disk, selects
an older generation through the real keyboard and requires its exact system
closure, root dataset and a new boot ID before clean poweroff.

```sh
nix build .#zbm-rs-efi-test --out-link result-test
nix build .#zbm-rs-boot-fixture --out-link result-fixture
cargo xtask boot-smoke --image result-test --fixture result-fixture \
  --run target/vm/nixos-new --tcg
```

Snapshot acceptance needs a writable-import test profile. It verifies the clone
root and preserves the source dataset and snapshot:

```sh
nix build .#zbm-rs-efi-test-snapshot --out-link result-snapshot
cargo xtask boot-smoke --snapshot --image result-snapshot --fixture result-fixture \
  --run target/vm/snapshot-new
```

The Arch fixture uses a sanitized mkarchiso staging tree, a real ZFS-root
mkinitcpio image and a disposable disk. Build it from the matching
[archinstall_zfs](https://github.com/okhsunrog/archinstall_zfs) output:

```sh
uv run --no-project python xtask/fixtures/build_arch.py \
  /path/to/mkarchiso/workdir/x86_64/airootfs target/arch-fixture-new --sudo
cargo xtask arch-smoke --image result-snapshot --fixture target/arch-fixture-new \
  --run target/vm/arch-live-new --mode live
```

Use fresh run directories with `--mode snapshot`, `rollback`, `clone` or `promote`
for the other Arch paths. This tests a staged boot environment, rather than the
installer wizard. Rollback checks cancellation, modified Enter, dependent-clone
protection and read-only policy before restoring data and booting the result.

Protected acceptance uses [the separate signing workflow](security-testing.md).

## GUI prototype review

The `gui-prototype` feature selects the Slint interface with Skia software rendering.
It is a sample-data preview: it does not discover pools, verify boot inputs, boot
an entry or open recovery tools. The ordinary Nix image continues to use the TUI.

Build the prototype on the development host:

```sh
cargo build -p zbm-rs --features gui-prototype --locked
cargo test -p zbm-rs --features gui-prototype gui::tests --locked
```

Copy the binary into a disposable LinuxKMS VM and run it on a guest VT, as an
ordinary process rather than PID 1. Inside that VM:

```sh
SLINT_BACKEND=linuxkms-skia-software ZBM_GUI_FIXTURE=demo ./zbm-rs
```

`ZBM_GUI_FIXTURE` accepts `demo`, `empty`, `long` and `many` (60 sample generations).
An unknown fixture is rejected. Use the boot-source selector to test empty filtered
results as well. Selected entries retain their identity when a filter changes their
position; changing the selection or source clears the previous action result.

Tab cycles through the source selector, list, details and action button. In the
list, arrows or j/k select entries, Home/End jump to the ends, Page Up/Down move
by a viewport, and Enter invokes the same preview action as the button. The details
pane supports arrows and Page Up/Down for scrolling. Mouse selection returns
keyboard focus to the list. Below 720 logical pixels, the Entries/Details button
switches the main pane; the action area stays visible.

Review at 1920×1080 (100%, 150%, 200%), 1024×768, 800×600 and a narrow logical
viewport. Inspect full-size screenshots and exercise filtering, keyboard/button
activation, focus after mouse selection, stale-status clearing, long text, empty
results and scrolling to the last of many entries. A successful unit test does not
establish layout or keyboard behavior. The pinned LinuxKMS version does not deliver
Shift+Tab as a reverse-focus key in the VM; use forward Tab to cycle controls.

## Observing a VM

`screen` returns instrumented application state. `screen --text` reads the actual
virtual terminal through `/dev/vcs1`; PNG capture is a separate observation.
The screenshot command pauses a running guest while capturing stable frames and
then resumes it. Preserve logs and captures from failed runs for diagnosis.

## Working conventions

- Use Rust 2024 and add Rust dependencies with `cargo add`.
- Run Python tooling with `uv run`; manage its dependencies with `uv add`.
- Keep boot-domain models and policy in core, input/rendering in tui, and VM
  lifecycle/control in xtask. Reuse the published zfskit crate for ZFS operations.
- Validate new boot behavior in the persistent harness with disposable guest
  disks and explicit fixture inputs. Host disks, keys, caches and hostid stay out.
- Run QEMU as a tracked foreground process. Reap owned QEMU/swtpm children on
  success, interruption and failure; use a fresh directory for each scenario.
- Discovery reads state. Import, mount, key loading and destructive operations
  require an explicit action and policy.

The ordinary CI workflow builds Nix images and runs unsigned lifecycle, SCSI,
NixOS-root and snapshot scenarios. Protected-image tests use the separate
[owner signing and Secure Boot workflow](security-testing.md).

[Repository instructions](AGENTS.md) record the detailed maintenance rules.
