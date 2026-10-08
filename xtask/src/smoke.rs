//! Reusable full boot scenario. Drop always terminates and reaps the owned VM.
use crate::vm;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(crate) struct OwnedVm(pub(crate) Child);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
pub(crate) fn install_interrupt() -> Result<()> {
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed))?;
    Ok(())
}
impl Drop for OwnedVm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) struct Swtpm {
    pub socket: std::path::PathBuf,
    _process: OwnedVm,
}
impl Swtpm {
    pub fn start(run: &Path) -> Result<Self> {
        let state = vm::absolute(run)?.with_extension("tpm");
        fs::create_dir_all(state.parent().context("TPM state needs a parent")?)?;
        fs::create_dir(&state)?;
        let log = fs::File::create(state.join("swtpm.log"))?;
        let socket = state.join("control.sock");
        let mut process = OwnedVm(
            Command::new("swtpm")
                .args(["socket", "--tpm2", "--flags", "not-need-init"])
                .arg("--tpmstate")
                .arg(format!("dir={}", state.display()))
                .arg("--ctrl")
                .arg(format!("type=unixio,path={}", socket.display()))
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?,
        );
        wait_for(10, Some(&mut process.0), || {
            ensure!(socket.exists(), "Waiting for disposable TPM");
            Ok(())
        })?;
        Ok(Self {
            socket,
            _process: process,
        })
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

pub struct Scenarios {
    pub lifecycle: bool,
    pub failures: bool,
    pub scsi: bool,
}

pub fn run(
    run: &Path,
    image: &Path,
    tcg: bool,
    port: u16,
    direct: bool,
    scenarios: Scenarios,
) -> Result<()> {
    let Scenarios {
        lifecycle,
        failures,
        scsi,
    } = scenarios;
    install_interrupt()?;
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
    if scsi {
        command.arg("--scsi");
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
                "uname -r; test -c /dev/zfs; test -L /etc/zbm-rs/config.json; test -r /etc/zbm-rs/config.json; test -r /etc/udev/rules.d/80-drivers.rules",
            )
        })?;
        let console = vm::console(run)?;
        ensure!(
            console.contains("Rescan") && console.contains("Power off"),
            "Missing VT controls"
        );
        fs::write(run.join("empty.txt"), console)?;
        crate::interface::empty(run)?;
        // Disposable guest disk only; never attach host devices to this scenario.
        let disk = vm::ssh(
            run,
            r#"for path in /dev/disk/by-id/*zbm-fixture-disk; do [ -L "$path" ] && readlink -f "$path"; done | sort -u"#,
        )?;
        let disk = disk.trim();
        ensure!(
            disk.starts_with("/dev/")
                && disk
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-".contains(&b)),
            "Expected one disposable disk with fixture serial, found: {disk:?}"
        );
        if scsi {
            ensure!(disk.starts_with("/dev/sd"), "Expected a SCSI disk: {disk}");
        }
        let provision = r#"zpool create -f -o cachefile=none -m none zbm_fixture DISPOSABLE_DISK && zfs create -o mountpoint=none zbm_fixture/ROOT && zfs snapshot zbm_fixture/ROOT@fresh && zfs clone -o mountpoint=none zbm_fixture/ROOT@fresh zbm_fixture/clone && zfs rename zbm_fixture/clone zbm_fixture/renamed && zfs promote zbm_fixture/renamed && printf '%s\n' disposable-zbm-test-passphrase > /run/zbm-test-key && zfs create -o encryption=aes-256-gcm -o keyformat=passphrase -o keylocation=file:///run/zbm-test-key -o mountpoint=none zbm_fixture/crypt && zfs unload-key zbm_fixture/crypt && zfs load-key zbm_fixture/crypt && test "$(zfs get -H -o value keystatus zbm_fixture/crypt)" = available && zpool export zbm_fixture"#;
        vm::ssh(run, &provision.replace("DISPOSABLE_DISK", disk))?;
        vm::key(run, "r")?;
        let pool = wait_for(20, Some(&mut owned.0), || ready(run, 2, 1))?;
        ensure!(
            pool["state"]["pools"][0]["name"] == "zbm_fixture",
            "Wrong pool: {pool}"
        );
        // BE policy fixture: inherited active, hidden roots, explicit legacy,
        // non-root filesystems and an encrypted candidate with a useful diagnostic.
        vm::ssh(
            run,
            "set -eu; zpool import -N -o cachefile=none zbm_fixture; zfs set canmount=noauto mountpoint=/ zbm_fixture/ROOT; zfs set org.zfsbootmenu:active=on zbm_fixture; zfs create -o canmount=noauto -o mountpoint=legacy zbm_fixture/default; zfs create -o canmount=noauto -o mountpoint=/ -o org.zfsbootmenu:active=off zbm_fixture/hidden; zfs create -o canmount=noauto -o mountpoint=/home zbm_fixture/data; zfs set canmount=noauto mountpoint=/ zbm_fixture/crypt; zpool set bootfs=zbm_fixture/default zbm_fixture; zpool export zbm_fixture",
        )?;
        vm::key(run, "ret")?;
        wait_for(30, None, || {
            let state = vm::screen(run)?;
            ensure!(
                state["event"] == "boot-environments" && state["state"]["error"].is_null(),
                "Waiting for boot environments: {state}"
            );
            let environments = state["state"]["environments"].as_array().unwrap();
            let names: Vec<_> = environments
                .iter()
                .map(|be| be["dataset"].as_str().unwrap())
                .collect();
            ensure!(
                names
                    == [
                        "zbm_fixture/ROOT",
                        "zbm_fixture/crypt",
                        "zbm_fixture/default"
                    ],
                "Wrong BE visibility: {names:?}"
            );
            ensure!(
                state["state"]["selected_environment"] == 2
                    && environments[2]["is_default"] == true,
                "bootfs was not selected"
            );
            ensure!(
                environments[1]["unavailable"]
                    .as_str()
                    .is_some_and(|e| e.contains("Encrypted")),
                "Missing encrypted-root diagnostic"
            );
            fs::write(
                run.join("environments.json"),
                serde_json::to_vec_pretty(&state)?,
            )?;
            Ok(())
        })?;
        vm::screenshot(run, &run.join("environments.png"))?;
        fs::write(run.join("environments.txt"), vm::console(run)?)?;
        crate::interface::environments(run)?;
        vm::ssh(
            run,
            r#"test "$(zpool get -H -o value readonly zbm_fixture)" = on; test "$(awk 'index($5, "/run/zbm-rs/roots/") == 1 {n++} END {print n}' /proc/self/mountinfo)" = 2"#,
        )?;
        vm::key(run, "b")?;
        vm::screenshot(run, &run.join("pool.png"))?;
        let console = vm::console(run)?;
        ensure!(
            console.contains("zbm_fixture") && console.contains("Rescan"),
            "Incomplete pool VT"
        );
        fs::write(run.join("pool.txt"), console)?;
        if failures {
            crate::failures::exercise(run)?;
        }
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
        if failures {
            crate::failures::start_hung_mount(run)?;
        }
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
                "failures": failures, "disk_bus": if scsi { "scsi" } else { "virtio-blk" },
                "empty": empty, "pool": pool,
                "checks": ["boot", "real-zfs-module", "ssh", "snapshot-clone-rename-promote", "native-encryption-local-key", "discovery", "BE-visibility", "inherited-active", "bootfs-selection", "encrypted-BE-diagnostic", "read-only-candidate-mounts", "rescan", "QMP-keyboard", "real-vt-rendering", "shell-return", "guest-poweroff"]
            }))?,
        )?;
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = vm::screenshot(run, &run.join("failure.png"));
        if let Ok(console) = vm::console(run) {
            let _ = fs::write(run.join("failure.txt"), console);
        }
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
