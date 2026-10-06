mod config;
mod manager;
mod session;
mod supervisor;
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
    manager::run(channel)
}
