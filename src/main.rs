use std::io;

use anyhow::Result;
use clap::Parser;

use install_in_arch::adapters::inbound::cli::{CliArgs, CliDispatcher};
use install_in_arch::adapters::outbound::multi_format_adapter::MultiFormatPackageAdapter;
use install_in_arch::adapters::outbound::pacman_adapter::PacmanAdapter;
use install_in_arch::adapters::outbound::system_process::{
    SystemClock, SystemCommandRunner, SystemPrivilegeDetector,
};
use install_in_arch::domain::ports::{ArchPackageConverterPort, SourcePackageInspectorPort};

fn main() -> Result<()> {
    let args = CliArgs::parse();

    let privilege_detector = SystemPrivilegeDetector;
    let command_runner = SystemCommandRunner;
    let clock = SystemClock;
    let pacman_adapter = PacmanAdapter::new(&command_runner, &privilege_detector);
    let package_adapter = MultiFormatPackageAdapter::new();

    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&package_adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&package_adapter];

    let dispatcher = CliDispatcher::new(
        &inspectors,
        &converters,
        &pacman_adapter,
        &pacman_adapter,
        &clock,
    );

    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    dispatcher.dispatch(args, &mut stdout, &mut stderr)?;

    Ok(())
}
