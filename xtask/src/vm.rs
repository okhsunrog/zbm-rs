use crate::qmp::Qmp;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    })
}

pub struct BootOptions<'a> {
    pub image: &'a Path,
    pub tcg: bool,
    pub port: u16,
    pub direct: bool,
    pub fixture: Option<&'a Path>,
    pub scsi: bool,
    pub secure_vars: Option<&'a Path>,
    pub tpm_socket: Option<&'a Path>,
}

pub fn boot(run: &Path, options: BootOptions<'_>) -> Result<()> {
    let BootOptions {
        image,
        tcg,
        port,
        direct,
        fixture,
        scsi,
        secure_vars,
        tpm_socket,
    } = options;
    let run = absolute(run)?;
    let image = image
        .canonicalize()
        .context("Build the image first with cargo xtask image")?;
    fs::create_dir_all(run.parent().context("Run directory needs a parent")?)?;
    fs::create_dir(&run).context("Use a fresh --run directory; previous runs are retained")?;
    ensure!(
        run.join("qmp.sock").as_os_str().len() < 104,
        "Run path too long for a Unix socket"
    );
    ensure!(
        !image.to_string_lossy().contains(','),
        "Image path cannot contain a QEMU option separator"
    );
    ensure!(
        !run.to_string_lossy().contains(','),
        "Run path cannot contain a QEMU option separator"
    );
    let (ordinary_code, ovmf_vars) = firmware()?;
    ensure!(
        !direct || secure_vars.is_none(),
        "Secure Boot requires the EFI boot path"
    );
    let ovmf_code = if secure_vars.is_some() {
        [
            "/usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd",
            "/usr/share/OVMF/OVMF_CODE_4M.secboot.fd",
        ]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or_else(|| anyhow::anyhow!("No supported 4M Secure Boot firmware found"))?
    } else {
        ordinary_code
    };
    ensure!(
        Path::new(ovmf_code).is_file(),
        "Matching Secure Boot firmware is unavailable"
    );
    if let Some(vars) = secure_vars {
        ensure!(
            fs::metadata(vars)?.len() == fs::metadata(ovmf_vars)?.len(),
            "Secure OVMF CODE/VARS size mismatch"
        );
    }
    fs::copy(
        secure_vars.unwrap_or(Path::new(ovmf_vars)),
        run.join("OVMF_VARS.fd"),
    )?;
    fs::copy(image.join("manifest.json"), run.join("image-manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&fs::read(image.join("manifest.json"))?)?;
    if manifest["test_ssh"] == true {
        fs::copy(image.join("id_ed25519"), run.join("id_ed25519"))?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(run.join("id_ed25519"), fs::Permissions::from_mode(0o600))?;
    }
    ensure!(
        Command::new("qemu-img")
            .args(["create", "-f", "qcow2"])
            .arg(run.join("disk.qcow2"))
            .arg("2G")
            .status()?
            .success(),
        "qemu-img failed"
    );
    fs::write(
        run.join("connection.json"),
        serde_json::to_vec_pretty(
            &json!({"ssh_port": port, "test_ssh": manifest["test_ssh"], "direct": direct, "disk_bus": if scsi { "scsi" } else { "virtio-blk" }}),
        )?,
    )?;
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args([
        "-machine",
        if tcg && secure_vars.is_some() {
            "q35,accel=tcg,smm=on"
        } else if !tcg && secure_vars.is_some() {
            "q35,accel=kvm,smm=on"
        } else if tcg {
            "q35,accel=tcg"
        } else {
            "q35,accel=kvm"
        },
        "-cpu",
        if tcg { "max" } else { "host" },
        "-m",
        if fixture.is_some() { "2048" } else { "1024" },
        "-smp",
        "2",
        "-vga",
        "virtio",
        "-display",
        "none",
        "-no-reboot",
        "-qmp",
        &format!("unix:{},server=on,wait=off", run.join("qmp.sock").display()),
        "-serial",
        &format!("file:{}", run.join("serial.log").display()),
        "-drive",
        &format!(
            "file={},format=qcow2,if=none,id=zbm-root",
            run.join("disk.qcow2").display()
        ),
    ]);
    if secure_vars.is_some() {
        cmd.args(["-global", "driver=cfi.pflash01,property=secure,value=on"]);
    }
    if let Some(socket) = tpm_socket {
        let socket = socket.canonicalize()?;
        ensure!(
            !socket.to_string_lossy().contains(','),
            "Invalid TPM socket path"
        );
        cmd.args([
            "-chardev",
            &format!("socket,id=zbm-tpm,path={}", socket.display()),
            "-tpmdev",
            "emulator,id=zbm-tpmdev,chardev=zbm-tpm",
            "-device",
            "tpm-crb,tpmdev=zbm-tpmdev",
            // Disposable SMBIOS identity for deterministic hardware measurements.
            "-uuid",
            "01234567-89ab-cdef-0123-456789abcdef",
        ]);
    }
    if scsi {
        cmd.args([
            "-device",
            "virtio-scsi-pci,id=scsi0",
            "-device",
            "scsi-hd,drive=zbm-root,bus=scsi0.0,serial=zbm-fixture-disk",
        ]);
    } else {
        cmd.args([
            "-device",
            "virtio-blk-pci,drive=zbm-root,serial=zbm-fixture-disk",
        ]);
    }
    if let Some(fixture) = fixture {
        let archive = fixture.join("root.tar").canonicalize()?;
        ensure!(
            archive.is_file() && !archive.to_string_lossy().contains(','),
            "Invalid generated fixture archive"
        );
        cmd.args([
            "-drive",
            &format!(
                "file={},format=raw,if=virtio,readonly=on",
                archive.display()
            ),
        ]);
    }
    if direct {
        cmd.arg("-kernel")
            .arg(image.join("vmlinuz"))
            .arg("-initrd")
            .arg(image.join("initramfs.img"))
            .arg("-append")
            .arg(fs::read_to_string(image.join("cmdline"))?.trim());
    } else {
        ensure!(
            Command::new("cp")
                .arg("-r")
                .arg(image.join("esp"))
                .arg(run.join("esp"))
                .status()?
                .success(),
            "ESP copy failed"
        );
        cmd.args([
            "-drive",
            &format!("if=pflash,format=raw,unit=0,readonly=on,file={ovmf_code}"),
        ])
        .args([
            "-drive",
            &format!(
                "if=pflash,format=raw,unit=1,file={}",
                run.join("OVMF_VARS.fd").display()
            ),
        ])
        .args([
            "-drive",
            &format!("format=raw,file=fat:rw:{}", run.join("esp").display()),
        ]);
    }
    if manifest["test_ssh"] == true {
        cmd.args([
            "-nic",
            &format!("user,model=e1000,hostfwd=tcp:127.0.0.1:{port}-:22"),
        ]);
    } else {
        cmd.args(["-nic", "none"]);
    }
    fs::write(run.join("command.txt"), format!("{cmd:?}\n"))?;
    let log = fs::File::create(run.join("qemu.log"))?;
    cmd.stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    println!(
        "VM run: {} (QEMU owns this foreground process)",
        run.display()
    );
    // Replace this process, so the caller's tracked session owns the real VM.
    Err(cmd.exec().into())
}

fn firmware() -> Result<(&'static str, &'static str)> {
    for pair in [
        (
            "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
            "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE_4M.fd",
            "/usr/share/OVMF/OVMF_VARS_4M.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
        ),
    ] {
        if Path::new(pair.0).exists() && Path::new(pair.1).exists() {
            return Ok(pair);
        }
    }
    bail!("No matched OVMF CODE/VARS pair found; install edk2-ovmf or ovmf")
}

pub fn qmp(run: &Path) -> Result<Qmp> {
    Qmp::connect(&absolute(run)?.join("qmp.sock"))
}

pub fn key(run: &Path, key: &str) -> Result<()> {
    let keys: Vec<Value> = key
        .split('-')
        .map(|code| json!({"type": "qcode", "data": code}))
        .collect();
    qmp(run)?.request("send-key", json!({"keys": keys, "hold-time": 80}))?;
    // QMP acknowledges before key-up. Do not overlap successive keystrokes.
    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(())
}

pub fn screenshot(run: &Path, output: &Path) -> Result<()> {
    let output = absolute(output)?;
    fs::create_dir_all(output.parent().context("Screenshot needs parent")?)?;
    let mut qmp = qmp(run)?;
    // A blinking console cursor prevents pixel equality even on a settled page.
    // Freeze guest updates for capture and restore only our own pause afterward.
    let running = qmp.request("query-status", json!({}))?["running"] == true;
    if running {
        qmp.request("stop", json!({}))?;
    }
    let result = capture_screen(&mut qmp, &output);
    let resume = if running {
        qmp.request("cont", json!({})).map(|_| ())
    } else {
        Ok(())
    };
    result?;
    resume
}

fn capture_screen(qmp: &mut Qmp, output: &Path) -> Result<()> {
    let mut previous = Vec::new();
    let mut stable = 0;
    // Guest console writes and VGA refresh are separate from serial readiness.
    // Capture a settled display rather than a partially updated surface.
    for _ in 0..12 {
        qmp.request("screendump", json!({"filename": output, "format": "png"}))?;
        let current = fs::read(output)?;
        if current == previous {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable >= 3 {
            return Ok(());
        }
        previous = current;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    bail!(
        "Display did not settle; last screenshot preserved at {}",
        output.display()
    )
}

/// Latest *observed* application state, not an OCR substitute for screenshots.
pub fn screen(run: &Path) -> Result<Value> {
    let log = fs::read_to_string(run.join("serial.log"))?;
    log.lines()
        .rev()
        .find_map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .filter(|v| v.get("event").is_some())
        })
        .context("No TUI state yet; inspect logs or screenshot")
}

pub fn ssh(run: &Path, command: &str) -> Result<String> {
    ssh_timeout(run, command, 30)
}
pub fn ssh_timeout(run: &Path, command: &str, seconds: u32) -> Result<String> {
    Ok(String::from_utf8(ssh_bytes(run, command, seconds)?)?)
}
pub fn ssh_bytes(run: &Path, command: &str, seconds: u32) -> Result<Vec<u8>> {
    let info: Value = serde_json::from_slice(&fs::read(run.join("connection.json"))?)?;
    if info["test_ssh"] != true {
        bail!("SSH unavailable: rebuild with cargo xtask image --test-ssh");
    }
    let bounded = format!(
        "timeout {seconds} /bin/sh -c '{}'",
        command.replace('\'', "'\\''")
    );
    let output = Command::new("timeout")
        .args(["--kill-after=2", &(seconds + 5).to_string(), "ssh"])
        .args([
            "-F",
            "/dev/null",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "IdentitiesOnly=yes",
        ])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            absolute(run)?.join("known_hosts").display()
        ))
        .arg("-i")
        .arg(run.join("id_ed25519"))
        .arg("-p")
        .arg(
            info["ssh_port"]
                .as_u64()
                .context("Invalid port")?
                .to_string(),
        )
        .args(["root@127.0.0.1", &bounded])
        .output()?;
    ensure!(
        output.status.success(),
        "Guest SSH failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

/// Read the actual Linux VT, independent of application instrumentation and GPU.
pub fn console(run: &Path) -> Result<String> {
    let output = ssh(run, "stty -F /dev/tty1 size; od -An -v -tu1 /dev/vcs1")?;
    let mut lines = output.lines();
    let geometry: Vec<usize> = lines
        .next()
        .context("No VT geometry")?
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    ensure!(
        geometry.len() == 2 && geometry[1] > 0,
        "Invalid VT geometry"
    );
    let cells: Vec<u8> = lines
        .flat_map(str::split_whitespace)
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    ensure!(
        cells.len() == geometry[0] * geometry[1],
        "Incomplete VT capture"
    );
    Ok(cells
        .chunks(geometry[1])
        .map(|row| {
            let row: String = row
                .iter()
                .map(|&c| {
                    if (32..127).contains(&c) {
                        c as char
                    } else {
                        ' '
                    }
                })
                .collect();
            format!("{}\n", row.trim_end())
        })
        .collect())
}
