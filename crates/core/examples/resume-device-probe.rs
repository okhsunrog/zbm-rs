//! Disposable-VM harness for the resume device API. Not installed in images.
use std::{env, fs, io};
use zbm_core::{
    boot::BootPlan,
    resume::{device::Device, linux::Swap},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 2 {
        return Err(io::Error::other(
            "usage: resume-device-probe SPEC_JSON status|record|verify|cancel|restore [ARG]",
        )
        .into());
    }
    let device: Device = serde_json::from_slice(&fs::read(&args[0])?)?;
    let write = matches!(args[1].as_str(), "record" | "cancel");
    let swap = Swap::open_device(&device, write)?;
    match args[1].as_str() {
        "status" if args.len() == 2 => println!("{}", serde_json::to_string(&swap.status()?)?),
        "record" | "verify" if args.len() == 3 => {
            let plan: BootPlan = serde_json::from_slice(&fs::read(&args[2])?)?;
            if write {
                swap.record(42, &plan)?;
            } else {
                swap.verify_handoff(42, &plan)?;
            }
        }
        "cancel" if args.len() == 3 => swap.cancel(&args[2])?,
        "restore" if args.len() == 2 => swap.restore()?,
        _ => return Err(io::Error::other("Invalid probe operation").into()),
    }
    Ok(())
}
