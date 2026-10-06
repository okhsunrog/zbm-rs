//! Fault isolation and responsive recovery on the owned disposable smoke VM.
use crate::{
    smoke::{manager_pid, new_manager, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

const BAD: &str = "zbm_fixture/z_bad";

fn restore_mount(run: &Path) -> Result<()> {
    vm::ssh(run, "rm /bin/mount; ln -s /bin/busybox /bin/mount")?;
    Ok(())
}

fn mount_wrapper(run: &Path, hang: bool) -> Result<()> {
    let effect = if hang {
        "echo $$ > /run/zbm-rs/hung-mount.pid; exec /bin/busybox sleep 600"
    } else {
        "echo deliberate-BE-mount-failure >&2; exit 1"
    };
    vm::ssh(
        run,
        &format!(
            "set -eu; rm -f /run/zbm-rs/hung-mount.pid; rm /bin/mount; cat > /bin/mount <<'ZBM_WRAPPER'\n#!/bin/sh\nfor argument in \"$@\"; do\n if [ \"$argument\" = {BAD} ]; then {effect}; fi\ndone\nexec /bin/busybox mount \"$@\"\nZBM_WRAPPER\nchmod +x /bin/mount"
        ),
    )?;
    Ok(())
}

pub fn start_hung_mount(run: &Path) -> Result<u32> {
    let mounts = vm::ssh(run, "cat /proc/self/mountinfo")?;
    for line in mounts.lines() {
        if let Some((before, after)) = line.split_once(" - ")
            && after.split_whitespace().nth(1) == Some(BAD)
        {
            let path = before.split_whitespace().nth(4).unwrap();
            ensure!(
                path.starts_with("/run/zbm-rs/roots/")
                    && path
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_-".contains(&b)),
                "Unexpected fixture mount path"
            );
            vm::ssh(run, &format!("umount '{path}'"))?;
        }
    }
    mount_wrapper(run, true)?;
    vm::key(run, "ret")?;
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "operation-started" && state["state"]["operation"].is_string(),
            "Waiting for active operation: {state}"
        );
        Ok(vm::ssh(run, "cat /run/zbm-rs/hung-mount.pid")?
            .trim()
            .parse()?)
    })
}

fn exit_shell(run: &Path, previous: u32) -> Result<()> {
    for key in ["e", "x", "i", "t", "ret"] {
        vm::key(run, key)?;
    }
    wait_for(10, None, || new_manager(run, previous, 1))?;
    Ok(())
}

pub fn exercise(run: &Path) -> Result<()> {
    // Existing mounts and all new datasets belong to this disposable fixture.
    // Provision writable, then let the manager re-import under its own policy.
    vm::ssh(
        run,
        r#"set -eu; for path in $(awk 'index($5,"/run/zbm-rs/roots/")==1 {print $5}' /proc/self/mountinfo); do umount "$path"; done; zpool export zbm_fixture; zpool import -N -o cachefile=none zbm_fixture; zfs create -o canmount=noauto -o mountpoint=legacy -o org.zfsbootmenu:active=on zbm_fixture/z_bad; mkdir -p /fixture-default; mount -t zfs zbm_fixture/default /fixture-default; mkdir -p /fixture-default/nix/var/nix/profiles /fixture-default/nix/store/good/etc /fixture-default/nix/store/broken"#,
    )?;
    let spec = json!({"org.nixos.bootspec.v1": {
        "system": "x86_64-linux", "kernel": "/nix/store/good/kernel", "initrd": "/nix/store/good/initrd",
        "init": "/nix/store/good/init", "toplevel": "/nix/store/good", "kernelParams": ["root=fstab"], "label": "fault-isolation fixture"
    }});
    vm::ssh(
        run,
        &format!(
            "set -eu; cat > /fixture-default/nix/store/good/boot.json <<'ZBM_SPEC'\n{spec}\nZBM_SPEC\nprintf 'zbm_fixture/default / zfs defaults 0 0\\n' > /fixture-default/nix/store/good/etc/fstab; touch /fixture-default/nix/store/good/kernel /fixture-default/nix/store/good/initrd /fixture-default/nix/store/good/init; echo broken-json > /fixture-default/nix/store/broken/boot.json; ln -s /nix/store/good /fixture-default/nix/var/nix/profiles/system-1-link; ln -s /nix/store/broken /fixture-default/nix/var/nix/profiles/system-2-link; umount /fixture-default; zpool export zbm_fixture"
        ),
    )?;
    mount_wrapper(run, false)?;
    vm::key(run, "r")?;
    wait_for(20, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "ready" && state["state"]["error"].is_null(),
            "Waiting for rescan"
        );
        Ok(())
    })?;
    vm::key(run, "ret")?;
    let isolation = wait_for(30, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "boot-environments" && state["state"]["error"].is_null(),
            "Waiting for isolated BE failures: {state}"
        );
        let environments = state["state"]["environments"].as_array().unwrap();
        let good = environments
            .iter()
            .find(|be| be["dataset"] == "zbm_fixture/default")
            .unwrap();
        let bad = environments.iter().find(|be| be["dataset"] == BAD).unwrap();
        ensure!(
            good["targets"]
                .as_array()
                .is_some_and(|targets| targets.len() == 1),
            "Good generation disappeared: {good}"
        );
        ensure!(
            good["rejected_generations"][0]["generation"] == 2,
            "Missing per-generation diagnostic: {good}"
        );
        ensure!(
            bad["unavailable"]
                .as_str()
                .is_some_and(|error| error.contains("deliberate-BE-mount-failure")),
            "Missing mount diagnostic: {bad}"
        );
        Ok(state)
    })?;
    vm::screenshot(run, &run.join("fault-isolation.png"))?;
    vm::key(run, "ret")?;
    let generations = wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "boot-targets"
                && state["state"]["targets"][0]["generation"] == 1
                && state["state"]["rejected_generations"][0]["generation"] == 2,
            "Waiting for good generation: {state}"
        );
        ensure!(
            vm::console(run)?.contains("generation 2:"),
            "Generation diagnostic is not visible at the VT"
        );
        Ok(state)
    })?;
    vm::screenshot(run, &run.join("mixed-generations.png"))?;
    fs::write(run.join("mixed-generations.txt"), vm::console(run)?)?;
    crate::interface::generations(run)?;
    for key in ["b", "b"] {
        vm::key(run, key)?;
    }
    restore_mount(run)?;

    let previous = manager_pid(run)?;
    let hung = start_hung_mount(run)?;
    vm::screenshot(run, &run.join("pending-operation.png"))?;
    fs::write(run.join("pending-operation.txt"), vm::console(run)?)?;
    vm::key(run, "f4")?;
    wait_for(10, None, || {
        ensure!(
            vm::console(run)?.contains("emergency recovery"),
            "Shell is unavailable during a pending mount"
        );
        vm::ssh(
            run,
            &format!("test ! -e /proc/{previous}; test ! -e /proc/{hung}; test -d /proc/1"),
        )?;
        Ok(())
    })?;
    restore_mount(run)?;
    exit_shell(run, previous)?;

    let previous = manager_pid(run)?;
    let hung = start_hung_mount(run)?;
    let timeout = wait_for(40, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "operation-timeout"
                && state["state"]["requires_restart"] == true
                && state["state"]["operation"].is_null(),
            "Waiting for bounded operation: {state}"
        );
        vm::ssh(run, &format!("test ! -e /proc/{hung}"))?;
        Ok(state)
    })?;
    vm::key(run, "ret")?;
    ensure!(
        vm::screen(run)?["state"]["requires_restart"] == true,
        "Timed-out mutation was retried without reconciliation"
    );
    vm::screenshot(run, &run.join("operation-timeout.png"))?;
    fs::write(run.join("operation-timeout.txt"), vm::console(run)?)?;
    restore_mount(run)?;
    vm::key(run, "n")?;
    wait_for(10, None, || new_manager(run, previous, 1))?;
    let state: Value = vm::screen(run)?;
    ensure!(
        state["state"]["requires_restart"] == false,
        "Fresh manager retained a stale timeout"
    );
    fs::write(
        run.join("failure-report.json"),
        serde_json::to_vec_pretty(&json!({
            "passed": true, "isolation": isolation, "generations": generations, "timeout": timeout,
            "checks": ["BE-mount-failure-isolation", "mixed-generation-discovery", "visible-generation-diagnostic", "shell-during-pending-mount", "cancelled-command-reaped", "operation-deadline", "timeout-requires-reconciliation", "restart-after-timeout"]
        }))?,
    )?;
    Ok(())
}
