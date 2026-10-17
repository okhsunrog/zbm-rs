# Screenshot sources

These PNGs are unmodified 1280×800 QEMU captures made on 2026-10-17. They show
the actual Ratatui interface with the portable snapshot-test VM image,
`security.mode = "off"` and no TPM. Wide screens use 160×50 console cells; the
compact capture uses an 80×25 VT within the same framebuffer. They demonstrate
the interface, not Secure Boot or successful OS-handoff evidence.

| File | View |
| --- | --- |
| [generations.png](generations.png) | Two valid NixOS generations in a real ZFS root. |
| [snapshot.png](snapshot.png) | The same generations in a snapshot, with an owned writable boot clone prepared. |
| [keyboard-help.png](keyboard-help.png) | Keyboard help with bounded scrolling over the generation list. |
| [compact.png](compact.png) | 80×25 snapshot view with the clone explanation and recovery controls. |

## Reproduction

Use the checkout's Nix image builder and disposable NixOS boot fixture. The
snapshot-test image uses writable host-ID-guarded imports, visible snapshots and
no autoboot. It is deliberately labelled `zbm-rs snapshot test` in the header.

The capture session used
`/home/okhsunrog/tmp_zfs/codex_tmp/zbm-tui-review/visual-002`, with the freshly built
snapshot-test image and the existing `target/nixos-fixture-default-regression`
fixture (two distinct real system closures). The ordinary smoke run, including
fault/recovery and keyboard regression checks, is in the sibling `smoke-002`
directory. Both guests use disposable disks, without host disks, keys or TPM.
Build and launch equivalent images with fresh output/run paths:

```sh
export TMPDIR=/home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/tmp
mkdir -p "$TMPDIR"
nix build .#zbm-rs-efi-test-snapshot --out-link /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/image
nix build .#zbm-rs-boot-fixture --out-link /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/fixture
cargo xtask vm --run /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/run boot \
  --image /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/image \
  --fixture /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/fixture --port 2298
```

From another terminal, provision only the VM's disposable disk:

```sh
cargo xtask vm --run /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/run ssh 'set -eu;
  zpool create -o cachefile=none -O compression=lz4 -O mountpoint=none zbm_fixture /dev/vda;
  zfs create -o mountpoint=legacy -o org.zfsbootmenu:active=on zbm_fixture/nixos;
  zpool set bootfs=zbm_fixture/nixos zbm_fixture;
  mkdir -p /fixture-root;
  mount -t zfs zbm_fixture/nixos /fixture-root'

# The large fixture needs a longer deadline than the harness's 30-second SSH
# command limit. These are generated disposable VM credentials.
timeout --kill-after=2 905 ssh -F /dev/null \
  -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
  -o UserKnownHostsFile=/home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/run/known_hosts \
  -i /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/run/id_ed25519 -p 2298 root@127.0.0.1 \
  'timeout 900 tar -xf /dev/vdb -C /fixture-root'

cargo xtask vm --run /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/run ssh 'set -eu;
  zfs snapshot zbm_fixture/nixos@known-good;
  umount /fixture-root;
  zpool export zbm_fixture'
```

Rescan with R, import the pool with Enter and open its boot environment with Enter.
F1 opens help. T opens snapshots, Enter inspects `known-good`, and C prepares the
clone without booting it. Capture each completed view through the harness:

```sh
cargo xtask vm --run /home/okhsunrog/tmp_zfs/codex_tmp/zbm-screenshots/run screenshot /path/to/capture.png
```

The harness pauses QEMU for each capture and resumes the running guest afterward.
For the compact view, set `stty cols 80 rows 25 < /dev/tty1` through the guest
SSH command and wait for the resize before capturing. The capture session also
exercised search, action menus, full details and help scrolling at 60×20. It powered
off through F10 and its foreground QEMU process exited.
No host disks, keys or TPM were attached.
