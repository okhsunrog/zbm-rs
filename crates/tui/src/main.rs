mod broker;
mod config;
#[cfg(feature = "gui-prototype")]
mod gui;
mod manager;
mod security;
mod session;
mod supervisor;
mod tpm;
#[cfg(feature = "vm-test")]
mod vm_test;

fn main() -> anyhow::Result<()> {
    // This branch is before argument parsing, Tokio, Ratatui or ZFS initialization.
    if std::process::id() == 1 {
        supervisor::run();
    }
    let channel = session::Client::from_environment()?;
    #[cfg(feature = "vm-test")]
    if vm_test::utility()? {
        return Ok(());
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--nixos-plans") {
        anyhow::ensure!(
            args.len() == 5,
            "usage: zbm-rs --nixos-plans ROOT DATASET IMAGE_CONFIG TOPLEVEL"
        );
        let root = std::path::Path::new(&args[1]).canonicalize()?;
        let dataset = args[2]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Dataset must be UTF-8"))?;
        let config = config::parse(&std::fs::read_to_string(&args[3])?)?;
        let target =
            zbm_core::boot::nixos_target(&root, dataset, std::path::Path::new(&args[4]), 0)?;
        let plan = zbm_core::boot::BootPlan::resolve_verified(&target, &config.kernel_args)?;
        println!("{}", serde_json::to_string(&[plan])?);
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--linux-plans") {
        anyhow::ensure!(
            args.len() == 4,
            "usage: zbm-rs --linux-plans ROOT DATASET IMAGE_CONFIG"
        );
        let root = std::path::Path::new(&args[1]).canonicalize()?;
        let dataset = args[2]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Dataset must be UTF-8"))?;
        let config = config::parse(&std::fs::read_to_string(&args[3])?)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let properties = runtime.block_on(zbm_core::boot_zfs::properties(
            &zbm_core::Zfs::new(),
            dataset,
        ))?;
        let discovery = zbm_core::linux::discover(&root, dataset, &properties)?;
        let plans = discovery
            .targets
            .iter()
            .map(|target| zbm_core::boot::BootPlan::resolve_verified(target, &config.kernel_args))
            .collect::<std::io::Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string(&plans)?);
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--broker") {
        anyhow::ensure!(args.len() == 1, "Invalid broker arguments");
        return broker::run();
    }
    if args.first().is_some_and(|arg| arg == "--validate-config") {
        anyhow::ensure!(args.len() == 2, "usage: zbm-rs --validate-config PATH");
        let input = std::fs::read_to_string(&args[1])?;
        let config = config::parse(&input)?;
        println!("{}", serde_json::to_string(&config)?);
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--print-config") {
        anyhow::ensure!(args.len() == 1, "usage: zbm-rs --print-config");
        println!(
            "{}",
            serde_json::to_string(&config::load(channel.is_some())?)?
        );
        return Ok(());
    }
    #[cfg(feature = "gui-prototype")]
    return gui::run();
    #[cfg(not(feature = "gui-prototype"))]
    manager::run(channel)
}
