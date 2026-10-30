//! Real loader UI with encrypted ZFS and LUKS, using disposable image signatures.
//! Real RAM restoration is a separate NixOS VM check.
use crate::{
    smoke::{OwnedVm, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

fn state(run: &Path, wanted: &str) -> Result<Value> {
    let screen = vm::screen(run)?;
    ensure!(
        screen["state"]["resume"]["state"] == wanted,
        "Waiting for resume {wanted}: {screen}"
    );
    Ok(screen)
}
fn password(run: &Path, text: &str) -> Result<()> {
    for byte in text.bytes() {
        let key = if byte == b'-' {
            "minus".into()
        } else {
            (byte as char).to_string()
        };
        vm::key(run, &key)?;
    }
    vm::key(run, "ret")
}
fn prompt(run: &Path) -> Result<()> {
    ensure!(
        vm::screen(run)?["ui"]["panel"] == "Unlock",
        "Waiting for shared credential prompt"
    );
    Ok(())
}
fn seed_image(run: &Path, target: &Value, guid: &str) -> Result<()> {
    let kernel = format!(
        "{}{}",
        target["root"].as_str().unwrap(),
        target["inputs"]["kernel"].as_str().unwrap()
    );
    let plan = json!({"target":target, "kernel":kernel, "initrd":null, "cmdline":["root=ZFS=zbm_resume/root"]});
    vm::write_file(run, "/run/resume-plan.json", &serde_json::to_vec(&plan)?)?;
    vm::ssh(
        run,
        &format!(
            "zbm-rs --resume-record-config /etc/zbm-rs/config.json {guid} /run/resume-plan.json; printf preserved-payload | dd of=/dev/mapper/resume-swap bs=1 seek=4096 conv=notrunc 2>/dev/null; printf 'SWAPSPACE2S1SUSPEND\\000' | dd of=/dev/mapper/resume-swap bs=1 seek=4076 conv=notrunc 2>/dev/null; sync"
        ),
    )?;
    Ok(())
}

pub fn run(run: &Path, image: &Path, port: u16, tcg: bool) -> Result<()> {
    crate::smoke::install_interrupt()?;
    let mut command = Command::new("/proc/self/exe");
    command
        .args(["vm", "--run"])
        .arg(run)
        .args(["boot", "--port", &port.to_string(), "--image"])
        .arg(image);
    if tcg {
        command.arg("--tcg");
    }
    let mut owned = OwnedVm(command.stdout(Stdio::null()).spawn()?);
    let result = (|| {
        wait_for(90, Some(&mut owned.0), || {
            state(run, "blocked")?;
            vm::ssh(run, "test -b /dev/vda")?;
            Ok(())
        })?;
        vm::ssh(
            run,
            r#"set -eu
sgdisk --zap-all --new=1:2048:+512M --partition-guid=1:12345678-1234-1234-1234-123456789abc --new=2:0:0 /dev/vda
udevadm settle
printf source-passphrase > /run/source-key
printf different-swap-key > /run/swap-key
cryptsetup luksFormat --batch-mode --type luks2 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 --uuid ca55a30a-636e-4e69-9e68-bb36bf380408 --key-file /run/swap-key /dev/vda1
cryptsetup open --key-file /run/swap-key /dev/vda1 resume-swap
mkswap /dev/mapper/resume-swap
cryptsetup close resume-swap
zpool create -o cachefile=none -O mountpoint=none -O encryption=aes-256-gcm -O keyformat=passphrase -O keylocation=file:///run/source-key zbm_resume /dev/vda2
zfs set keylocation=prompt org.zfsbootmenu:active=off zbm_resume
zfs create -o mountpoint=legacy -o canmount=noauto -o org.zfsbootmenu:active=on zbm_resume/root
zpool set bootfs=zbm_resume/root zbm_resume
mkdir /fixture
mount -t zfs zbm_resume/root /fixture
mkdir -p /fixture/nix/var/nix/profiles /fixture/nix/store/shared /fixture/nix/store/system1 /fixture/nix/store/system2
printf kernel-fixture > /fixture/nix/store/shared/kernel
touch /fixture/nix/store/shared/initrd /fixture/nix/store/system1/init /fixture/nix/store/system2/init
"#,
        )?;
        for generation in [1, 2] {
            let spec = json!({"org.nixos.bootspec.v1": {"system":"x86_64-linux", "kernel":"/nix/store/shared/kernel", "initrd":"/nix/store/shared/initrd", "init":format!("/nix/store/system{generation}/init"), "toplevel":format!("/nix/store/system{generation}"), "kernelParams":[], "label":format!("resume generation {generation}")}});
            vm::write_file(
                run,
                &format!("/fixture/nix/store/system{generation}/boot.json"),
                &serde_json::to_vec(&spec)?,
            )?;
            vm::ssh(
                run,
                &format!(
                    "ln -s /nix/store/system{generation} /fixture/nix/var/nix/profiles/system-{generation}-link"
                ),
            )?;
        }
        vm::ssh(
            run,
            "ln -s system-2-link /fixture/nix/var/nix/profiles/system; umount /fixture; zpool export zbm_resume",
        )?;
        let guid = vm::ssh(run, "zpool import | awk '/id:/ { print $2 }'")?
            .trim()
            .to_owned();
        let mut config: Value =
            serde_json::from_str(&vm::ssh(run, "cat /etc/zbm-rs/config.json")?)?;
        config["resume"]["credential"]["pool_guid"] = guid.clone().into();
        vm::ssh(
            run,
            "rm /etc/zbm-rs/config.json; cryptsetup open --key-file /run/swap-key /dev/vda1 resume-swap",
        )?;
        vm::write_file(
            run,
            "/etc/zbm-rs/config.json",
            &serde_json::to_vec(&config)?,
        )?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            let s = state(run, "blocked")?;
            ensure!(
                s["state"]["resume"]["reason"]
                    .as_str()
                    .is_some_and(|s| s.contains("outside this loader")),
                "Foreign mapping was not rejected"
            );
            Ok(())
        })?;
        vm::ssh(run, "cryptsetup close resume-swap")?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            state(run, "locked")?;
            prompt(run)
        })?;
        vm::ssh(
            run,
            "test $(zpool get -H -o value readonly zbm_resume) = on; ! grep -q ' - zfs ' /proc/self/mountinfo",
        )?;
        vm::screenshot(run, &run.join("shared-unlock.png"))?;
        password(run, "wrong-passphrase")?;
        wait_for(25, None, || {
            ensure!(
                vm::screen(run)?["event"] == "unlock-error",
                "Waiting for rejected ZFS password"
            );
            prompt(run)
        })?;
        password(run, "source-passphrase")?;
        wait_for(25, None, || {
            let s = state(run, "locked")?;
            ensure!(
                s["event"] == "unlock-error"
                    && s["state"]["error"]
                        .as_str()
                        .is_some_and(|e| e.contains("LUKS2 swap")),
                "Expected separate LUKS failure"
            );
            prompt(run)
        })?;
        vm::ssh(
            run,
            "test $(zfs get -H -o value keystatus zbm_resume) = available; test ! -e /dev/mapper/resume-swap; ! grep -q ' - zfs ' /proc/self/mountinfo; cryptsetup luksAddKey --pbkdf pbkdf2 --pbkdf-force-iterations 1000 --key-file /run/swap-key /dev/vda1 /run/source-key",
        )?;
        password(run, "source-passphrase")?;
        let cold = wait_for(25, None, || {
            let s = state(run, "inactive")?;
            ensure!(
                s["event"] == "catalog-ready" && s["state"]["error"].is_null(),
                "Waiting for cold-boot catalog"
            );
            Ok(s)
        })?;
        let target = cold["state"]["environments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["dataset"] == "zbm_resume/root")
            .unwrap()["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["generation"] == 1)
            .unwrap()
            .clone();
        seed_image(run, &target, &guid)?;
        vm::key(run, "f6")?;
        let pending = wait_for(25, None, || {
            let s = state(run, "pending")?;
            ensure!(s["event"] == "catalog-ready", "Waiting for pending catalog");
            Ok(s)
        })?;
        ensure!(
            pending["state"]["resume"]["session"]["target"]["generation"] == 1,
            "Wrong default generation"
        );
        vm::ssh(
            run,
            "test $(zpool get -H -o value readonly zbm_resume) = on",
        )?;
        vm::screenshot(run, &run.join("resume-pending.png"))?;
        vm::key(run, "k")?;
        wait_for(15, None, || {
            ensure!(
                vm::screen(run)?["ui"]["view"] == "Generations",
                "Waiting for generations"
            );
            Ok(())
        })?;
        vm::key(run, "up")?;
        vm::key(run, "ret")?;
        wait_for(10, None, || {
            ensure!(
                vm::screen(run)?["ui"]["panel"] == "ConfirmResume",
                "Expected cancellation for another generation sharing the kernel"
            );
            Ok(())
        })?;
        vm::screenshot(run, &run.join("cancel-confirmation.png"))?;
        vm::key(run, "esc")?;
        vm::key(run, "esc")?;
        vm::key(run, "s")?;
        wait_for(10, None, || {
            ensure!(
                vm::screen(run)?["ui"]["panel"] == "ConfirmResume",
                "Expected cancellation before shell"
            );
            Ok(())
        })?;
        vm::key(run, "ret")?;
        wait_for(15, None, || {
            ensure!(
                vm::console(run)?.trim_end().ends_with("~ #"),
                "Waiting for shell after verified cancellation"
            );
            Ok(())
        })?;
        vm::ssh(
            run,
            "test $(dd if=/dev/mapper/resume-swap bs=1 skip=4086 count=10 2>/dev/null) = SWAPSPACE2; test $(dd if=/dev/mapper/resume-swap bs=1 skip=4096 count=17 2>/dev/null) = preserved-payload",
        )?;
        for key in ["e", "x", "i", "t", "ret"] {
            vm::key(run, key)?;
        }
        wait_for(25, None, || {
            let s = state(run, "inactive")?;
            ensure!(s["event"] == "catalog-ready", "Waiting for shell return");
            Ok(())
        })?;
        seed_image(run, &target, &guid)?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            let s = state(run, "pending")?;
            ensure!(s["event"] == "catalog-ready", "Waiting for second image");
            Ok(())
        })?;
        vm::key(run, "s")?;
        wait_for(10, None, || {
            ensure!(
                vm::screen(run)?["ui"]["panel"] == "ConfirmResume",
                "Waiting for power-off test dialog"
            );
            Ok(())
        })?;
        let before = vm::ssh_timeout(run, "sha256sum /dev/vda", 60)?
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        vm::key(run, "f10")?;
        let mut exit = None;
        wait_for(20, None, || {
            exit = owned.0.try_wait()?;
            ensure!(exit.is_some(), "Waiting for immediate power off");
            Ok(())
        })?;
        ensure!(exit.unwrap().success(), "Power off failed");
        let raw = run.join("disk-after.raw");
        ensure!(
            Command::new("qemu-img")
                .args(["convert", "-O", "raw"])
                .arg(run.join("disk.qcow2"))
                .arg(&raw)
                .status()?
                .success(),
            "Cannot inspect stopped VM disk"
        );
        let hash = Command::new("sha256sum").arg(&raw).output()?;
        ensure!(
            hash.status.success()
                && String::from_utf8_lossy(&hash.stdout)
                    .split_whitespace()
                    .next()
                    == Some(before.as_str()),
            "Power off changed the saved image or pool"
        );
        let serial = fs::read_to_string(run.join("serial.log"))?;
        ensure!(
            !serial.contains("source-passphrase") && !serial.contains("different-swap-key"),
            "Secret leaked into diagnostics"
        );
        fs::write(
            run.join("report.json"),
            serde_json::to_vec_pretty(
                &json!({"passed":true,"checks":["foreign-mapping","metadata-only-before-unlock","wrong-ZFS-password","different-LUKS-password","shared-credential","manager-restart","pending-default","other-generation-confirmation","Esc-preserves-image","verified-cancel-before-shell","payload-preserved","F10-preserves-entire-disk","no-secret-telemetry"],"real_ram_restore":false}),
            )?,
        )?;
        Ok(())
    })();
    if let Err(error) = &result {
        fs::write(
            run.join("failure.txt"),
            format!("{error:#}\n{}", vm::console(run).unwrap_or_default()),
        )?;
        let _ = vm::screenshot(run, &run.join("failure.png"));
    }
    result
}
