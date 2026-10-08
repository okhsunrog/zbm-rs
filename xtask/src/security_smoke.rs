//! Actual signed OVMF boot, native verified-input syscall and bounded recovery.
use crate::{
    smoke::{OwnedVm, Swtpm, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum TpmExpected {
    Ready,
    Degraded,
    Unavailable,
    Rejected,
}

pub fn run(
    run: &Path,
    image: &Path,
    vars: &Path,
    port: u16,
    tcg: bool,
    tpm: bool,
    expected: Option<TpmExpected>,
) -> Result<()> {
    crate::smoke::install_interrupt()?;
    let owned_tpm = tpm.then(|| Swtpm::start(run)).transpose()?;
    let mut command = Command::new("/proc/self/exe");
    command
        .args(["vm", "--run"])
        .arg(run)
        .args(["boot", "--image"])
        .arg(image)
        .args(["--secure-vars"])
        .arg(vars)
        .args(["--port", &port.to_string()]);
    if tcg {
        command.arg("--tcg");
    }
    if let Some(tpm) = &owned_tpm {
        command.arg("--tpm-socket").arg(&tpm.socket);
    }
    let mut owned = OwnedVm(command.stdout(Stdio::null()).spawn()?);
    let result = (|| -> Result<()> {
        if matches!(expected, Some(TpmExpected::Rejected)) {
            wait_for(120, Some(&mut owned.0), || {
                ensure!(
                    fs::read_to_string(run.join("serial.log"))?
                        .contains("protected-recovery; shell unavailable"),
                    "Waiting for required TPM rejection"
                );
                Ok(())
            })?;
            // Required setup can fail before the NIC's first link-up. Recovery
            // is already visible while the separate test SSH transport is not.
            let record = wait_for(30, Some(&mut owned.0), || {
                vm::ssh(run, "cat /run/zbm-rs/tpm.json")
            })?;
            let evidence: serde_json::Value = serde_json::from_str(&record)?;
            ensure!(
                evidence["state"] != "ready",
                "Required TPM failure was accepted"
            );
            if tpm {
                ensure!(
                    evidence["srk_ready"] == true
                        && evidence["enter_initrd_measured"] == true
                        && evidence["initialized_nvpcrs"]
                            .as_array()
                            .is_some_and(Vec::is_empty),
                    "Expected isolated signed-policy failure: {evidence}"
                );
                fs::write(
                    run.join("tpm-rejection.log"),
                    vm::ssh(run, "cat /run/zbm-rs/tpm-setup.log")?,
                )?;
            } else {
                ensure!(
                    evidence["state"] == "unavailable" && evidence["srk_ready"] == false,
                    "Expected missing TPM: {evidence}"
                );
            }
            fs::write(run.join("tpm-rejected.json"), record)?;
            ensure!(
                vm::ssh(
                    run,
                    "if test -f /run/zbm-rs/manager.pid; then echo BAD; else echo NO_MANAGER; fi"
                )?
                .contains("NO_MANAGER"),
                "Manager started after mandatory failure"
            );
            vm::screenshot(run, &run.join("protected-tpm-recovery.png"))?;
            vm::key(run, "p")?;
            vm::key(run, "ret")?;
            return wait_for(30, None, || {
                ensure!(
                    owned.0.try_wait()?.is_some_and(|status| status.success()),
                    "Rejected guest did not power off"
                );
                Ok(())
            });
        }
        wait_for(120, Some(&mut owned.0), || {
            let state = vm::screen(run)?;
            ensure!(
                state["event"] == "ready" && state["config"]["security"]["mode"] == "enforce",
                "Waiting for protected manager"
            );
            Ok(())
        })?;
        let readiness = wait_for(30, Some(&mut owned.0), || {
            vm::ssh(run, "cat /run/zbm-rs/security.json")
        })?;
        let evidence: serde_json::Value = serde_json::from_str(&readiness)?;
        ensure!(
            evidence["firmware"] == "enabled" && evidence["ima_policy_loaded"] == true,
            "Mandatory security readiness missing"
        );
        fs::write(run.join("readiness.json"), readiness)?;
        let expected = expected.or(tpm.then_some(TpmExpected::Ready));
        if let Some(expected) = expected {
            let state = match expected {
                TpmExpected::Ready => "ready",
                TpmExpected::Degraded => "degraded",
                TpmExpected::Unavailable => "unavailable",
                TpmExpected::Rejected => unreachable!(),
            };
            ensure!(
                evidence["tpm"]["state"] == state,
                "Unexpected TPM evidence: {}",
                evidence["tpm"]
            );
        }
        if matches!(expected, Some(TpmExpected::Ready)) {
            ensure!(
                evidence["tpm"]["state"] == "ready"
                    && evidence["tpm"]["srk_ready"] == true
                    && evidence["tpm"]["enter_initrd_measured"] == true,
                "TPM/NvPCR readiness failed: {}",
                evidence["tpm"]
            );
            let log = vm::ssh(
                run,
                "cat /run/zbm-rs/tpm.json; cat /run/zbm-rs/tpm-setup.log; cat /run/log/systemd/tpm2-measure.log",
            )?;
            fs::write(run.join("tpm-startup.txt"), log)?;
            fs::write(
                run.join("tpm-firmware.bin"),
                vm::ssh_bytes(
                    run,
                    "cat /sys/kernel/security/tpm0/binary_bios_measurements",
                    30,
                )?,
            )?;
            fs::write(
                run.join("tpm-pcr11.txt"),
                vm::ssh(run, "cat /sys/class/tpm/tpm0/pcr-sha256/11")?,
            )?;
            fs::write(
                run.join("tpm-pcr15-initial.txt"),
                vm::ssh(run, "cat /sys/class/tpm/tpm0/pcr-sha256/15")?,
            )?;
        }
        vm::screenshot(run, &run.join("protected-menu.png"))?;
        let status = vm::ssh(
            run,
            "pid=$(cat /run/zbm-rs/manager.pid); cat /proc/$pid/status",
        )?;
        for name in ["Uid:", "Gid:"] {
            let line = status
                .lines()
                .find(|line| line.starts_with(name))
                .ok_or_else(|| anyhow::anyhow!("Missing {name}"))?;
            ensure!(
                line.split_whitespace()
                    .skip(1)
                    .all(|value| value == "65534"),
                "UI retained privileged IDs"
            );
        }
        for name in ["CapInh:", "CapPrm:", "CapEff:", "CapAmb:"] {
            ensure!(
                status.lines().any(|line| line.starts_with(name)
                    && line.split_whitespace().nth(1) == Some("0000000000000000")),
                "UI retained capabilities"
            );
        }
        ensure!(
            status.lines().any(|line| line.starts_with("NoNewPrivs:")
                && line.split_whitespace().nth(1) == Some("1")),
            "UI can gain privileges"
        );
        fs::write(run.join("ui-status.txt"), status)?;
        for case in [
            "wrong-ima",
            "unsigned-kernel",
            "wrong-cms",
            "wrong-arguments",
            "changed-initramfs",
        ] {
            let log = vm::ssh(
                run,
                &format!(
                    "if /bin/zbm-rs --test-verified-load /fixtures/{case}/plan.json >/run/probe.log 2>&1; then echo UNEXPECTED_ACCEPT; exit 1; fi; cat /run/probe.log"
                ),
            )?;
            fs::write(run.join(format!("{case}.log")), log)?;
        }
        let legacy = vm::ssh(run, "/bin/zbm-rs --test-legacy-kexec")?;
        ensure!(
            legacy.contains("ZBM_TEST_LEGACY_KEXEC_REJECTED"),
            "Legacy syscall bypass"
        );
        fs::write(run.join("legacy.log"), legacy)?;
        let valid = vm::ssh(
            run,
            "/bin/zbm-rs --test-verified-load /fixtures/valid/plan.json",
        )?;
        ensure!(
            valid.contains("ZBM_TEST_VERIFIED_LOAD_PASS"),
            "Valid sealed inputs rejected"
        );
        fs::write(run.join("valid-load.log"), valid)?;
        if matches!(expected, Some(TpmExpected::Ready)) {
            let log = vm::ssh(
                run,
                "cat /run/zbm-rs/tpm-target.json; cat /run/log/systemd/tpm2-measure.log; test ! -f /run/zbm-rs/tpm-target-error",
            )?;
            ensure!(
                log.contains("zbm-rs:target-prepared:v1:"),
                "Prepared target measurement missing"
            );
            fs::write(run.join("tpm-prepared.txt"), log)?;
            fs::write(
                run.join("tpm-userspace.jsonseq"),
                vm::ssh_bytes(run, "cat /run/log/systemd/tpm2-measure.log", 30)?,
            )?;
            fs::write(
                run.join("tpm-target.json"),
                vm::ssh(run, "cat /run/zbm-rs/tpm-target.json")?,
            )?;
            fs::write(
                run.join("tpm-pcr15-prepared.txt"),
                vm::ssh(run, "cat /sys/class/tpm/tpm0/pcr-sha256/15")?,
            )?;
            let verification = Command::new("uv")
                .args([
                    "run",
                    "--no-project",
                    "python",
                    "xtask/fixtures/check_tpm.py",
                ])
                .arg(run)
                .output()?;
            fs::write(
                run.join("tpm-replay.txt"),
                [&verification.stdout[..], &verification.stderr[..]].concat(),
            )?;
            ensure!(
                verification.status.success(),
                "TPM PCR replay failed; inspect tpm-replay.txt"
            );
        }
        let pid = vm::screen(run)?["pid"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Missing manager PID"))?;
        vm::key(run, "f4")?;
        wait_for(20, Some(&mut owned.0), || {
            ensure!(
                vm::console(run)?.contains("does not allow an administrative shell"),
                "Shell refusal is missing"
            );
            ensure!(
                vm::screen(run)?["pid"] == pid,
                "Shell request changed manager"
            );
            Ok(())
        })?;
        // Test-only root control injects crashes; production has no such socket.
        vm::ssh(run, "/bin/zbm-rs --test-send abort")?;
        wait_for(30, Some(&mut owned.0), || {
            let state = vm::screen(run)?;
            ensure!(
                state["event"] == "ready" && state["pid"] != pid,
                "Manager did not restart"
            );
            Ok(())
        })?;
        vm::ssh(run, "/bin/zbm-rs --test-send abort")?;
        wait_for(20, Some(&mut owned.0), || {
            ensure!(
                fs::read_to_string(run.join("serial.log"))?
                    .contains("protected-recovery; shell unavailable"),
                "Protected recovery is missing"
            );
            Ok(())
        })?;
        vm::screenshot(run, &run.join("protected-recovery.png"))?;
        vm::key(run, "m")?;
        vm::key(run, "ret")?;
        wait_for(30, Some(&mut owned.0), || {
            let state = vm::screen(run)?;
            ensure!(
                state["event"] == "ready" && state["pid"] != pid,
                "Protected retry failed"
            );
            vm::ssh(run, "test -f /run/zbm-rs/manager.pid")?;
            Ok(())
        })?;
        let old_id = vm::ssh(run, "cat /proc/sys/kernel/random/boot_id")?;
        // Successful kexec intentionally closes the SSH transport.
        let handoff = vm::ssh(
            run,
            "/bin/zbm-rs --test-verified-load /fixtures/valid/plan.json --execute",
        );
        fs::write(run.join("handoff.txt"), format!("{handoff:?}"))?;
        let serial = wait_for(120, None, || {
            let log = fs::read_to_string(run.join("serial.log"))?;
            ensure!(
                log.contains("ZBM_TEST_SECOND_KERNEL_BOOTED")
                    && log.contains("ZBM_TEST_SECOND_SECURE_BOOT=1"),
                "Waiting for signed second kernel proof"
            );
            let id = log
                .lines()
                .find_map(|line| line.strip_prefix("ZBM_TEST_SECOND_BOOT_ID="))
                .ok_or_else(|| anyhow::anyhow!("Missing second boot ID"))?;
            ensure!(id.trim() != old_id.trim(), "No actual kernel handoff");
            Ok(log)
        })?;
        ensure!(
            serial.contains("root=ZFS=tank/root zbm.fixture=second"),
            "Unexpected final arguments"
        );
        wait_for(30, None, || {
            ensure!(
                owned.0.try_wait()?.is_some_and(|status| status.success()),
                "Guest did not power off cleanly"
            );
            Ok(())
        })?;
        Ok(())
    })();
    fs::write(
        run.join("security-report.json"),
        serde_json::to_vec_pretty(&json!({
            "passed": result.is_ok(), "error": result.as_ref().err().map(|e| format!("{e:#}")),
            "firmware": "OVMF Secure Boot", "scope": "loader and synthetic verified-input handoff; not installed OS or physical boot",
            "swtpm": tpm,
            "expected_tpm": expected.map(|value| format!("{value:?}")),
            "checks": if matches!(expected, Some(TpmExpected::Rejected)) {
                vec!["mandatory-TPM-rejection", "no-manager", "protected-recovery", "clean-poweroff"]
            } else { vec!["signed-policy", "module-enforcement", "pinned-CMS", "IMA-rejection", "unsigned-kernel-rejection", "exact-arguments", "sealed-inputs", "unprivileged-UI", "protected-recovery", "actual-kexec"] }
        }))?,
    )?;
    // OwnedVm reaps on success, failure, panic and interruption.
    std::thread::sleep(Duration::from_millis(10));
    result
}
