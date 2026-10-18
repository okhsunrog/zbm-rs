//! Real PID-1 lifecycle acceptance; runs on the disposable Nix SSH/test profile.
use crate::{
    smoke::{manager_pid, new_manager, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

const DIAGNOSTIC_UUID: &str = "ed5d4b72-0aac-48d3-8f89-43362d952ca0";

fn attach_diagnostic_esp(run: &Path) -> Result<()> {
    let disk = vm::absolute(run)?.join("diagnostic-esp.img");
    fs::File::create(&disk)?.set_len(64 * 1024 * 1024)?;
    let mut partition = Command::new("sfdisk")
        .arg(&disk)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    write!(
        partition.stdin.take().unwrap(),
        "label: gpt\nstart=2048,size=126976,type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B,uuid={DIAGNOSTIC_UUID}\n"
    )?;
    ensure!(
        partition.wait()?.success(),
        "Disposable diagnostic partitioning failed"
    );
    ensure!(
        Command::new("mkfs.fat")
            .args(["--offset", "2048"])
            .arg(&disk)
            .arg("63488")
            .stdout(Stdio::null())
            .status()?
            .success(),
        "Diagnostic FAT fixture failed"
    );
    let mut qmp = vm::qmp(run)?;
    qmp.request("blockdev-add", serde_json::json!({"driver":"raw","node-name":"diagnostic-esp", "file":{"driver":"file","filename":disk}}))?;
    qmp.request("device_add", serde_json::json!({"driver":"virtio-blk-pci","bus":"zbm-fixture-slot","drive":"diagnostic-esp","id":"diagnostic-esp","serial":"zbm-disposable-diagnostic-esp"}))?;
    wait_for(10, None, || {
        vm::ssh(
            run,
            &format!("test -b /dev/disk/by-partuuid/{DIAGNOSTIC_UUID}"),
        )?;
        Ok(())
    })
}

pub fn check_persistent_report(run: &Path) -> Result<()> {
    let output = Command::new("mtype")
        .arg("-i")
        .arg(format!(
            "{}@@1048576",
            run.join("diagnostic-esp.img").display()
        ))
        .arg("::/EFI/zbm-rs/last-failure.log")
        .output()?;
    ensure!(
        output.status.success(),
        "Cannot read failure report after guest shutdown"
    );
    let text = String::from_utf8(output.stdout)?;
    ensure!(
        text.contains("failure report")
            && text.contains("supervisor panic caught")
            && text.contains("kernel log"),
        "Incomplete persistent failure report"
    );
    fs::write(run.join("persisted-failure.log"), text)?;
    Ok(())
}

fn send(run: &Path, action: &str) -> Result<()> {
    vm::ssh(run, &format!("/bin/zbm-rs --test-send {action}"))?;
    Ok(())
}
fn emergency(run: &Path) -> Result<()> {
    wait_for(10, None, || {
        ensure!(
            vm::console(run)?.contains("emergency recovery"),
            "Waiting for usable emergency console"
        );
        let baseline = vm::ssh(run, "cat /run/zbm-rs/console-baseline")?;
        ensure!(
            vm::ssh(run, "cat /run/zbm-rs/console-restored")? == baseline,
            "Console state was not restored"
        );
        ensure!(
            baseline.contains("canonical=true") && baseline.contains("echo=true"),
            "Invalid saved console"
        );
        Ok(())
    })
}
fn exit_shell(run: &Path, previous: u32) -> Result<()> {
    for key in ["e", "x", "i", "t", "ret"] {
        vm::key(run, key)?;
    }
    wait_for(10, None, || new_manager(run, previous, 1))?;
    Ok(())
}
fn changed(run: &Path, previous: u32) -> Result<()> {
    wait_for(10, None, || new_manager(run, previous, 1))?;
    Ok(())
}

pub fn exercise(run: &Path) -> Result<()> {
    let first = manager_pid(run)?;
    ensure!(first != 1, "Manager is PID 1");
    vm::ssh(
        run,
        "test /init -ef /bin/zbm-rs; test /proc/1/exe -ef /bin/zbm-rs",
    )?;

    attach_diagnostic_esp(run)?;
    // Reproduce a present device node without usable UART hardware. The
    // readiness event must reach RAM even when the optional serial mirror fails.
    // sshd's own test logfile uses ttyS0 too. Keep one already-open SSH
    // session across replacement, manager restart, RAM readiness and restore.
    let without_uart: u32 = vm::ssh(
        run,
        &format!(
            r#"set -eu
mv /dev/ttyS0 /dev/zbm-test-uart
trap 'rm -f /dev/ttyS0; mv /dev/zbm-test-uart /dev/ttyS0' EXIT
mknod /dev/ttyS0 c 4 95
if echo uart-probe > /dev/ttyS0; then exit 1; fi
/bin/zbm-rs --test-send restart
for attempt in $(seq 1 100); do
    pid=$(cat /run/zbm-rs/manager.pid 2>/dev/null || true)
    if [ -n "$pid" ] && [ "$pid" != "{first}" ] &&
       grep '"event":"ready"' /run/zbm-rs/events.jsonl | grep -q '"pid":'"$pid"','; then
        echo "$pid"
        exit 0
    fi
    sleep 0.1
done
exit 1"#
        ),
    )?
    .trim()
    .parse()?;
    ensure!(
        vm::console(run)?.contains("Rescan"),
        "No usable TUI without UART"
    );
    vm::screenshot(run, &run.join("no-uart.png"))?;
    send(run, "restart")?;
    changed(run, without_uart)?;
    let first = manager_pid(run)?;

    send(run, "probe-fd")?;
    wait_for(5, None, || {
        ensure!(
            vm::ssh(run, "cat /run/zbm-rs/fd-probe")? == "inherited=false",
            "IPC FD inherited across exec"
        );
        Ok(())
    })?;
    send(run, "orphan")?;
    wait_for(5, None, || {
        let pid = vm::ssh(run, "cat /run/zbm-rs/orphan.pid")?;
        let log = vm::ssh(run, "cat /run/zbm-rs/supervisor.log")?;
        ensure!(
            log.contains(&format!("reaped pid={} ", pid.trim())),
            "Orphan not reaped"
        );
        vm::ssh(run, &format!("test ! -e /proc/{}", pid.trim()))?;
        Ok(())
    })?;

    send(run, "dirty-abort")?;
    changed(run, first)?;
    let damage: serde_json::Value =
        serde_json::from_str(&vm::ssh(run, "cat /run/zbm-rs/console-damage.json")?)?;
    ensure!(
        damage["canonical"] == false
            && damage["echo"] == false
            && damage["keyboard"] == 4
            && damage["display"] == 1,
        "Fault did not actually damage the console: {damage}"
    );
    ensure!(
        vm::ssh(run, "cat /run/zbm-rs/console-restored")?
            == vm::ssh(run, "cat /run/zbm-rs/console-baseline")?,
        "Dirty terminal was not restored"
    );
    ensure!(
        vm::console(run)?.contains("Rescan"),
        "Restored TUI not usable"
    );

    let before = manager_pid(run)?;
    vm::key(run, "n")?;
    changed(run, before)?;
    let before = manager_pid(run)?;
    send(run, "leak-restart")?;
    changed(run, before)?;
    vm::ssh(
        run,
        "leak=$(cat /run/zbm-rs/leak.pid); fd=$(cat /run/zbm-rs/leak.fd); test -d /proc/$leak && test -L /proc/$leak/fd/$fd && kill -TERM $leak",
    )?;

    let before = manager_pid(run)?;
    vm::ssh(run, "kill -TERM 1")?;
    changed(run, before)?;
    let before = manager_pid(run)?;
    vm::ssh(run, &format!("kill -INT {before}"))?;
    emergency(run)?;
    exit_shell(run, before)?;

    for signal in ["INT", "HUP", "QUIT"] {
        let before = manager_pid(run)?;
        vm::ssh(run, &format!("kill -{signal} 1"))?;
        changed(run, before)?;
        let before = manager_pid(run)?;
        vm::key(run, "n")?;
        changed(run, before)?;
    }

    let before = manager_pid(run)?;
    send(run, "panic")?;
    changed(run, before)?;
    let before = manager_pid(run)?;
    send(run, "segv")?;
    emergency(run)?;
    exit_shell(run, before)?;

    // A KexecStarting intent that returns is a failed handoff, not a crash loop.
    let before = manager_pid(run)?;
    send(run, "kexec-return")?;
    emergency(run)?;
    exit_shell(run, before)?;

    for fault in ["supervisor-error", "supervisor-panic"] {
        let before = manager_pid(run)?;
        send(run, fault)?;
        emergency(run)?;
        vm::ssh(run, &format!("test -d /proc/1; test ! -e /proc/{before}"))?;
        ensure!(
            vm::console(run)?.contains("last-ditch recovery"),
            "Supervisor failure did not reach last-ditch console"
        );
        // A restored flag alone is insufficient: type a command at the real VT.
        for key in ["e", "c", "h", "o", "spc", "o", "k", "ret"] {
            vm::key(run, key)?;
        }
        wait_for(5, None, || {
            ensure!(
                vm::console(run)?.contains("\nok"),
                "Recovery shell did not execute keyboard input"
            );
            Ok(())
        })?;
        exit_shell(run, before)?;
    }
    let log = vm::ssh(run, "cat /run/zbm-rs/supervisor.log")?;
    ensure!(
        !fs::read_to_string(run.join("serial.log"))?.contains("Attempted to kill init"),
        "Kernel panic killed PID 1"
    );
    ensure!(
        log.contains("forward signal=15")
            && log.contains("crash-loop")
            && log.contains("kexec-returned")
            && log.contains("supervisor error: Deliberate VM supervisor error")
            && log.contains("supervisor panic caught")
            && log.matches("last-ditch-return").count() == 2
            && log.contains("signal=Some(11)"),
        "Incomplete supervisor evidence"
    );
    fs::write(run.join("supervisor.log"), log)?;
    fs::write(
        run.join("lifecycle.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "passed": true, "checks": ["unavailable-UART-not-fatal", "one-ELF", "PID1-survives", "CLOEXEC", "orphan-reaping", "dirty-abort-console-restoration",
            "controlled-restart", "leaked-FD-no-hang", "PID1-SIGTERM-forwarding", "manager-SIGINT", "panic", "SIGSEGV", "crash-loop-recovery", "failed-kexec-classification", "supervisor-error-recovery", "supervisor-panic-recovery", "last-ditch-keyboard"],
            "actual_kexec": "covered by the separate boot-smoke scenario"
        }))?,
    )?;
    Ok(())
}
