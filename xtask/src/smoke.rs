//! Reusable full boot scenario. Drop always terminates and reaps the owned VM.
use crate::vm;
use anyhow::{Result, bail, ensure};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

struct OwnedVm(Child);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
impl Drop for OwnedVm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn wait_for<T>(
    seconds: u64,
    mut child: Option<&mut Child>,
    mut probe: impl FnMut() -> Result<T>,
) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        ensure!(!INTERRUPTED.load(Ordering::Relaxed), "Smoke interrupted");
        if let Some(process) = child.as_mut()
            && let Some(status) = process.try_wait()?
        {
            bail!("VM exited before scenario completed: {status}");
        }
        match probe() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn ready(run: &Path, scans: u64, pools: usize) -> Result<Value> {
    let state = vm::screen(run)?;
    ensure!(
        state["event"] == "ready" && state["state"]["scans"].as_u64().unwrap_or(0) >= scans,
        "Waiting for scan {scans}: {state}"
    );
    ensure!(
        state["state"]["error"].is_null(),
        "Discovery failed: {state}"
    );
    ensure!(
        state["state"]["pools"]
            .as_array()
            .is_some_and(|p| p.len() == pools),
        "Unexpected pools: {state}"
    );
    Ok(state)
}

pub fn run(
    run: &Path,
    image: &Path,
    tcg: bool,
    port: u16,
    direct: bool,
    lifecycle: bool,
) -> Result<()> {
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed))?;
    // Re-exec this running inode even if Cargo replaces the checkout binary.
    let mut command = Command::new("/proc/self/exe");
    command
        .args(["vm", "--run"])
        .arg(run)
        .args(["boot", "--port", &port.to_string(), "--image"])
        .arg(image);
    if tcg {
        command.arg("--tcg");
    }
    if direct {
        command.arg("--direct");
    }
    let mut owned = OwnedVm(command.stdout(Stdio::null()).spawn()?);
    let result = (|| {
        let empty = wait_for(90, Some(&mut owned.0), || ready(run, 1, 0))?;
        ensure!(
            empty["config"]["ui"]["timeout_secs"] == 0
                && empty["config"]["zfs"]["import_policy"] == "read-only"
                && empty["config"]["ui"]["title"] == "zbm-rs test",
            "Manager did not load the immutable test config: {empty}"
        );
        vm::screenshot(run, &run.join("empty.png"))?;
        wait_for(30, Some(&mut owned.0), || {
            vm::ssh(
                run,
                "uname -r; test -c /dev/zfs; test -L /etc/zbm-rs/config.json; test -r /etc/zbm-rs/config.json",
            )
        })?;
        let console = vm::console(run)?;
        ensure!(
            console.contains("Rescan") && console.contains("Power off"),
            "Missing VT controls"
        );
        fs::write(run.join("empty.txt"), console)?;
        // Disposable guest disk only; never attach host devices to this scenario.
        vm::ssh(
            run,
            r#"zpool create -f -o cachefile=none -m none zbm_fixture /dev/vda && zfs create -o mountpoint=none zbm_fixture/ROOT && zfs snapshot zbm_fixture/ROOT@fresh && zfs clone -o mountpoint=none zbm_fixture/ROOT@fresh zbm_fixture/clone && zfs rename zbm_fixture/clone zbm_fixture/renamed && zfs promote zbm_fixture/renamed && printf '%s\n' disposable-zbm-test-passphrase > /run/zbm-test-key && zfs create -o encryption=aes-256-gcm -o keyformat=passphrase -o keylocation=file:///run/zbm-test-key -o mountpoint=none zbm_fixture/crypt && zfs unload-key zbm_fixture/crypt && zfs load-key zbm_fixture/crypt && test "$(zfs get -H -o value keystatus zbm_fixture/crypt)" = available && zpool export zbm_fixture"#,
        )?;
        vm::key(run, "r")?;
        let pool = wait_for(20, Some(&mut owned.0), || ready(run, 2, 1))?;
        ensure!(
            pool["state"]["pools"][0]["name"] == "zbm_fixture",
            "Wrong pool: {pool}"
        );
        vm::screenshot(run, &run.join("pool.png"))?;
        let console = vm::console(run)?;
        ensure!(
            console.contains("zbm_fixture") && console.contains("Rescan"),
            "Incomplete pool VT"
        );
        fs::write(run.join("pool.txt"), console)?;
        if lifecycle {
            crate::lifecycle::exercise(run)?;
        }
        let old_pid = manager_pid(run)?;
        vm::key(run, "s")?;
        wait_for(10, Some(&mut owned.0), || {
            ensure!(vm::screen(run)?["event"] == "shell", "Waiting for shell");
            Ok(())
        })?;
        // Type through the same QMP keyboard device as a person at the console.
        for key in ["e", "x", "i", "t", "ret"] {
            vm::key(run, key)?;
        }
        wait_for(10, Some(&mut owned.0), || new_manager(run, old_pid, 1))?;
        vm::key(run, "r")?;
        wait_for(20, Some(&mut owned.0), || ready(run, 2, 1))?;
        vm::screenshot(run, &run.join("shell-return.png"))?;
        fs::write(run.join("shell-return.txt"), vm::console(run)?)?;
        vm::key(run, "p")?;
        wait_for(15, None, || {
            ensure!(
                owned.0.try_wait()?.is_some_and(|s| s.success()),
                "Waiting for guest poweroff"
            );
            Ok(())
        })?;
        fs::write(
            run.join("report.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "passed": true, "firmware": if direct { "direct-linux" } else { "OVMF-UEFI" },
                "lifecycle": lifecycle,
                "empty": empty, "pool": pool,
                "checks": ["boot", "real-zfs-module", "ssh", "snapshot-clone-rename-promote", "native-encryption-local-key", "discovery", "rescan", "QMP-keyboard", "real-vt-rendering", "shell-return", "guest-poweroff"]
            }))?,
        )?;
        Ok(())
    })();
    if let Err(error) = &result {
        eprintln!(
            "Smoke failed: {error:#}. Preserved diagnostics: {}",
            run.display()
        );
    }
    result
}

pub(crate) fn manager_pid(run: &Path) -> Result<u32> {
    Ok(vm::ssh(run, "cat /run/zbm-rs/manager.pid")?
        .trim()
        .parse()?)
}
pub(crate) fn new_manager(run: &Path, previous: u32, pools: usize) -> Result<Value> {
    let state = ready(run, 1, pools)?;
    let current = manager_pid(run)?;
    ensure!(
        current != previous && state["pid"].as_u64() == Some(current as u64),
        "Waiting for a fresh manager"
    );
    ensure!(vm::ssh(run, &format!("test ! -e /proc/{previous}; test -d /proc/1; test /proc/1/exe -ef /proc/{current}/exe"))?.is_empty(), "Unexpected process evidence");
    Ok(state)
}
