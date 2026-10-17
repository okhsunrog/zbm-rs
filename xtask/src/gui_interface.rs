//! LinuxKMS GUI checks use real QMP input and manager telemetry, never VT cells.
use crate::{
    smoke::{manager_pid, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use std::{fs, path::Path};

fn panel(run: &Path, expected: &str) -> Result<()> {
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["panel"] == expected,
            "Waiting for {expected}"
        );
        Ok(())
    })
}

pub fn empty(run: &Path) -> Result<()> {
    let pid = manager_pid(run)?;
    let scans = vm::screen(run)?["state"]["scans"].as_u64().unwrap();
    // Entries -> details -> Actions. The popup starts on its focus scope;
    // Tab reaches Back, then skips disabled snapshot actions to Rescan.
    for key in ["tab", "tab", "ret", "tab", "tab", "down"] {
        vm::key(run, key)?;
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    ensure!(manager_pid(run)? == pid, "Menu navigation invoked recovery");
    ensure!(
        vm::screen(run)?["state"]["scans"] == scans,
        "Menu navigation invoked a command without activation"
    );
    vm::screenshot(run, &run.join("gui-actions.png"))?;
    vm::key(run, "ret")?;
    wait_for(15, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["state"]["scans"].as_u64() == Some(scans + 1)
                && state["state"]["scanning"] == false,
            "Actions menu did not dispatch Rescan"
        );
        Ok(())
    })?;
    vm::key(run, "slash")?;
    for key in ["p", "s", "n", "q"] {
        vm::key(run, key)?;
    }
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["ui"]["filter"] == "psnq",
            "GUI search input missing: {state}"
        );
        ensure!(
            manager_pid(run)? == pid,
            "Typing dispatched a power/recovery shortcut"
        );
        Ok(())
    })?;
    vm::screenshot(run, &run.join("gui-filter.png"))?;
    vm::key(run, "esc")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["filter"] == "",
            "Filter did not clear"
        );
        Ok(())
    })
}

pub fn environments(run: &Path) -> Result<()> {
    // bootfs is row 2; the encrypted fixture is row 1.
    vm::key(run, "up")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["state"]["selected_environment"] == 1,
            "Encrypted BE not selected"
        );
        Ok(())
    })?;
    vm::key(run, "ret")?;
    panel(run, "Unlock")?;
    for key in ["s", "e", "c", "r", "e", "t"] {
        vm::key(run, key)?;
    }
    vm::screenshot(run, &run.join("gui-unlock.png"))?;
    ensure!(
        !fs::read_to_string(run.join("serial.log"))?.contains("secret"),
        "Passphrase leaked into telemetry"
    );
    vm::key(run, "esc")?;
    panel(run, "None")?;
    ensure!(
        vm::screen(run)?["state"]["environments"][1]["properties"]["keystatus"] == "unavailable",
        "Cancel unlocked the dataset"
    );
    vm::ssh(run, "rm /run/zbm-test-key")?;
    vm::key(run, "ret")?;
    panel(run, "Unlock")?;
    for key in ["w", "r", "o", "n", "g", "ret"] {
        vm::key(run, key)?;
    }
    wait_for(15, None, || {
        ensure!(
            vm::screen(run)?["event"] == "unlock-error",
            "Wrong key was not rejected"
        );
        Ok(())
    })?;
    vm::key(run, "ret")?;
    panel(run, "Unlock")?;
    for ch in "disposable-zbm-test-passphrase".chars() {
        vm::key(
            run,
            if ch == '-' {
                "minus".to_string()
            } else {
                ch.to_string()
            }
            .as_str(),
        )?;
    }
    vm::key(run, "ret")?;
    wait_for(20, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["state"]["environments"][1]["properties"]["keystatus"] == "available",
            "GUI did not unlock BE: {state}"
        );
        Ok(())
    })?;
    ensure!(
        !fs::read_to_string(run.join("serial.log"))?.contains("disposable-zbm-test-passphrase"),
        "Unlock key leaked into telemetry"
    );
    // Rediscovery selects bootfs; verify its actual identity before continuing.
    vm::key(run, "end")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["state"]["selected_environment"] == 2,
            "Focus did not return to entries"
        );
        Ok(())
    })?;
    vm::screenshot(run, &run.join("gui-environments.png"))
}
