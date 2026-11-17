//! Developer validation harness; the native app uses the same asynchronous API.
use anyhow::{Context, Result};
use pokarak_navigation::{InstallPhase, ManagedTools};
use std::{path::PathBuf, thread, time::Duration};
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().context("root argument required")?);
    let tools = ManagedTools::new(root, None)?;
    for package in args {
        let install = tools.begin_install(&package)?;
        let mut previous = String::new();
        loop {
            let progress = install.snapshot();
            let text = format!(
                "{} {:?}: {} bytes / {:?}: {}",
                progress.package,
                progress.phase,
                progress.downloaded_bytes,
                progress.total_bytes,
                progress.message
            );
            if text != previous {
                eprintln!("{text}");
                previous = text;
            }
            if matches!(
                progress.phase,
                InstallPhase::Complete | InstallPhase::Failed | InstallPhase::Cancelled
            ) {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        let result = install.wait()?;
        println!(
            "{} {} {} artifacts",
            result.package,
            result.version,
            result.artifacts.len()
        );
    }
    println!("{}", serde_json::to_string_pretty(&tools.statuses()?)?);
    Ok(())
}
