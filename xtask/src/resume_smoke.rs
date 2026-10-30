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

fn guest(run: &Path, command: &str) -> Result<String> {
    vm::ssh(run, &format!("set -eu\n{command}"))
}

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
fn action(run: &Path, wanted: &str) -> Result<()> {
    vm::key(run, "f2")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["panel"] == "Actions",
            "Waiting for action menu"
        );
        Ok(())
    })?;
    for _ in 0..24 {
        let selected = vm::screen(run)?["ui"]["selected_action"].clone();
        if selected == wanted {
            vm::key(run, "ret")?;
            return wait_for(10, None, || {
                ensure!(
                    vm::screen(run)?["ui"]["panel"] == "ConfirmResume",
                    "Waiting for {wanted} confirmation"
                );
                Ok(())
            });
        }
        vm::key(run, "down")?;
        wait_for(5, None, || {
            ensure!(
                vm::screen(run)?["ui"]["selected_action"] != selected,
                "Waiting for next action after {selected}"
            );
            Ok(())
        })?;
    }
    anyhow::bail!("Action menu did not contain {wanted}")
}

fn pending(run: &Path) -> Result<Value> {
    wait_for(25, None, || {
        let s = state(run, "pending")?;
        ensure!(s["event"] == "catalog-ready", "Waiting for pending catalog");
        Ok(s)
    })
}

fn target(screen: &Value) -> Value {
    screen["state"]["environments"]
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
        .clone()
}

fn cancelled_header(run: &Path) -> Result<()> {
    guest(
        run,
        "test $(dd if=/dev/mapper/resume-swap bs=1 skip=4086 count=10 2>/dev/null) = SWAPSPACE2; test $(dd if=/dev/mapper/resume-swap bs=1 skip=4096 count=17 2>/dev/null) = preserved-payload",
    )?;
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
    guest(
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
            guest(run, "test -b /dev/vda")?;
            Ok(())
        })?;
        guest(
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
mkdir -p /fixture/nix/var/nix/profiles /fixture/nix/store/shared /fixture/nix/store/system1/etc /fixture/nix/store/system2/etc
touch /fixture/nix/store/system1/init /fixture/nix/store/system2/init
"#,
        )?;
        // A real kernel/initramfs permits a cold kexec into the loader again.
        // This is a handoff fixture, not a full NixOS or restored RAM session.
        for (source, destination) in [("vmlinuz", "kernel"), ("initramfs.img", "initrd")] {
            vm::write_file(
                run,
                &format!("/fixture/nix/store/shared/{destination}"),
                &fs::read(image.join(source))?,
            )?;
        }
        for generation in [1, 2] {
            let spec = json!({"org.nixos.bootspec.v1": {"system":"x86_64-linux", "kernel":"/nix/store/shared/kernel", "initrd":"/nix/store/shared/initrd", "init":format!("/nix/store/system{generation}/init"), "toplevel":format!("/nix/store/system{generation}"), "kernelParams":["console=ttyS0,115200","console=tty0","loglevel=3","panic=-1"], "label":format!("resume generation {generation}")}});
            vm::write_file(
                run,
                &format!("/fixture/nix/store/system{generation}/boot.json"),
                &serde_json::to_vec(&spec)?,
            )?;
            vm::write_file(
                run,
                &format!("/fixture/nix/store/system{generation}/etc/fstab"),
                b"zbm_resume/root / zfs defaults 0 0\n",
            )?;
            guest(
                run,
                &format!(
                    "ln -s /nix/store/system{generation} /fixture/nix/var/nix/profiles/system-{generation}-link"
                ),
            )?;
        }
        guest(
            run,
            "ln -s system-2-link /fixture/nix/var/nix/profiles/system; umount /fixture; zpool export zbm_resume",
        )?;
        let guid = guest(run, "zpool import | awk '/id:/ { print $2 }'")?
            .trim()
            .to_owned();
        let mut config: Value = serde_json::from_str(&guest(run, "cat /etc/zbm-rs/config.json")?)?;
        config["resume"]["credential"]["pool_guid"] = guid.clone().into();
        guest(
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
        guest(run, "cryptsetup close resume-swap")?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            state(run, "locked")?;
            prompt(run)
        })?;
        guest(
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
        guest(
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
        let saved_target = target(&cold);
        let boot_id = guest(run, "cat /proc/sys/kernel/random/boot_id")?;
        seed_image(run, &saved_target, &guid)?;
        vm::key(run, "f6")?;
        let pending_screen = pending(run)?;
        ensure!(
            pending_screen["state"]["resume"]["session"]["target"]["generation"] == 1,
            "Wrong default generation"
        );
        guest(
            run,
            "test $(zpool get -H -o value readonly zbm_resume) = on",
        )?;
        vm::screenshot(run, &run.join("resume-pending.png"))?;
        let header = guest(
            run,
            "dd if=/dev/mapper/resume-swap bs=4096 count=1 2>/dev/null | sha256sum",
        )?;
        action(run, "CancelResume")?;
        vm::screenshot(run, &run.join("discard-stay-confirmation.png"))?;
        vm::key(run, "esc")?;
        ensure!(
            guest(
                run,
                "dd if=/dev/mapper/resume-swap bs=4096 count=1 2>/dev/null | sha256sum"
            )? == header,
            "Esc changed the saved image"
        );

        // Expire consent after the dialog opens. A failed cancellation must
        // preserve the image and must never run the queued cold boot.
        action(run, "ColdBoot")?;
        vm::screenshot(run, &run.join("cold-boot-confirmation.png"))?;
        guest(
            run,
            "dd if=/dev/mapper/resume-swap of=/run/header-byte bs=1 skip=2048 count=1 2>/dev/null; printf x | dd of=/dev/mapper/resume-swap bs=1 seek=2048 conv=notrunc 2>/dev/null; sync",
        )?;
        vm::key(run, "ret")?;
        wait_for(15, None, || {
            let s = state(run, "pending")?;
            ensure!(
                s["event"] == "resume-cancel-error",
                "Waiting for expired-consent error"
            );
            Ok(())
        })?;
        ensure!(
            guest(run, "cat /proc/sys/kernel/random/boot_id")? == boot_id,
            "Failed cancellation booted a kernel"
        );
        guest(
            run,
            "test ! -e /run/zbm-rs/boot-plan.json; test $(dd if=/dev/mapper/resume-swap bs=1 skip=4086 count=9 2>/dev/null) = S1SUSPEND; test $(zpool get -H -o value readonly zbm_resume) = on; dd if=/run/header-byte of=/dev/mapper/resume-swap bs=1 seek=2048 conv=notrunc 2>/dev/null; sync",
        )?;
        vm::screenshot(run, &run.join("expired-consent.png"))?;
        vm::key(run, "f6")?;
        pending(run)?;
        action(run, "CancelResume")?;
        vm::key(run, "ret")?;
        wait_for(15, None, || {
            let s = state(run, "inactive")?;
            ensure!(
                s["event"] == "resume-cancelled"
                    && s["ui"]["panel"] == "None"
                    && s["state"]["error"].is_null(),
                "Standalone discard did not return to the menu"
            );
            Ok(())
        })?;
        cancelled_header(run)?;
        ensure!(
            guest(run, "cat /proc/sys/kernel/random/boot_id")? == boot_id,
            "Standalone discard booted a kernel"
        );
        guest(
            run,
            "test ! -e /run/zbm-rs/boot-plan.json; test $(zpool get -H -o value readonly zbm_resume) = on",
        )?;
        ensure!(
            vm::console(run)?.contains("Generation 2"),
            "Discard left the saved generation as the displayed default"
        );
        vm::screenshot(run, &run.join("discarded-stay-menu.png"))?;
        seed_image(run, &saved_target, &guid)?;
        vm::key(run, "f6")?;
        pending(run)?;
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
        cancelled_header(run)?;
        for key in ["e", "x", "i", "t", "ret"] {
            vm::key(run, key)?;
        }
        wait_for(25, None, || {
            let s = state(run, "inactive")?;
            ensure!(s["event"] == "catalog-ready", "Waiting for shell return");
            Ok(())
        })?;
        seed_image(run, &saved_target, &guid)?;
        vm::key(run, "f6")?;
        pending(run)?;
        // The environment defaults to generation 2, but its saved target is 1.
        // Explicit cold boot must pin 1 before invalidation changes selection.
        action(run, "ColdBoot")?;
        vm::key(run, "ret")?;
        wait_for(90, Some(&mut owned.0), || {
            ensure!(
                vm::screen(run)?["config"]["resume"]["credential"]["pool_guid"] == "42",
                "Waiting for the newly booted loader fixture"
            );
            Ok(())
        })?;
        // This owned disposable VM generates fresh SSH host keys in RAM on
        // each kernel boot. Forget only its per-run host entry after observing
        // the new fixture; production host-key checking is unchanged.
        fs::remove_file(run.join("known_hosts"))?;
        wait_for(90, Some(&mut owned.0), || {
            let next_id = guest(run, "cat /proc/sys/kernel/random/boot_id")?;
            ensure!(next_id != boot_id, "Waiting for cold kernel handoff");
            guest(run, "grep -qw init=/nix/store/system1/init /proc/cmdline")?;
            Ok(())
        })?;
        // The fixture payload is the loader itself, whose baked pool GUID is
        // deliberately unrelated. Restore only the disposable runtime policy.
        guest(run, "rm /etc/zbm-rs/config.json")?;
        vm::write_file(
            run,
            "/etc/zbm-rs/config.json",
            &serde_json::to_vec(&config)?,
        )?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            state(run, "locked")?;
            prompt(run)
        })?;
        password(run, "source-passphrase")?;
        let after_cold = wait_for(25, None, || {
            let s = state(run, "inactive")?;
            ensure!(
                s["event"] == "catalog-ready",
                "Waiting for cold-boot fixture catalog"
            );
            Ok(s)
        })?;
        cancelled_header(run)?;
        vm::screenshot(run, &run.join("after-cold-boot.png"))?;
        // Rescue uses the same image inspection/cancellation API. It must not
        // turn a recognized image into a default resume or bypass confirmation.
        seed_image(run, &target(&after_cold), &guid)?;
        config["resume"]["role"] = "cancel-only".into();
        vm::write_file(
            run,
            "/etc/zbm-rs/config.json",
            &serde_json::to_vec(&config)?,
        )?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            let s = state(run, "unavailable")?;
            ensure!(s["event"] == "catalog-ready", "Waiting for rescue catalog");
            ensure!(
                s["state"]["resume"]["reason"]
                    .as_str()
                    .is_some_and(|s| s.contains("Rescue cannot resume")),
                "Rescue did not explain its cancellation-only role"
            );
            Ok(())
        })?;
        guest(
            run,
            "if zbm-rs --resume-restore-config /etc/zbm-rs/config.json > /run/rescue-restore-error 2>&1; then exit 1; fi; grep -q 'Cancellation-only rescue' /run/rescue-restore-error",
        )?;
        let rescue_boot_id = guest(run, "cat /proc/sys/kernel/random/boot_id")?;
        vm::key(run, "ret")?;
        wait_for(10, None, || {
            ensure!(
                vm::screen(run)?["ui"]["panel"] == "ConfirmResume",
                "Rescue boot bypassed cancellation confirmation"
            );
            Ok(())
        })?;
        vm::key(run, "esc")?;
        state(run, "unavailable")?;
        vm::screenshot(run, &run.join("rescue-cancellation-only.png"))?;
        action(run, "CancelResume")?;
        vm::key(run, "ret")?;
        wait_for(25, None, || {
            state(run, "inactive")?;
            cancelled_header(run)
        })?;
        ensure!(
            guest(run, "cat /proc/sys/kernel/random/boot_id")? == rescue_boot_id,
            "Standalone rescue cancellation booted a kernel"
        );
        config["resume"]["role"] = "resume".into();
        vm::write_file(
            run,
            "/etc/zbm-rs/config.json",
            &serde_json::to_vec(&config)?,
        )?;
        vm::key(run, "f6")?;
        wait_for(25, None, || {
            let s = state(run, "inactive")?;
            ensure!(
                s["event"] == "catalog-ready",
                "Waiting for normal role after rescue test"
            );
            Ok(())
        })?;
        seed_image(run, &target(&after_cold), &guid)?;
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
                &json!({"passed":true,"checks":["foreign-mapping","metadata-only-before-unlock","wrong-ZFS-password","different-LUKS-password","shared-credential","manager-restart","pending-default","other-generation-confirmation","Esc-preserves-image","expired-consent-blocks-cold-boot","standalone-discard-stays-in-menu","verified-cancel-before-shell","payload-preserved","cold-kexec-pins-saved-generation","rescue-rejects-restore-utility","rescue-boot-requires-cancellation","rescue-standalone-discard","F10-preserves-entire-disk","no-secret-telemetry"],"real_ram_restore":false,"cold_handoff_payload":"loader-fixture"}),
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
