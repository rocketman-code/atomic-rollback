//! CLI entry point. Parses arguments into Command/SnapshotCommand enums,
//! then dispatches.

mod check;
mod consts;
mod grub;
mod kernel_hook;
mod migrate;
mod parse;
mod platform;
#[cfg(any(test, kani))]
mod proof;
mod rollback;
mod snapshot;
mod swap;
mod tools;

use std::path::Path;

// --- Command types ---

enum Command {
    Check { root: String },
    Setup,
    Migrate,
    Snapshot(SnapshotCommand),
    Rollback { name: String },
    KernelHook { command: String, kver: String },
}

enum SnapshotCommand {
    Create { name: Option<String> },
    List,
    Delete { name: String },
}

// --- Help text ---

fn print_help() {
    eprintln!("atomic-rollback: atomic system rollback for Fedora via Btrfs subvolume swap");
    eprintln!();
    eprintln!("usage:");
    eprintln!("  atomic-rollback check                verify the system is bootable");
    eprintln!("  atomic-rollback setup                separate /var, enable snapshots and rollback");
    eprintln!("  atomic-rollback migrate              full boot migration for complete kernel rollback");
    eprintln!("  atomic-rollback snapshot             create snapshot (default name)");
    eprintln!("  atomic-rollback snapshot create [N]  create snapshot with optional name");
    eprintln!("  atomic-rollback snapshot list        show available snapshots");
    eprintln!("  atomic-rollback snapshot delete <N>  delete a snapshot by name");
    eprintln!("  atomic-rollback rollback [name]      roll back to a snapshot");
}

fn print_snapshot_help() {
    eprintln!("usage:");
    eprintln!("  atomic-rollback snapshot             create snapshot (default name)");
    eprintln!("  atomic-rollback snapshot create [N]  create snapshot with optional name");
    eprintln!("  atomic-rollback snapshot list        show available snapshots");
    eprintln!("  atomic-rollback snapshot delete <N>  delete a snapshot by name");
}

// --- Parsing ---

fn is_help(arg: Option<&String>) -> bool {
    arg.is_some_and(|a| a == "--help" || a == "-h")
}

fn parse_snapshot(args: &[String]) -> SnapshotCommand {
    if is_help(args.get(2)) {
        print_snapshot_help();
        std::process::exit(0);
    }
    match args.get(2).map(|s| s.as_str()) {
        Some("create") => {
            if is_help(args.get(3)) { print_snapshot_help(); std::process::exit(0); }
            SnapshotCommand::Create { name: args.get(3).cloned() }
        }
        Some("list") => SnapshotCommand::List,
        Some("delete") => {
            if is_help(args.get(3)) { print_snapshot_help(); std::process::exit(0); }
            match args.get(3) {
                Some(name) => SnapshotCommand::Delete { name: name.clone() },
                None => {
                    eprintln!("Usage: atomic-rollback snapshot delete <name>");
                    std::process::exit(2);
                }
            }
        }
        None => SnapshotCommand::Create { name: None },
        Some(unknown) => {
            eprintln!("Unknown snapshot command '{unknown}'.");
            eprintln!();
            print_snapshot_help();
            std::process::exit(2);
        }
    }
}

fn parse_args(args: &[String]) -> Command {
    if args.len() < 2 || is_help(args.get(1)) {
        print_help();
        std::process::exit(if args.len() < 2 { 2 } else { 0 });
    }

    match args[1].as_str() {
        "check" => {
            if is_help(args.get(2)) { print_help(); std::process::exit(0); }
            Command::Check { root: args.get(2).cloned().unwrap_or_else(|| "/".into()) }
        }
        "setup" => Command::Setup,
        "migrate" => Command::Migrate,
        "snapshot" => Command::Snapshot(parse_snapshot(args)),
        "rollback" => {
            if is_help(args.get(2)) { print_help(); std::process::exit(0); }
            Command::Rollback {
                name: args.get(2).cloned().unwrap_or_else(|| consts::DEFAULT_SNAPSHOT_NAME.into()),
            }
        }
        "kernel-hook" => {
            // kernel-install calls hooks for events we may not handle.
            // Missing args = nothing to do.
            let command = args.get(2).cloned().unwrap_or_default();
            let kver = args.get(3).cloned().unwrap_or_default();
            if command.is_empty() || kver.is_empty() {
                std::process::exit(0);
            }
            Command::KernelHook { command, kver }
        }
        unknown => {
            eprintln!("Unknown command '{unknown}'.");
            eprintln!();
            print_help();
            std::process::exit(2);
        }
    }
}

// --- Dispatch ---

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match parse_args(&args) {
        Command::Check { root } => {
            println!("atomic-rollback: checking system bootability\n");
            match check::verify_bootable(Path::new(&root)) {
                check::BootStatus::Pass => {
                    println!("\nSystem is bootable.\n");
                    if let Ok((_, fstab)) = tools::root_device() {
                        check::print_rollback_scope(&fstab);
                    }
                }
                check::BootStatus::Warn => {
                    println!("\nSystem is bootable (with warnings).\n");
                    if let Ok((_, fstab)) = tools::root_device() {
                        check::print_rollback_scope(&fstab);
                    }
                    std::process::exit(2);
                }
                check::BootStatus::Fail(failures) => {
                    for f in &failures { eprintln!("  {f}"); }
                    println!("\nSystem has boot problems.");
                    std::process::exit(1);
                }
            }
        }
        Command::Setup => {
            println!("atomic-rollback: setting up snapshots and rollback\n");
            if let Err(e) = migrate::setup() {
                eprintln!("Setup failed: {e}");
                std::process::exit(1);
            }
        }
        Command::Migrate => {
            println!("atomic-rollback: full boot migration for atomic rollback\n");
            if let Err(e) = migrate::migrate() {
                eprintln!("Migration failed: {e}");
                std::process::exit(1);
            }
        }
        Command::Snapshot(sub) => match sub {
            SnapshotCommand::Create { name } => {
                match snapshot::snapshot(name.as_deref()) {
                    Ok(snapshot::SnapshotResult::Created(name)) =>
                        eprintln!("Snapshot '{name}' created."),
                    Ok(snapshot::SnapshotResult::Existed(name)) =>
                        eprintln!("Snapshot '{name}' already exists."),
                    Ok(snapshot::SnapshotResult::NotBtrfs) => {}
                    Err(e) => {
                        eprintln!("Snapshot failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            SnapshotCommand::List => {
                match snapshot::list() {
                    Ok(names) => {
                        if names.is_empty() {
                            eprintln!("No snapshots found.");
                        } else {
                            for name in &names {
                                println!("{name}");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Snapshot list failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            SnapshotCommand::Delete { name } => {
                let result: Result<tools::NonSystemSubvolume, String> = (|| {
                    let (_, fstab) = tools::root_device()?;
                    let subvol = tools::find_subvol("/", name)?;
                    tools::NonSystemSubvolume::refine(subvol, &fstab)
                })();
                let target = match result {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("Snapshot delete failed: {e}");
                        std::process::exit(1);
                    }
                };
                match snapshot::delete(&target) {
                    Ok(()) => eprintln!("Deleted snapshot '{}'.", target.as_subvolume().path),
                    Err(e) => {
                        eprintln!("Snapshot delete failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Command::Rollback { name } => {
            println!("atomic-rollback: rolling back to '{name}'\n");
            if let Err(e) = rollback::rollback(&name) {
                eprintln!("Rollback failed: {e}");
                std::process::exit(1);
            }
            println!("Rollback complete. Reboot to activate.");
        }
        Command::KernelHook { command, kver } => {
            if let Err(e) = kernel_hook::handle(&command, &kver) {
                eprintln!("atomic-rollback kernel-hook: {e}");
                std::process::exit(1);
            }
        }
    }
}
