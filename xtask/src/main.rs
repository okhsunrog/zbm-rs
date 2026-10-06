mod boot_smoke;
mod lifecycle;
mod qmp;
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
            } => vm::boot(&run, &image, tcg, port, direct, fixture.as_deref())?,
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
        } => smoke::run(&run, &image, tcg, port, direct, lifecycle)?,
        Task::BootSmoke {
            run,
            image,
            fixture,
            tcg,
            port,
        } => boot_smoke::run(&run, &image, &fixture, tcg, port)?,
    }
    Ok(())
}
