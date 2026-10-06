//! Real keyboard, VT and size checks for the production TUI on disposable VMs.
use crate::{
    smoke::{manager_pid, wait_for},
    vm,
};
use anyhow::{Result, ensure};
use std::{fs, path::Path};

fn panel(run: &Path, expected: &str) -> Result<()> {
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["ui"]["panel"] == expected,
            "Waiting for panel {expected}: {state}"
        );
        Ok(())
    })
}

pub fn empty(run: &Path) -> Result<()> {
    let pid = manager_pid(run)?;
    vm::key(run, "f1")?;
    panel(run, "Help")?;
    let text = vm::console(run)?;
    ensure!(
        text.contains("Discard prepared clone")
            && text.contains("Exit manager")
            && text.contains("Restart manager"),
        "Help lost existing commands"
    );
    vm::screenshot(run, &run.join("ui-help.png"))?;
    fs::write(run.join("ui-help.txt"), text)?;
    vm::key(run, "esc")?;
    panel(run, "None")?;
    vm::key(run, "f2")?;
    panel(run, "Actions")?;
    vm::screenshot(run, &run.join("ui-actions.png"))?;
    vm::key(run, "esc")?;
    panel(run, "None")?;
    vm::key(run, "slash")?;
    for key in ["p", "s", "n", "q"] {
        vm::key(run, key)?;
    }
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["ui"]["filter"] == "psnq" && state["ui"]["searching"] == true,
            "Typing triggered commands: {state}"
        );
        ensure!(manager_pid(run)? == pid, "Typing replaced the manager");
        Ok(())
    })?;
    // Help remains reachable inside search and does not corrupt its query.
    vm::key(run, "f1")?;
    panel(run, "Help")?;
    vm::key(run, "esc")?;
    panel(run, "None")?;
    vm::key(run, "p")?;
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["ui"]["filter"] == "psnqp" && state["ui"]["searching"] == true,
            "Help lost search focus: {state}"
        );
        ensure!(
            manager_pid(run)? == pid,
            "Continuing search replaced manager"
        );
        Ok(())
    })?;
    vm::key(run, "esc")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["filter"] == "",
            "Filter did not clear"
        );
        Ok(())
    })?;
    Ok(())
}

pub fn environments(run: &Path) -> Result<()> {
    vm::key(run, "f3")?;
    panel(run, "Details")?;
    ensure!(
        vm::console(run)?.contains("Read-only mount"),
        "Missing accurate inspection mode"
    );
    vm::key(run, "esc")?;
    panel(run, "None")?;
    vm::ssh(run, "stty cols 80 rows 25 < /dev/tty1")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["terminal_size"] == serde_json::json!([80, 25]),
            "Waiting for compact console"
        );
        Ok(())
    })?;
    let text = vm::console(run)?;
    ensure!(
        text.contains("Rescan") && text.contains("Shell") && text.contains("Power off"),
        "Compact console hid recovery"
    );
    vm::screenshot(run, &run.join("ui-compact.png"))?;
    fs::write(run.join("ui-compact.txt"), text)?;
    vm::key(run, "f3")?;
    panel(run, "Details")?;
    vm::screenshot(run, &run.join("ui-compact-details.png"))?;
    vm::key(run, "esc")?;
    panel(run, "None")?;
    vm::ssh(run, "stty cols 160 rows 50 < /dev/tty1")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["terminal_size"] == serde_json::json!([160, 50]),
            "Waiting for wide console"
        );
        Ok(())
    })?;
    Ok(())
}

pub fn generations(run: &Path) -> Result<()> {
    let pid = manager_pid(run)?;
    vm::key(run, "up")?;
    wait_for(10, None, || {
        ensure!(
            vm::screen(run)?["ui"]["selected_row"]["Rejected"] == 2,
            "Rejected generation is not inspectable"
        );
        Ok(())
    })?;
    vm::key(run, "ret")?;
    wait_for(10, None, || {
        let state = vm::screen(run)?;
        ensure!(
            state["event"] == "command-disabled" && state["state"]["operation"].is_null(),
            "Rejected generation started boot: {state}"
        );
        ensure!(
            manager_pid(run)? == pid,
            "Rejected generation killed manager"
        );
        Ok(())
    })?;
    vm::screenshot(run, &run.join("ui-rejected-generation.png"))?;
    vm::key(run, "down")?;
    vm::key(run, "f3")?;
    panel(run, "Details")?;
    let text = vm::console(run)?;
    ensure!(
        text.contains("/nix/store/good/kernel")
            && text.contains("/nix/store/good/initrd")
            && text.contains("init=/nix/store/good/init"),
        "Bootspec details are incomplete"
    );
    vm::screenshot(run, &run.join("ui-generation-details.png"))?;
    fs::write(run.join("ui-generation-details.txt"), text)?;
    vm::key(run, "esc")?;
    panel(run, "None")?;
    fs::write(
        run.join("interface-report.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"checks":["full-command-inventory","search-command-isolation","function-key-help","wide-and-80x25-layouts","compact-details","rejected-generation-inspection","rejected-generation-boot-blocked","real-Bootspec-input-preview"]}),
        )?,
    )?;
    Ok(())
}
