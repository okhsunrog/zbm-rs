# Screenshot sources

These PNGs are unmodified 1280×800 QEMU captures made on 2026-10-09. They show the
actual Ratatui interface at 160×50 console cells, using the portable disposable
VM profile with `security.mode = "off"` and no TPM. They demonstrate the interface;
they are not Secure Boot or successful OS-handoff evidence.

| File | View |
| --- | --- |
| [generations.png](generations.png) | Two valid NixOS generations in a real ZFS root. |
| [snapshot.png](snapshot.png) | The same generations in a snapshot, with an owned writable boot clone prepared. |
| [keyboard-help.png](keyboard-help.png) | Keyboard help over the generation list. |

## Reproduction

Use the checkout's Nix image builder and disposable NixOS boot fixture. The demo
uses these image settings in a consuming flake's `lib.mkImage` call:

```nix
{
  testProfile = true;
  loaderConfig = {
    ui = { timeout_secs = 0; show_snapshots = true; title = "zbm-rs"; };
    zfs.import_policy = "host-id";
    nixos.generation_limit = 20;
    kernel_args = [];
  };
}
```

The screenshot session used `target/readme-demo-image`,
`target/nixos-fixture-default-regression` and `target/vm/readme-demo-001`. The
fixture is the ordinary `zbm-rs-boot-fixture` output; it contains two distinct
system closures without the intentionally rejected peers from acceptance tests.
Build and launch equivalent images with fresh output/run paths:

```sh
nix build .#zbm-rs-boot-fixture --out-link result-fixture
cargo xtask vm --run target/vm/screenshots-new boot \
  --image result-demo --fixture result-fixture --port 2298
```

From another terminal, provision only the VM's disposable disk:

```sh
cargo xtask vm --run target/vm/screenshots-new ssh 'set -eu;
  zpool create -o cachefile=none -O compression=lz4 -O mountpoint=none zbm_fixture /dev/vda;
  zfs create -o mountpoint=legacy -o org.zfsbootmenu:active=on zbm_fixture/nixos;
  zpool set bootfs=zbm_fixture/nixos zbm_fixture;
  mkdir -p /fixture-root;
  mount -t zfs zbm_fixture/nixos /fixture-root;
  tar -xf /dev/vdb -C /fixture-root;
  zfs snapshot zbm_fixture/nixos@known-good;
  umount /fixture-root;
  zpool export zbm_fixture'
```

Rescan with R, import the pool with Enter and open its boot environment with Enter.
F1 opens help. T opens snapshots, Enter inspects `known-good`, and C prepares the
clone without booting it. Capture each completed view through the harness:

```sh
cargo xtask vm --run target/vm/screenshots-new screenshot /path/to/capture.png
```

The harness pauses QEMU for each capture and resumes the running guest afterward.
The original session powered off through P and its foreground QEMU process exited.
No host disks, keys or TPM were attached.
