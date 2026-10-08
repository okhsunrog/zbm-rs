//! Private bounded RPC to a root broker. The UI drops all IDs/privileges before
//! Tokio and never receives a privileged file descriptor or a loaded-kernel handle.
use crate::{
    config::{Config, SecurityMode},
    manager::{
        executor::LoadedKernel,
        operation::{Action, Outcome},
    },
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use zbm_core::{
    Pool, Zfs,
    boot::{BootPlan, BootTarget, TargetDiscovery},
    boot_zfs::{self, Snapshot},
    environment::BootEnvironment,
};

const LIMIT: usize = 1024 * 1024;
const FD_ENV: &str = "ZBM_BROKER_FD";
static CLIENT: OnceLock<Arc<Mutex<UnixStream>>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Request {
    Hello,
    Discover,
    Operation(Box<Action>),
    Execute,
    Unload,
}

#[derive(Serialize, Deserialize)]
enum Reply {
    Ready,
    Pools(Vec<Pool>),
    Environments(Vec<BootEnvironment>),
    Snapshots(Vec<Snapshot>),
    Targets(TargetDiscovery),
    Prepared(BootPlan),
    Discarded(String),
    Boot(BootPlan),
    Done,
}

fn send<T: Serialize>(socket: &mut UnixStream, value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= LIMIT, "Broker message exceeds bounds");
    socket.write_all(&(data.len() as u32).to_be_bytes())?;
    socket.write_all(&data)?;
    Ok(())
}
fn receive<T: serde::de::DeserializeOwned>(socket: &mut UnixStream) -> Result<T> {
    let mut size = [0u8; 4];
    socket.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    ensure!(size > 0 && size <= LIMIT, "Invalid broker message length");
    let mut data = vec![0; size];
    socket.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}
fn call(request: Request) -> Result<Reply> {
    let socket = CLIENT
        .get()
        .ok_or_else(|| anyhow::anyhow!("Privileged broker is unavailable"))?;
    let mut socket = socket
        .lock()
        .map_err(|_| anyhow::anyhow!("Broker connection poisoned"))?;
    let exchange = (|| {
        send(&mut socket, &request)?;
        receive::<std::result::Result<Reply, String>>(&mut socket)
    })();
    // A partial frame or timeout invalidates this stream. Never interpret a late
    // response as acknowledgement of a different privileged operation.
    let reply = match exchange {
        Ok(reply) => reply,
        Err(error) => {
            let _ = socket.shutdown(std::net::Shutdown::Both);
            return Err(error);
        }
    };
    reply.map_err(anyhow::Error::msg)
}
pub fn enabled() -> bool {
    CLIENT.get().is_some()
}

pub fn start() -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Broker must be started before privilege dropping"
    );
    let (parent, child) = UnixStream::pair()?;
    parent.set_read_timeout(Some(Duration::from_secs(30)))?;
    parent.set_write_timeout(Some(Duration::from_secs(2)))?;
    let fd = child.as_raw_fd();
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--broker")
        .env(FD_ENV, fd.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    // The broker inherits the manager's owned process group. PID 1 kills/reaps
    // both after a manager exit, including cancellation and abrupt failure.
    unsafe {
        command.pre_exec(move || crate::session::cloexec(fd, false));
    }
    let child_process = command.spawn()?;
    drop(child);
    CLIENT
        .set(Arc::new(Mutex::new(parent)))
        .map_err(|_| anyhow::anyhow!("Broker already started"))?;
    ensure!(
        matches!(call(Request::Hello)?, Reply::Ready),
        "Broker did not acknowledge startup"
    );
    fs::write("/run/zbm-rs/broker.pid", child_process.id().to_string())?;
    // Diagnostic files are UI-owned; the managed ledger, trust and readiness
    // files remain root-owned. No privileged directory becomes writable by UI.
    for name in ["state.json", "boot-plan.json"] {
        let path = format!("/run/zbm-rs/{name}");
        fs::write(&path, b"{}")?;
        let path = std::ffi::CString::new(path)?;
        ensure!(
            unsafe { libc::chown(path.as_ptr(), 65534, 65534) } == 0,
            "Cannot assign UI diagnostic file"
        );
    }
    ensure!(
        unsafe { libc::prctl(libc::PR_SET_KEEPCAPS, 0, 0, 0, 0) } == 0,
        "Cannot disable retained capabilities"
    );
    ensure!(
        unsafe {
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0,
            )
        } == 0,
        "Cannot clear ambient capabilities"
    );
    ensure!(
        unsafe { libc::setgroups(0, std::ptr::null()) } == 0,
        "Cannot clear supplementary groups"
    );
    ensure!(
        unsafe { libc::setresgid(65534, 65534, 65534) } == 0,
        "Cannot drop group IDs"
    );
    ensure!(
        unsafe { libc::setresuid(65534, 65534, 65534) } == 0,
        "Cannot drop user IDs"
    );
    ensure!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } == 0,
        "Cannot disable privilege gain"
    );
    ensure!(
        unsafe { libc::geteuid() } == 65534,
        "UI retained root privileges"
    );
    ensure!(
        fs::read_to_string("/proc/self/status")?
            .lines()
            .filter(|line| {
                ["CapInh:", "CapPrm:", "CapEff:", "CapAmb:"]
                    .iter()
                    .any(|field| line.starts_with(field))
            })
            .all(|line| line.split_whitespace().nth(1) == Some("0000000000000000")),
        "UI retained process capabilities"
    );
    Ok(())
}

pub async fn discover() -> std::result::Result<Vec<Pool>, String> {
    tokio::task::spawn_blocking(|| match call(Request::Discover)? {
        Reply::Pools(pools) => Ok(pools),
        _ => bail!("Unexpected broker discovery response"),
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}
pub async fn perform(action: Action) -> Result<Outcome> {
    tokio::task::spawn_blocking(move || match call(Request::Operation(Box::new(action)))? {
        Reply::Environments(v) => Ok(Outcome::Environments(v)),
        Reply::Snapshots(v) => Ok(Outcome::Snapshots(v)),
        Reply::Targets(v) => Ok(Outcome::Targets(v)),
        Reply::Prepared(v) => Ok(Outcome::Prepared(v)),
        Reply::Discarded(v) => Ok(Outcome::Discarded(v)),
        Reply::Boot(v) => Ok(Outcome::Boot(v, LoadedKernel::remote())),
        _ => bail!("Unexpected broker operation response"),
    })
    .await?
}
pub fn execute() -> Result<()> {
    call(Request::Execute)?;
    bail!("Broker kexec unexpectedly returned")
}
pub fn unload() {
    let _ = call(Request::Unload);
}

async fn pool(zfs: &Zfs, guid: u64) -> Result<Pool> {
    zbm_core::discover(zfs)
        .await
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .find(|p| p.guid == guid)
        .ok_or_else(|| anyhow::anyhow!("Selected pool GUID is unavailable"))
}
async fn target(zfs: &Zfs, pool: &Pool, wanted: &BootTarget, limit: u32) -> Result<BootTarget> {
    let candidates = if let Some(source) = &wanted.snapshot {
        let snapshot = Snapshot {
            source: source.clone(),
            creation: 0,
        };
        boot_zfs::snapshot_targets(zfs, pool, &snapshot, limit)
            .await?
            .targets
    } else {
        boot_zfs::environments(zfs, pool, false, limit)
            .await?
            .into_iter()
            .flat_map(|be| be.targets)
            .collect()
    };
    candidates
        .into_iter()
        .find(|candidate| {
            candidate.dataset == wanted.dataset
                && candidate.generation == wanted.generation
                && candidate.inputs.kernel == wanted.inputs.kernel
        })
        .ok_or_else(|| {
            anyhow::anyhow!("Target is not in the broker's independently resolved catalog")
        })
}

async fn normalize(action: Action, config: &Config) -> Result<Action> {
    let zfs = Zfs::new();
    let limit = config.nixos.generation_limit;
    let readonly = matches!(
        config.zfs.import_policy,
        crate::config::ImportPolicy::ReadOnly
    );
    Ok(match action {
        Action::Environments { pool: p, .. } => Action::Environments {
            pool: pool(&zfs, p.guid).await?,
            readonly,
            limit,
        },
        Action::Snapshots { pool: p, dataset } => Action::Snapshots {
            pool: pool(&zfs, p.guid).await?,
            dataset,
        },
        Action::Targets {
            pool: p, snapshot, ..
        } => Action::Targets {
            pool: pool(&zfs, p.guid).await?,
            snapshot,
            limit,
        },
        Action::Prepare {
            pool: p,
            target: wanted,
            ..
        } => {
            let p = pool(&zfs, p.guid).await?;
            let t = target(&zfs, &p, &wanted, limit).await?;
            Action::Prepare {
                pool: p,
                target: t,
                args: config.kernel_args.clone(),
            }
        }
        Action::Boot {
            pool: p,
            target: wanted,
            ..
        } => {
            let p = pool(&zfs, p.guid).await?;
            let t = target(&zfs, &p, &wanted, limit).await?;
            Action::Boot {
                pool: p,
                target: t,
                args: config.kernel_args.clone(),
            }
        }
        Action::Discard { pool: p, source } => Action::Discard {
            pool: pool(&zfs, p.guid).await?,
            source,
        },
        Action::Rollback { .. } | Action::Clone { .. } => bail!(
            "Protected mode does not authorize administrative rollback or persistent clone operations"
        ),
    })
}

pub fn run() -> Result<()> {
    ensure!(unsafe { libc::geteuid() } == 0, "Broker requires root");
    let fd: i32 = std::env::var(FD_ENV)?.parse()?;
    ensure!(fd >= 3, "Invalid private broker descriptor");
    unsafe {
        std::env::remove_var(FD_ENV);
    }
    crate::session::cloexec(fd, true)?;
    let mut socket = unsafe { UnixStream::from_raw_fd(fd) };
    let config = crate::config::load(true)?;
    ensure!(
        config.security.mode == SecurityMode::Enforce,
        "Broker requires enforced image policy"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut loaded: Option<LoadedKernel> = None;
    loop {
        let request: Request = match receive(&mut socket) {
            Ok(v) => v,
            Err(_) => break,
        };
        let reply: Result<Reply> = (|| {
            Ok(match request {
                Request::Hello => Reply::Ready,
                Request::Discover => Reply::Pools(
                    runtime
                        .block_on(zbm_core::discover(&Zfs::new()))
                        .map_err(anyhow::Error::msg)?,
                ),
                Request::Operation(action) => {
                    ensure!(
                        loaded.is_none(),
                        "A kernel is already loaded; unload before another operation"
                    );
                    let action = runtime.block_on(normalize(*action, &config))?;
                    match runtime.block_on(action.run_local())? {
                        Outcome::Environments(v) => Reply::Environments(v),
                        Outcome::Snapshots(v) => Reply::Snapshots(v),
                        Outcome::Targets(v) => Reply::Targets(v),
                        Outcome::Prepared(v) => Reply::Prepared(v),
                        Outcome::Discarded(v) => Reply::Discarded(v),
                        Outcome::Boot(plan, kernel) => {
                            loaded = Some(kernel);
                            Reply::Boot(plan)
                        }
                        Outcome::Changed { .. } => {
                            bail!("Administrative operation was not authorized")
                        }
                    }
                }
                Request::Execute => {
                    loaded
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("No verified kernel is loaded"))?
                        .execute()?;
                    Reply::Done
                }
                Request::Unload => {
                    loaded.take();
                    Reply::Done
                }
            })
        })();
        let reply = reply.map_err(|e| format!("{e:#}"));
        if send(&mut socket, &reply).is_err() {
            break;
        }
    }
    drop(loaded);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_frames_reject_oversize_truncation_and_unknown_requests() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        send(&mut sender, &Request::Hello).unwrap();
        assert!(matches!(
            receive::<Request>(&mut receiver).unwrap(),
            Request::Hello
        ));
        sender
            .write_all(&((LIMIT + 1) as u32).to_be_bytes())
            .unwrap();
        assert!(receive::<Request>(&mut receiver).is_err());
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        send(&mut sender, &serde_json::json!({"SkipVerification": true})).unwrap();
        assert!(receive::<Request>(&mut receiver).is_err());
        sender.write_all(&20u32.to_be_bytes()).unwrap();
        sender.write_all(b"x").unwrap();
        drop(sender);
        assert!(receive::<Request>(&mut receiver).is_err());
    }
}
