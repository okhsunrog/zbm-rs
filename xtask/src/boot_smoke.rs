//! Exact-generation NixOS kexec acceptance on a disposable ZFS root.
use crate::{
    smoke::{OwnedVm, manager_pid, new_manager, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};
pub fn run(run: &Path, image: &Path, fixture: &Path, tcg: bool, port: u16) -> Result<()> {
    crate::smoke::install_interrupt()?;
    let expected = fs::read_to_string(fixture.join("generation-1"))?
        .trim()
        .to_owned();
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
    let result = (|| {
        wait_for(90, Some(&mut owned.0), || {
            ensure!(vm::screen(run)?["event"] == "ready", "Waiting for loader");
            vm::ssh(run, "test -b /dev/vdb")?;
            Ok(())
        })?;
        let original_boot = vm::ssh(run, "cat /proc/sys/kernel/random/boot_id")?;
        println!("Loader ready; installing generated NixOS fixture on disposable /dev/vda");
        vm::ssh(
            run,
            "set -eu; zpool create -o cachefile=none -O compression=lz4 -O mountpoint=none zbm_fixture /dev/vda; zfs create -o mountpoint=legacy zbm_fixture/nixos; mkdir -p /fixture-root; mount -t zfs zbm_fixture/nixos /fixture-root",
        )?;
        vm::ssh_timeout(run, "tar -xf /dev/vdb -C /fixture-root", 900)?;
        vm::ssh(run, "umount /fixture-root; zpool export zbm_fixture")?;
        vm::key(run, "r")?;
        wait_for(20, None, || {
            let state = vm::screen(run)?;
            ensure!(
                state["event"] == "ready"
                    && state["state"]["pools"]
                        .as_array()
                        .is_some_and(|p| p.len() == 1),
                "Waiting for fixture pool"
            );
            Ok(())
        })?;
        vm::key(run, "ret")?;
        targets(run)?;
        vm::screenshot(run, &run.join("generations.png"))?;
        let old = manager_pid(run)?;
        vm::key(run, "n")?;
        wait_for(20, None, || new_manager(run, old, 1))?;
        vm::key(run, "ret")?;
        targets(run)?;
        vm::key(run, "down")?;
        wait_for(5, None, || {
            ensure!(
                vm::screen(run)?["state"]["selected_target"] == 1,
                "Waiting for older generation selection"
            );
            Ok(())
        })?;
        vm::screenshot(run, &run.join("selected-generation.png"))?;
        // A foreign dataset replacing an owned mount must survive preparation.
        let mountinfo = vm::ssh(run, "cat /proc/self/mountinfo")?;
        let mount = mountinfo
            .lines()
            .find_map(|line| {
                let (before, after) = line.split_once(" - ")?;
                (after.split_whitespace().nth(1) == Some("zbm_fixture"))
                    .then(|| before.split_whitespace().nth(4).unwrap().to_owned())
            })
            .ok_or_else(|| anyhow::anyhow!("Missing owned pool-root mount"))?;
        ensure!(
            mount.starts_with("/run/zbm-rs/roots/")
                && mount
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/-".contains(&b)),
            "Unexpected mount path"
        );
        vm::ssh(
            run,
            &format!(
                "set -eu; umount '{mount}'; truncate -s 128M /run/zbm-foreign-vdev; zpool create -o cachefile=none -O mountpoint=none zbm_foreign /run/zbm-foreign-vdev; mount -t zfs -o ro,zfsutil zbm_foreign '{mount}'"
            ),
        )?;
        let manager = manager_pid(run)?;
        vm::key(run, "ret")?;
        wait_for(10, None, || {
            let state = vm::screen(run)?;
            ensure!(
                state["event"] == "boot-error"
                    && state["state"]["error"]
                        .as_str()
                        .is_some_and(|error| error.contains("replaced by another dataset")),
                "Waiting for foreign-mount protection: {state}"
            );
            Ok(())
        })?;
        ensure!(
            manager_pid(run)? == manager,
            "Foreign mount error killed manager"
        );
        vm::ssh(run, "test \"$(cat /sys/kernel/kexec_loaded)\" = 0")?;
        let remaining = vm::ssh(run, "cat /proc/self/mountinfo")?;
        ensure!(
            remaining.lines().any(|line| line
                .split_once(" - ")
                .is_some_and(|(before, after)| before.split_whitespace().nth(4)
                    == Some(mount.as_str())
                    && after.split_whitespace().nth(1) == Some("zbm_foreign"))),
            "Foreign mount was unmounted"
        );
        vm::ssh(
            run,
            &format!(
                "set -eu; zpool list -H -o name zbm_fixture; umount '{mount}'; zpool export zbm_foreign; mount -t zfs -o ro,zfsutil zbm_fixture '{mount}'"
            ),
        )?;
        println!("Booting selected NixOS generation 1 via kexec_file_load");
        vm::key(run, "ret")?;
        let proof = wait_for(180, None, || {
            let log = fs::read_to_string(run.join("serial.log"))?;
            let line = log
                .lines()
                .find(|line| line.starts_with("ZBM_BOOT_SUCCESS generation=1 "))
                .ok_or_else(|| anyhow::anyhow!("Waiting for target OS boot proof"))?;
            ensure!(
                line.contains(&format!("system={expected}")),
                "Wrong NixOS toplevel booted: {line}"
            );
            ensure!(
                line.contains("root=zbm_fixture/nixos"),
                "Wrong root dataset"
            );
            ensure!(
                line.contains("boot_id=")
                    && !line.contains(&format!("boot_id={}", original_boot.trim())),
                "Target kernel boot ID did not change"
            );
            ensure!(
                !log.contains("kexec-returned"),
                "Supervisor classified kexec as failed"
            );
            Ok(line.to_owned())
        })?;
        wait_for(30, None, || {
            ensure!(
                owned.0.try_wait()?.is_some_and(|s| s.success()),
                "Waiting for target OS poweroff"
            );
            Ok(())
        })?;
        fs::write(
            run.join("boot-report.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"passed":true,"generation":1,"toplevel":expected,"loader_boot_id":original_boot.trim(),"proof":proof,"checks":["UEFI-loader","ZFS-root","Bootspec","non-default-generation","manager-restart-reconciliation","foreign-mount-protection","kexec_file_load","NixOS-multi-user","guest-poweroff"]}),
            )?,
        )?;
        println!("NixOS boot acceptance passed");
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = vm::screenshot(run, &run.join("failure.png"));
        if let Ok(console) = vm::console(run) {
            let _ = fs::write(run.join("failure.txt"), console);
        }
        eprintln!("Boot smoke failed: {error:#}; preserved {}", run.display());
    }
    result
}
fn targets(run: &Path) -> Result<()> {
    wait_for(30, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "boot-targets",
            "Waiting for generation discovery"
        );
        ensure!(
            state["state"]["error"].is_null(),
            "Generation discovery failed: {state}"
        );
        let targets = state["state"]["targets"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing targets"))?;
        ensure!(
            targets.len() == 2 && targets[0]["generation"] == 2 && targets[1]["generation"] == 1,
            "Wrong generations: {targets:?}"
        );
        Ok(())
    })
}
