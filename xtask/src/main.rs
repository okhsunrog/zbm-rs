mod arch_smoke;
mod boot_smoke;
mod failures;
mod interface;
mod lifecycle;
mod qmp;
mod security_smoke;
mod smoke;
mod vm;

use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::{path::PathBuf, process::Command};

#[derive(Parser)]
#[command(about = "zbm-rs image builder and persistent VM development harness")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}
#[derive(Subcommand)]
enum Task {
    SecureSmoke {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        secure_vars: PathBuf,
        #[arg(long)]
        swtpm: bool,
        #[arg(long, value_enum)]
        expect_tpm: Option<security_smoke::TpmExpected>,
        #[arg(long)]
        tcg: bool,
        #[arg(long, default_value_t = 2277)]
        port: u16,
    },
    /// Boot real Arch roots, snapshot clones and rolled-back datasets.
    ArchSmoke {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long)]
        tcg: bool,
        #[arg(long, default_value_t = 2266)]
        port: u16,
        #[arg(long, value_enum, default_value_t = arch_smoke::Mode::Live)]
        mode: arch_smoke::Mode,
    },
    BootSmoke {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long)]
        tcg: bool,
        #[arg(long, default_value_t = 2230)]
        port: u16,
        #[arg(long)]
        snapshot: bool,
    },
    Image {
        #[arg(long)]
        test_ssh: bool,
    },
    Vm {
        #[arg(long, default_value = "target/vm/dev", global = true)]
        run: PathBuf,
        #[command(subcommand)]
        command: VmTask,
    },
    Smoke {
        #[arg(long)]
        run: PathBuf,
        #[arg(long, default_value = "result-test")]
        image: PathBuf,
        #[arg(long)]
        tcg: bool,
        #[arg(long, default_value_t = 2228)]
        port: u16,
        #[arg(long)]
        direct: bool,
        #[arg(long)]
        lifecycle: bool,
        #[arg(long)]
        failures: bool,
        #[arg(long)]
        scsi: bool,
    },
}
#[derive(Subcommand)]
enum VmTask {
    Boot {
        #[arg(long, default_value = "result-test")]
        image: PathBuf,
        #[arg(long)]
        tcg: bool,
        #[arg(long, default_value_t = 2228)]
        port: u16,
        #[arg(long)]
        direct: bool,
        #[arg(long)]
        fixture: Option<PathBuf>,
        #[arg(long)]
        scsi: bool,
        /// Disposable enrolled OVMF variables; uses the matching Secure Boot firmware.
        #[arg(long)]
        secure_vars: Option<PathBuf>,
        /// Unix control socket of an externally owned disposable swtpm.
        #[arg(long)]
        tpm_socket: Option<PathBuf>,
    },
    Screen {
        #[arg(long)]
        text: bool,
    },
    Status,
    Key {
        key: String,
    },
    Screenshot {
        output: PathBuf,
    },
    Logs,
    Ssh {
        command: String,
    },
    Stop,
}

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    std::env::set_current_dir(root)?;
    match Cli::parse().command {
        Task::SecureSmoke {
            run,
            image,
            secure_vars,
            swtpm,
            expect_tpm,
            tcg,
            port,
        } => security_smoke::run(&run, &image, &secure_vars, port, tcg, swtpm, expect_tpm)?,
        Task::ArchSmoke {
            run,
            image,
            fixture,
            tcg,
            port,
            mode,
        } => arch_smoke::run(&run, &image, &fixture, tcg, port, mode)?,
        Task::Image { test_ssh } => {
            ensure!(
                Command::new("nix")
                    .args([
                        "--extra-experimental-features",
                        "nix-command flakes",
                        "build"
                    ])
                    .arg(if test_ssh {
                        ".#zbm-rs-efi-test"
                    } else {
                        ".#zbm-rs-efi"
                    })
                    .args([
                        "--out-link",
                        if test_ssh { "result-test" } else { "result" }
                    ])
                    .status()?
                    .success(),
                "Image build failed"
            );
            println!("Nix image complete");
        }
        Task::Vm { run, command } => match command {
            VmTask::Boot {
                image,
                tcg,
                port,
                direct,
                fixture,
                scsi,
                secure_vars,
                tpm_socket,
            } => vm::boot(
                &run,
                vm::BootOptions {
                    image: &image,
                    tcg,
                    port,
                    direct,
                    fixture: fixture.as_deref(),
                    scsi,
                    secure_vars: secure_vars.as_deref(),
                    tpm_socket: tpm_socket.as_deref(),
                },
            )?,
            VmTask::Screen { text } => {
                if text {
                    print!("{}", vm::console(&run)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&vm::screen(&run)?)?);
                }
            }
            VmTask::Status => println!("{}", vm::qmp(&run)?.request("query-status", json!({}))?),
            VmTask::Key { key } => vm::key(&run, &key)?,
            VmTask::Screenshot { output } => vm::screenshot(&run, &output)?,
            VmTask::Logs => {
                for name in ["qemu.log", "serial.log"] {
                    println!("{name}:\n{}", std::fs::read_to_string(run.join(name))?);
                }
            }
            VmTask::Ssh { command } => print!("{}", vm::ssh(&run, &command)?),
            VmTask::Stop => {
                vm::qmp(&run)?.request("quit", json!({}))?;
            }
        },
        Task::Smoke {
            run,
            image,
            tcg,
            port,
            direct,
            lifecycle,
            failures,
            scsi,
        } => smoke::run(
            &run,
            &image,
            tcg,
            port,
            direct,
            smoke::Scenarios {
                lifecycle,
                failures,
                scsi,
            },
        )?,
        Task::BootSmoke {
            run,
            image,
            fixture,
            tcg,
            port,
            snapshot,
        } => boot_smoke::run(&run, &image, &fixture, tcg, port, snapshot)?,
    }
    Ok(())
}
