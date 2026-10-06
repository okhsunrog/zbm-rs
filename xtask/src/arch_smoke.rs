//! Installed Arch boot acceptance using the archinstall_zfs staging root.
use crate::{
    smoke::{OwnedVm, manager_pid, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Mode {
    Live,
    Snapshot,
    Rollback,
    Clone,
    Promote,
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Snapshot => "snapshot",
            Self::Rollback => "rollback",
            Self::Clone => "clone",
            Self::Promote => "promote",
        }
    }
}

pub fn run(
    run: &Path,
    image: &Path,
    fixture: &Path,
    tcg: bool,
    port: u16,
    mode: Mode,
) -> Result<()> {
    crate::smoke::install_interrupt()?;
    let provenance: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.join("arch-fixture.json"))?)?;
    ensure!(
        provenance["kernel"] == "6.18.53-1-lts" && provenance["pkgbase"] == "linux-lts",
        "Unsupported Arch fixture kernel"
    );
    let mut command = Command::new("/proc/self/exe");
    command
        .args(["vm", "--run"])
        .arg(run)
        .args(["boot", "--port", &port.to_string(), "--image"])
        .arg(image)
        .arg("--fixture")
        .arg(fixture);
    if tcg {
        command.arg("--tcg");
    }
    let mut owned = OwnedVm(command.stdout(Stdio::null()).spawn()?);
    wait_for(90, Some(&mut owned.0), || {
        ensure!(vm::screen(run)?["event"] == "ready", "Waiting for loader");
        vm::ssh(run, "test -b /dev/vdb")?;
        Ok(())
    })?;
    println!("Deploying sanitized archinstall_zfs staging root onto disposable /dev/vda");
    vm::ssh(
        run,
        "set -eu; zpool create -o cachefile=none -O compression=lz4 -O mountpoint=none zbm_arch /dev/vda; zfs create -o mountpoint=none -o canmount=off zbm_arch/arch0; zfs create -o mountpoint=/ -o canmount=noauto -o org.zfsbootmenu:commandline='spl.spl_hostid=0x00bab10c zswap.enabled=0 rw' -o org.zfsbootmenu:rootprefix=zfs= zbm_arch/arch0/root; zpool set bootfs=zbm_arch/arch0/root zbm_arch; mkdir -p /arch-root; mount -t zfs zbm_arch/arch0/root /arch-root",
    )?;
    vm::ssh_timeout(run, "tar -xf /dev/vdb -C /arch-root", 900)?;
    // Mirror the repository's deploy-zfs-be.sh mkinitcpio path. Host identity,
    // caches and keys were excluded from the archive; the hostid is synthetic.
    vm::ssh(
        run,
        "set -eu; mkdir -p /arch-root/boot /arch-root/etc/zfs /arch-root/dev /arch-root/proc /arch-root/sys /arch-root/run /arch-root/tmp /arch-root/root; cp /arch-root/usr/lib/modules/6.18.53-1-lts/vmlinuz /arch-root/boot/vmlinuz-linux-lts; printf '\\014\\261\\272\\000' > /arch-root/etc/hostid; printf 'zbm_arch/arch0/root / zfs defaults 0 0\\n' > /arch-root/etc/fstab; rm -f /arch-root/etc/mkinitcpio.conf.d/archiso.conf; printf 'FILES=(/etc/hostid)\\nHOOKS=(base udev autodetect microcode modconf kms keyboard keymap consolefont block zfs filesystems)\\nCOMPRESSION=zstd\\nCOMPRESSION_OPTIONS=(-T1 -3)\\n' > /arch-root/etc/mkinitcpio.conf.d/zfs-root.conf; mount --bind /dev /arch-root/dev; mount --bind /proc /arch-root/proc; mount --bind /sys /arch-root/sys",
    )?;
    let build = vm::ssh_timeout(
        run,
        "chroot /arch-root /usr/bin/mkinitcpio -k 6.18.53-1-lts -g /boot/initramfs-linux-lts.img",
        180,
    )?;
    fs::write(run.join("arch-initramfs-build.log"), build)?;
    install_proof(run, mode)?;
    let inputs = vm::ssh(
        run,
        "set -eu; chroot /arch-root /usr/bin/lsinitcpio /boot/initramfs-linux-lts.img | grep -E 'hooks/zfs|zfs.ko|etc/hostid'; cat /arch-root/usr/lib/os-release; ls -l /arch-root/boot; test ! -e /arch-root/nix/var/nix/profiles/system; echo snapshot-original > /arch-root/arch-proof; zfs snapshot zbm_arch/arch0/root@known-good; echo live-changed > /arch-root/arch-proof; zfs snapshot zbm_arch/arch0/root@newer; umount /arch-root/sys; umount /arch-root/proc; umount /arch-root/dev; umount /arch-root; zpool export zbm_arch",
    )?;
    ensure!(
        inputs.contains("hooks/zfs") && inputs.contains("zfs.ko"),
        "Missing ZFS-root initramfs inputs"
    );
    fs::write(run.join("arch-inputs.txt"), inputs)?;
    let original_boot = vm::ssh(run, "cat /proc/sys/kernel/random/boot_id")?;
    vm::key(run, "r")?;
    event(run, "ready")?;
    vm::key(run, "ret")?;
    let environments = event(run, "boot-environments")?;
    let be = &environments["state"]["environments"][0];
    ensure!(
        be["dataset"] == "zbm_arch/arch0/root"
            && be["unavailable"].is_null()
            && be["targets"].as_array().is_some_and(|t| t.len() == 1),
        "Missing Arch kernel pair: {environments}"
    );
    ensure!(
        be["targets"][0]["generation"].is_null(),
        "Linux target must not invent a NixOS generation"
    );
    vm::screenshot(run, &run.join("arch-environment.png"))?;
    if !matches!(mode, Mode::Live) {
        vm::key(run, "t")?;
        let snapshots = event(run, "snapshots")?;
        let selected = snapshots["state"]["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .position(|s| s["source"]["name"] == "zbm_arch/arch0/root@known-good")
            .unwrap();
        vm::key(run, "home")?;
        for _ in 0..selected {
            vm::key(run, "down")?;
        }
        wait_for(5, None, || {
            ensure!(
                vm::screen(run)?["state"]["selected_snapshot"].as_u64() == Some(selected as u64),
                "Waiting for known-good snapshot"
            );
            Ok(())
        })?;
        vm::screenshot(run, &run.join("arch-snapshot-selected.png"))?;
        match mode {
            Mode::Rollback => rollback_checks(run)?,
            Mode::Clone | Mode::Promote => clone_checks(run, mode)?,
            Mode::Snapshot => {
                vm::key(run, "ret")?;
                event(run, "boot-targets")?;
            }
            Mode::Live => unreachable!(),
        }
    }
    if !matches!(mode, Mode::Snapshot) {
        vm::key(run, "ret")?;
        event(run, "boot-targets")?;
    }
    vm::screenshot(run, &run.join("arch-kernel.png"))?;
    let pid = manager_pid(run)?;
    vm::key(run, "f3")?;
    wait_for(5, None, || {
        ensure!(
            vm::screen(run)?["ui"]["panel"] == "Details",
            "Waiting for Arch details"
        );
        Ok(())
    })?;
    let details = vm::console(run)?;
    ensure!(
        details.contains("vmlinuz-linux-lts") && details.contains("initramfs-linux-lts.img"),
        "Arch details lost inputs"
    );
    vm::screenshot(run, &run.join("arch-details.png"))?;
    vm::key(run, "esc")?;
    ensure!(manager_pid(run)? == pid, "Inspection replaced manager");
    println!("Booting Arch mode={} via kexec_file_load", mode.name());
    vm::key(run, "ret")?;
    let proof = wait_for(180, None, || {
        let log = fs::read_to_string(run.join("serial.log"))?;
        let line = log
            .lines()
            .find_map(|line| line.find("ZBM_ARCH_SUCCESS ").map(|start| &line[start..]))
            .ok_or_else(|| anyhow::anyhow!("Waiting for Arch boot proof"))?;
        ensure!(
            line.contains(&format!("mode={}", mode.name()))
                && line.contains("kernel=6.18.53-1-lts")
                && !line.contains(original_boot.trim()),
            "Incorrect target boot: {line}"
        );
        Ok(line.to_owned())
    })?;
    wait_for(30, None, || {
        ensure!(
            owned.0.try_wait()?.is_some_and(|s| s.success()),
            "Waiting for Arch target poweroff"
        );
        Ok(())
    })?;
    fs::write(
        run.join("arch-report.json"),
        serde_json::to_vec_pretty(
            &json!({ "passed": true, "mode":mode.name(), "arch_boot_supported":true, "arch_boot_attempted":true, "proof":proof, "fixture":provenance, "checks":["real-Arch-root-and-ZFS-initramfs", "ZBM-kernel-pair", "no-NixOS-generation-required", "parent-KCL-expansion", "conflicting-root-arguments-suppressed", "real-keyboard-and-details", "exact-root-and-state", "kexec_file_load", "multi-user", "guest-poweroff"] }),
        )?,
    )?;
    println!("Arch {} boot acceptance passed", mode.name());
    Ok(())
}

fn event(run: &Path, expected: &str) -> Result<serde_json::Value> {
    wait_for(30, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == expected,
            "Waiting for {expected}: {state}"
        );
        Ok(state)
    })
}

fn rollback_checks(run: &Path) -> Result<()> {
    // Dependent newer clones and read-only policy must block rollback without
    // altering source data. Recovery deliberately uses the actual manager keys.
    vm::ssh(
        run,
        "zfs clone -o mountpoint=none -o canmount=noauto zbm_arch/arch0/root@newer zbm_arch/rollback-blocker",
    )?;
    confirmed_rollback(run)?;
    let failure = event(run, "rollback-error")?;
    ensure!(
        failure["state"]["requires_restart"] == true,
        "Rollback failure must invalidate inspection state"
    );
    vm::ssh(
        run,
        "set -eu; zfs list -H zbm_arch/rollback-blocker; zfs list -H zbm_arch/arch0/root@newer; test \"$(cat /sys/kernel/kexec_loaded)\" = 0; zfs destroy zbm_arch/rollback-blocker; zpool set org.zfsbootmenu:readonly=on zbm_arch",
    )?;
    recover_snapshot(run)?;
    confirmed_rollback(run)?;
    let failure = event(run, "rollback-error")?;
    ensure!(
        failure["state"]["error"]
            .as_str()
            .is_some_and(|s| s.contains("readonly")),
        "Missing read-only policy error"
    );
    vm::ssh(
        run,
        "set -eu; zfs list -H zbm_arch/arch0/root@newer; zpool set org.zfsbootmenu:readonly=off zbm_arch",
    )?;
    recover_snapshot(run)?;
    vm::key(run, "u")?;
    event(run, "rollback-confirmation")?;
    vm::screenshot(run, &run.join("rollback-confirmation.png"))?;
    vm::key(run, "ret")?;
    ensure!(
        vm::screen(run)?["ui"]["panel"] == "ConfirmRollback",
        "Bare Enter confirmed destructive rollback"
    );
    vm::key(run, "esc")?;
    vm::ssh(run, "zfs list -H zbm_arch/arch0/root@newer")?;
    vm::key(run, "u")?;
    event(run, "rollback-confirmation")?;
    for key in [
        "shift-r", "shift-o", "shift-l", "shift-l", "shift-b", "shift-a", "shift-c", "shift-k",
    ] {
        vm::key(run, key)?;
    }
    vm::key(run, "alt-ret")?;
    ensure!(
        vm::screen(run)?["ui"]["panel"] == "ConfirmRollback",
        "Modified Enter confirmed rollback"
    );
    vm::key(run, "ret")?;
    event(run, "environment-changed")?;
    vm::ssh(
        run,
        "set -eu; ! zfs list -H zbm_arch/arch0/root@newer; zfs list -H zbm_arch/arch0/root@known-good; test \"$(cat /run/zbm-rs/roots/*/*/arch-proof)\" = snapshot-original",
    )?;
    vm::screenshot(run, &run.join("rollback-complete.png"))?;
    Ok(())
}

fn confirmed_rollback(run: &Path) -> Result<()> {
    vm::key(run, "u")?;
    event(run, "rollback-confirmation")?;
    for key in [
        "shift-r", "shift-o", "shift-l", "shift-l", "shift-b", "shift-a", "shift-c", "shift-k",
    ] {
        vm::key(run, key)?;
    }
    vm::key(run, "ret")?;
    Ok(())
}

fn recover_snapshot(run: &Path) -> Result<()> {
    let old = manager_pid(run)?;
    vm::key(run, "n")?;
    wait_for(20, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "ready" && manager_pid(run)? != old,
            "Waiting for new manager"
        );
        Ok(())
    })?;
    vm::key(run, "ret")?;
    event(run, "boot-environments")?;
    vm::key(run, "t")?;
    let state = event(run, "snapshots")?;
    let selected = state["state"]["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["source"]["name"] == "zbm_arch/arch0/root@known-good")
        .unwrap();
    vm::key(run, "home")?;
    for _ in 0..selected {
        vm::key(run, "down")?;
    }
    Ok(())
}

fn clone_checks(run: &Path, mode: Mode) -> Result<()> {
    if matches!(mode, Mode::Promote) {
        vm::key(run, "m")?;
        event(run, "promote-confirmation")?;
        vm::screenshot(run, &run.join("promote-confirmation.png"))?;
        vm::key(run, "ret")?;
    } else {
        vm::key(run, "o")?;
    }
    let state = event(run, "environment-changed")?;
    let index = state["state"]["selected_environment"].as_u64().unwrap() as usize;
    let dataset = state["state"]["environments"][index]["dataset"]
        .as_str()
        .unwrap();
    ensure!(
        dataset.starts_with("zbm_arch/arch0/root-recovery-"),
        "Persistent clone was not selected"
    );
    let origin = vm::ssh(run, &format!("zfs get -H -o value origin {dataset}"))?;
    ensure!(
        origin.trim()
            == if matches!(mode, Mode::Promote) {
                "-"
            } else {
                "zbm_arch/arch0/root@known-good"
            },
        "Wrong clone/promote origin"
    );
    vm::screenshot(run, &run.join("persistent-clone.png"))?;
    Ok(())
}

fn install_proof(run: &Path, mode: Mode) -> Result<()> {
    vm::ssh(
        run,
        &format!(
            "set -eu; zfs set org.zfsbootmenu:commandline='spl.spl_hostid=0x00bab10c zswap.enabled=0 rw console=tty0 console=ttyS0,115200' zbm_arch/arch0; zfs set org.zfsbootmenu:commandline='%{{parent}} zbm.arch-mode={}' zbm_arch/arch0/root",
            mode.name()
        ),
    )?;
    if matches!(mode, Mode::Live) {
        vm::ssh(
            run,
            "zfs inherit org.zfsbootmenu:rootprefix zbm_arch/arch0/root",
        )?;
    }
    vm::ssh(
        run,
        r###"set -eu
mkdir -p /arch-root/etc/systemd/system/multi-user.target.wants /arch-root/usr/local/bin
ln -sfn /dev/null /arch-root/etc/systemd/system/zfs-mount.service
ln -sfn /usr/lib/systemd/system/multi-user.target /arch-root/etc/systemd/system/default.target
cat > /arch-root/boot/vmlinuz-linux-lts.kcl <<'KCL'
%{parent} root=must-be-replaced zfs=must-be-replaced
KCL
cat > /arch-root/etc/systemd/system/zbm-arch-proof.service <<'UNIT'
[Unit]
Description=Disposable Arch boot acceptance
After=multi-user.target
DefaultDependencies=no
[Service]
Type=oneshot
ExecStart=/usr/local/bin/zbm-arch-proof
[Install]
WantedBy=multi-user.target
UNIT
ln -sfn ../zbm-arch-proof.service /arch-root/etc/systemd/system/multi-user.target.wants/zbm-arch-proof.service
cat > /arch-root/usr/local/bin/zbm-arch-proof <<'PROOF'
#!/bin/bash
set -eu
exec > /dev/ttyS0 2>&1
trap 'echo ZBM_ARCH_FAILURE; cat /proc/cmdline; findmnt /; systemctl --failed --no-pager' ERR
systemctl is-active --quiet multi-user.target
test "$(uname -r)" = 6.18.53-1-lts
test "$(findmnt -n -o FSTYPE /)" = zfs
root="$(findmnt -n -o SOURCE /)"
mode=""
for argument in $(cat /proc/cmdline); do case "$argument" in zbm.arch-mode=*) mode="${argument#*=}";; esac; done
case "$mode" in
 live) test "$root" = zbm_arch/arch0/root; test "$(cat /arch-proof)" = live-changed;;
 rollback) test "$root" = zbm_arch/arch0/root; test "$(cat /arch-proof)" = snapshot-original; ! zfs list -H zbm_arch/arch0/root@newer;;
 snapshot|clone|promote)
   case "$mode:$root" in snapshot:zbm_arch/zbm-rs-*|clone:zbm_arch/arch0/root-recovery-*|promote:zbm_arch/arch0/root-recovery-*) ;; *) exit 1;; esac
   test "$(cat /arch-proof)" = snapshot-original
   echo clone-write > /arch-proof
   test "$(cat /arch-proof)" = clone-write
   mkdir -p /run/source-root
   mount -t zfs -o ro,zfsutil zbm_arch/arch0/root /run/source-root
   test "$(cat /run/source-root/arch-proof)" = live-changed
   umount /run/source-root
   ;;
 *) exit 1;;
esac
echo "ZBM_ARCH_SUCCESS mode=$mode kernel=$(uname -r) root=$root boot_id=$(cat /proc/sys/kernel/random/boot_id)"
systemctl poweroff
PROOF
chmod 755 /arch-root/usr/local/bin/zbm-arch-proof
"###,
    )?;
    Ok(())
}
