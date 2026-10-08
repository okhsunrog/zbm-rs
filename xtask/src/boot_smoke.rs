//! Exact-generation NixOS kexec acceptance on a disposable ZFS root.
use crate::{
    smoke::{OwnedVm, Swtpm, manager_pid, new_manager, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};
pub struct Options<'a> {
    pub tcg: bool,
    pub port: u16,
    pub snapshot: bool,
    pub secure_vars: Option<&'a Path>,
    pub swtpm: bool,
}
pub fn run(run: &Path, image: &Path, fixture: &Path, options: Options<'_>) -> Result<()> {
    let Options {
        tcg,
        port,
        snapshot,
        secure_vars,
        swtpm,
    } = options;
    crate::smoke::install_interrupt()?;
    let owned_tpm = swtpm.then(|| Swtpm::start(run)).transpose()?;
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
    if let Some(vars) = secure_vars {
        command.arg("--secure-vars").arg(vars);
    }
    if let Some(tpm) = &owned_tpm {
        command.arg("--tpm-socket").arg(&tpm.socket);
    }
    let mut owned = OwnedVm(command.stdout(Stdio::null()).spawn()?);
    let result = (|| {
        wait_for(90, Some(&mut owned.0), || {
            ensure!(vm::screen(run)?["event"] == "ready", "Waiting for loader");
            vm::ssh(run, "test -b /dev/vdb")?;
            Ok(())
        })?;
        let original_boot = vm::ssh(run, "cat /proc/sys/kernel/random/boot_id")?;
        if secure_vars.is_some() {
            let readiness = vm::ssh(run, "cat /run/zbm-rs/security.json")?;
            let evidence: serde_json::Value = serde_json::from_str(&readiness)?;
            ensure!(
                evidence["firmware"] == "enabled"
                    && evidence["mode"] == "enforce"
                    && evidence["ima_policy_loaded"] == true,
                "Verified loader readiness missing"
            );
            if swtpm {
                ensure!(
                    evidence["tpm"]["state"] == "ready",
                    "TPM not ready: {evidence}"
                );
            }
            fs::write(run.join("security-readiness.json"), readiness)?;
        }
        println!("Loader ready; installing generated NixOS fixture on disposable /dev/vda");
        vm::ssh(
            run,
            "set -eu; zpool create -o cachefile=none -O compression=lz4 -O mountpoint=none zbm_fixture /dev/vda; zfs create -o mountpoint=legacy -o org.zfsbootmenu:active=on zbm_fixture/nixos; zfs create -o mountpoint=legacy -o org.zfsbootmenu:active=on zbm_fixture/inspect; zpool set bootfs=zbm_fixture/nixos zbm_fixture; mkdir -p /fixture-root; mount -t zfs zbm_fixture/nixos /fixture-root",
        )?;
        vm::ssh_timeout(run, "tar -xf /dev/vdb -C /fixture-root", 900)?;
        if secure_vars.is_some() && !snapshot {
            // A valid-looking Bootspec with changed arguments must never become
            // authorization, even though it uses the same signed image pair.
            vm::ssh(
                run,
                "set -eu; fake=/nix/store/zbm-unauthorized-generation; mkdir -p /fixture-root$fake; system=$(readlink /fixture-root/nix/var/nix/profiles/system-2-link); sed \"s|$system|$fake|g; s/\\\"zbm.fixture=2\\\"/\\\"zbm.fixture=unauthorized\\\"/\" /fixture-root$system/boot.json > /fixture-root$fake/boot.json; ln -s $system/init /fixture-root$fake/init; ln -s $system/etc /fixture-root$fake/etc; ln -s $fake /fixture-root/nix/var/nix/profiles/system-4-link",
            )?;
        }
        // A malformed peer must not hide either real NixOS generation, including
        // when their profiles are later browsed inside the source snapshot.
        vm::ssh(
            run,
            "set -eu; mkdir -p /fixture-root/nix/store/zbm-broken-generation; echo broken-json > /fixture-root/nix/store/zbm-broken-generation/boot.json; ln -s /nix/store/zbm-broken-generation /fixture-root/nix/var/nix/profiles/system-3-link",
        )?;
        if snapshot {
            vm::ssh(
                run,
                "set -eu; cd /fixture-root/nix/var/nix/profiles; cp -a system-2-link /run/system-2-link; rm system-2-link; ln -sfn system-1-link system; echo snapshot-original > /fixture-root/snapshot-proof; zfs snapshot zbm_fixture/nixos@known-good; cp -a /run/system-2-link .; ln -sfn system-2-link system; echo live-changed > /fixture-root/snapshot-proof",
            )?;
        }
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
        environments(run)?;
        if snapshot {
            return snapshot_boot(
                run,
                &mut owned,
                &expected,
                original_boot.trim(),
                secure_vars.is_some(),
                swtpm,
            );
        }
        vm::key(run, "ret")?;
        targets(run, secure_vars.is_some())?;
        if secure_vars.is_some() {
            vm::key(run, "ret")?;
            wait_for(15, None, || {
                let state = vm::screen(run)?;
                ensure!(
                    state["event"] == "boot-error" && state["state"]["error"].is_string(),
                    "Unauthorized plan was not rejected"
                );
                vm::ssh(run, "test \"$(cat /sys/kernel/kexec_loaded)\" = 0")?;
                fs::write(
                    run.join("unauthorized-plan.json"),
                    serde_json::to_vec_pretty(&state)?,
                )?;
                Ok(())
            })?;
        }
        vm::screenshot(run, &run.join("generations.png"))?;
        let old = manager_pid(run)?;
        vm::key(run, "n")?;
        wait_for(20, None, || new_manager(run, old, 1))?;
        vm::key(run, "ret")?;
        environments(run)?;
        vm::key(run, "ret")?;
        targets(run, secure_vars.is_some())?;
        select_generation(run, 1)?;
        vm::screenshot(run, &run.join("selected-generation.png"))?;
        // A foreign dataset replacing an owned mount must survive preparation.
        let mountinfo = vm::ssh(run, "cat /proc/self/mountinfo")?;
        let mount = mountinfo
            .lines()
            .find_map(|line| {
                let (before, after) = line.split_once(" - ")?;
                (after.split_whitespace().nth(1) == Some("zbm_fixture/inspect"))
                    .then(|| before.split_whitespace().nth(4).unwrap().to_owned())
            })
            .ok_or_else(|| anyhow::anyhow!("Missing owned inspection mount"))?;
        ensure!(
            mount.starts_with("/run/zbm-rs/roots/")
                && mount
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-".contains(&b)),
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
                "set -eu; zpool list -H -o name zbm_fixture; umount '{mount}'; zpool export zbm_foreign; mount -t zfs -o ro,zfsutil zbm_fixture/inspect '{mount}'"
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
            if swtpm {
                ensure!(
                    log.contains("ZBM_TARGET_TPM_SRK_READY"),
                    "Target TPM SRK acceptance missing"
                );
            }
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
                &serde_json::json!({"passed":true,"generation":1,"toplevel":expected,"loader_boot_id":original_boot.trim(),"proof":proof,
                    "firmware_secure_boot":secure_vars.is_some(),"swtpm":swtpm,"unauthorized_parameters_rejected":secure_vars.is_some(),
                    "scope":"selected OS boot and loader protection; target TPM consumer integration has separate gates",
                    "checks":["UEFI-loader","ZFS-root","Bootspec","non-default-generation","manager-restart-reconciliation","foreign-mount-protection","kexec_file_load","NixOS-multi-user","guest-poweroff"]}),
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
fn select_generation(run: &Path, generation: u64) -> Result<()> {
    // Rejected generations are inspectable rows too. A fixed number of Down
    // keys can select a different target as the catalog grows.
    for _ in 0..8 {
        let state = vm::screen(run)?;
        let row = state["ui"]["selected_row"].clone();
        if row["Generation"] == generation {
            let selected = state["state"]["selected_target"].as_u64().unwrap() as usize;
            ensure!(
                state["state"]["targets"][selected]["generation"] == generation,
                "UI/domain generation mismatch"
            );
            return Ok(());
        }
        vm::key(run, "down")?;
        wait_for(5, None, || {
            ensure!(
                vm::screen(run)?["ui"]["selected_row"] != row,
                "Waiting for selection acknowledgment"
            );
            Ok(())
        })?;
    }
    anyhow::bail!("Generation {generation} not reachable in visible rows")
}
fn targets(run: &Path, secure: bool) -> Result<()> {
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
            if secure {
                targets.len() == 3
                    && targets[0]["generation"] == 4
                    && targets[1]["generation"] == 2
                    && targets[2]["generation"] == 1
            } else {
                targets.len() == 2 && targets[0]["generation"] == 2 && targets[1]["generation"] == 1
            },
            "Wrong generations: {targets:?}"
        );
        ensure!(
            state["state"]["rejected_generations"][0]["generation"] == 3,
            "Missing malformed-generation diagnostic: {state}"
        );
        Ok(())
    })
}

fn environments(run: &Path) -> Result<()> {
    wait_for(30, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "boot-environments" && state["state"]["error"].is_null(),
            "Waiting for BE discovery: {state}"
        );
        let selected = state["state"]["selected_environment"].as_u64().unwrap() as usize;
        ensure!(
            state["state"]["environments"][selected]["dataset"] == "zbm_fixture/nixos"
                && state["state"]["environments"][selected]["is_default"] == true,
            "bootfs was not selected: {state}"
        );
        Ok(())
    })
}

fn snapshot_generations(run: &Path) -> Result<()> {
    vm::key(run, "t")?;
    wait_for(30, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "snapshots" && state["state"]["error"].is_null(),
            "Waiting for snapshots: {state}"
        );
        ensure!(
            state["state"]["snapshots"][0]["source"]["name"] == "zbm_fixture/nixos@known-good",
            "Missing snapshot"
        );
        Ok(())
    })?;
    vm::screenshot(run, &run.join("snapshots.png"))?;
    vm::key(run, "ret")?;
    wait_for(30, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "boot-targets" && state["state"]["error"].is_null(),
            "Waiting for snapshot generations: {state}"
        );
        let targets = state["state"]["targets"].as_array().unwrap();
        ensure!(
            targets.len() == 1
                && targets[0]["generation"] == 1
                && targets[0]["snapshot"]["name"] == "zbm_fixture/nixos@known-good",
            "Wrong snapshot generation: {targets:?}"
        );
        ensure!(
            state["state"]["rejected_generations"][0]["generation"] == 3,
            "Missing snapshot-generation diagnostic: {state}"
        );
        Ok(())
    })
}
fn prepared_clone(run: &Path) -> Result<String> {
    vm::key(run, "c")?;
    wait_for(20, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "snapshot-prepared",
            "Waiting for clone preparation: {state}"
        );
        let plan: serde_json::Value =
            serde_json::from_str(&vm::ssh(run, "cat /run/zbm-rs/boot-plan.json")?)?;
        let clone = plan["target"]["dataset"].as_str().unwrap().to_owned();
        ensure!(
            clone.starts_with("zbm_fixture/zbm-rs-")
                && clone
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-".contains(&b)),
            "Invalid clone name"
        );
        ensure!(
            plan["cmdline"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == &format!("root={clone}")),
            "Missing clone root argument"
        );
        Ok(clone)
    })
}
fn snapshot_boot(
    run: &Path,
    owned: &mut OwnedVm,
    expected: &str,
    original_boot: &str,
    secure: bool,
    swtpm: bool,
) -> Result<()> {
    snapshot_generations(run)?;
    // Do not let clone preparation upgrade a read-only pool import.
    vm::ssh(
        run,
        r#"set -eu; for path in $(awk 'index($5,"/run/zbm-rs/snapshots/")==1 {print $5}' /proc/self/mountinfo); do umount "$path"; done; for path in $(awk 'index($5,"/run/zbm-rs/roots/")==1 {print $5}' /proc/self/mountinfo); do umount "$path"; done; zpool export zbm_fixture; zpool import -N -o cachefile=none -o readonly=on zbm_fixture"#,
    )?;
    vm::key(run, "c")?;
    snapshot_error(run, "read-only")?;
    vm::ssh(
        run,
        "zpool export zbm_fixture; zpool import -N -o cachefile=none zbm_fixture",
    )?;
    vm::key(run, "r")?;
    wait_for(20, None, || {
        ensure!(vm::screen(run)?["event"] == "ready", "Waiting for rescan");
        Ok(())
    })?;
    vm::key(run, "ret")?;
    environments(run)?;
    snapshot_generations(run)?;
    vm::ssh(run, "zpool set org.zfsbootmenu:readonly=on zbm_fixture")?;
    vm::key(run, "c")?;
    snapshot_error(run, "org.zfsbootmenu:readonly")?;
    vm::ssh(run, "zpool set org.zfsbootmenu:readonly=off zbm_fixture")?;
    let first_clone = prepared_clone(run)?;
    vm::key(run, "f3")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["panel"] == "Details",
            "Waiting for snapshot details"
        );
        Ok(())
    })?;
    let details = vm::console(run)?;
    ensure!(
        details.contains(&first_clone) && details.contains("Prepared cmdline"),
        "Missing real prepared clone inputs"
    );
    vm::screenshot(run, &run.join("ui-snapshot-details.png"))?;
    vm::key(run, "esc")?;
    vm::key(run, "d")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["event"] == "discard-confirmation",
            "Waiting for cancellable discard"
        );
        Ok(())
    })?;
    vm::screenshot(run, &run.join("ui-discard-confirmation.png"))?;
    vm::key(run, "esc")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["panel"] == "None",
            "Waiting for discard cancellation"
        );
        Ok(())
    })?;
    vm::ssh(run, &format!("zfs list -H '{first_clone}'"))?;
    // Non-recursive discard must refuse a foreign child dataset.
    vm::ssh(
        run,
        &format!("zfs create -o mountpoint=none '{first_clone}/foreign-child'"),
    )?;
    discard(run)?;
    snapshot_error(run, "")?;
    vm::ssh(
        run,
        &format!(
            "zfs list -H '{first_clone}/foreign-child'; zfs destroy '{first_clone}/foreign-child'"
        ),
    )?;
    discard(run)?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["event"] == "snapshot-discarded",
            "Waiting for explicit discard"
        );
        Ok(())
    })?;
    let clone = prepared_clone(run)?;
    ensure!(clone != first_clone, "Discarded clone was reused");
    let token = vm::ssh(
        run,
        &format!("zfs get -H -o value org.zbm-rs:owner '{clone}'"),
    )?
    .trim()
    .to_owned();
    ensure!(
        token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid clone token"
    );
    vm::ssh(run, &format!("zfs set org.zbm-rs:owner=foreign '{clone}'"))?;
    vm::key(run, "c")?;
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "snapshot-error"
                && state["state"]["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("ownership")),
            "Foreign clone was not rejected: {state}"
        );
        Ok(())
    })?;
    vm::ssh(run, &format!("zfs set org.zbm-rs:owner={token} '{clone}'"))?;
    let old = manager_pid(run)?;
    vm::ssh(run, &format!("kill -ABRT {old}"))?;
    wait_for(20, None, || new_manager(run, old, 1))?;
    vm::key(run, "ret")?;
    environments(run)?;
    snapshot_generations(run)?;
    ensure!(
        prepared_clone(run)? == clone,
        "Manager restart created another clone"
    );
    vm::ssh(
        run,
        &format!(
            "test \"$(zfs get -H -o value origin '{clone}')\" = zbm_fixture/nixos@known-good; test \"$(zfs get -H -o value org.zbm-rs:state '{clone}')\" = prepared; test \"$(zfs list -H -r -o name zbm_fixture | awk 'index($0,\"zbm_fixture/zbm-rs-\")==1 {{n++}} END {{print n}}')\" = 1"
        ),
    )?;
    vm::screenshot(run, &run.join("snapshot-prepared.png"))?;
    fs::write(run.join("snapshot-prepared.txt"), vm::console(run)?)?;
    vm::key(run, "ret")?;
    let proof = wait_for(180, None, || {
        let log = fs::read_to_string(run.join("serial.log"))?;
        let line = log
            .lines()
            .find(|line| line.starts_with("ZBM_BOOT_SUCCESS generation=1 "))
            .ok_or_else(|| anyhow::anyhow!("Waiting for snapshot OS boot proof"))?;
        ensure!(
            line.contains(&format!("system={expected}")) && line.contains(&format!("root={clone}")),
            "Wrong snapshot boot: {line}"
        );
        ensure!(
            !line.contains(&format!("boot_id={original_boot}")),
            "Boot ID did not change"
        );
        if swtpm {
            ensure!(
                log.contains("ZBM_TARGET_TPM_SRK_READY"),
                "Snapshot target TPM SRK acceptance missing"
            );
        }
        Ok(line.to_owned())
    })?;
    wait_for(120, None, || {
        ensure!(
            owned.0.try_wait()?.is_some_and(|status| status.success()),
            "Waiting for snapshot OS poweroff"
        );
        Ok(())
    })?;
    fs::write(
        run.join("snapshot-report.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "passed":true,"clone":clone,"source":"zbm_fixture/nixos@known-good","generation":1,"proof":proof,
            "firmware_secure_boot":secure,"swtpm":swtpm,
            "scope":"authorized snapshot clone boot; target TPM consumer integration has separate gates",
            "checks":["snapshot-generation-discovery","explicit-clone","foreign-owner-rejection","abort-reconciliation","no-duplicate-clone","systemd-root-override","original-and-snapshot-unchanged","writable-clone","exact-generation-kexec","guest-poweroff"]
        }))?,
    )?;
    println!("Snapshot clone boot acceptance passed");
    Ok(())
}

fn snapshot_error(run: &Path, message: &str) -> Result<()> {
    wait_for(15, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "snapshot-error"
                && state["state"]["error"]
                    .as_str()
                    .is_some_and(|error| error.contains(message)),
            "Waiting for snapshot rejection: {state}"
        );
        Ok(())
    })
}

fn discard(run: &Path) -> Result<()> {
    vm::key(run, "d")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["event"] == "discard-confirmation",
            "Waiting for clone discard confirmation"
        );
        Ok(())
    })?;
    vm::key(run, "ret")?;
    Ok(())
}
