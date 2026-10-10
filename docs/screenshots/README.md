# Screenshot sources

These PNGs are unmodified 1280×800 QEMU captures made on 2026-10-10. They show the
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

The screenshot session used `target/ui-design-image-v3`,
`target/nixos-fixture-default-regression` and `target/vm/ui-design-final-002`. The
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
  mount -t zfs zbm_fixture/nixos /fixture-root'

# The large fixture needs a longer deadline than the harness's 30-second SSH
# command limit. These are generated disposable VM credentials.
timeout --kill-after=2 905 ssh -F /dev/null \
  -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
  -o UserKnownHostsFile=target/vm/screenshots-new/known_hosts \
  -i target/vm/screenshots-new/id_ed25519 -p 2298 root@127.0.0.1 \
  'timeout 900 tar -xf /dev/vdb -C /fixture-root'

cargo xtask vm --run target/vm/screenshots-new ssh 'set -eu;
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
The capture session also exercised the action menu, full details, discard
confirmation (cancelled) and 80×25 controls. It powered off through P and its
foreground QEMU process exited.
No host disks, keys or TPM were attached.
