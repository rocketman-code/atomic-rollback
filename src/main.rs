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
    Rollback { name: Option<String> },
    KernelHook { command: String, kver: String },
    EnsureHooks,
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
    eprintln!("  atomic-rollback check                verify the boot chain is valid");
    eprintln!("  atomic-rollback setup                separate /var, enable snapshots and rollback");
    eprintln!("  atomic-rollback migrate              full boot migration for complete kernel rollback");
    eprintln!("  atomic-rollback snapshot             create snapshot (auto-named)");
    eprintln!("  atomic-rollback snapshot create [N]  create snapshot with optional name");
    eprintln!("  atomic-rollback snapshot list        show available snapshots");
    eprintln!("  atomic-rollback snapshot delete <N>  delete a snapshot by ID or name");
    eprintln!("  atomic-rollback rollback [id|name]    roll back to a snapshot");
}

fn print_snapshot_help() {
    eprintln!("usage:");
    eprintln!("  atomic-rollback snapshot             create snapshot (auto-named)");
    eprintln!("  atomic-rollback snapshot create [N]  create snapshot with optional name");
    eprintln!("  atomic-rollback snapshot list        show available snapshots");
    eprintln!("  atomic-rollback snapshot delete <N>  delete a snapshot by ID or name");
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
                    eprintln!("Usage: atomic-rollback snapshot delete <id|name>");
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
                name: args.get(2).cloned(),
            }
        }
        "ensure-hooks" => Command::EnsureHooks,
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
            println!("atomic-rollback: verifying boot chain\n");
            match check::verify_bootable(Path::new(&root)) {
                check::BootStatus::Pass => {
                    println!("\nBoot chain is valid.");
                }
                check::BootStatus::Warn => {
                    println!("\nBoot chain is valid (with warnings).");
                    std::process::exit(2);
                }
                check::BootStatus::Fail(failures) => {
                    for f in &failures { eprintln!("  {f}"); }
                    println!("\nBoot chain has problems.");
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
                    Ok(snapshot::SnapshotResult::Created(name, id)) =>
                        eprintln!("Snapshot '{name}' with ID {id} created."),
                    Ok(snapshot::SnapshotResult::Existed(name, id)) =>
                        eprintln!("Snapshot '{name}' with ID {id} already exists."),
                    Ok(snapshot::SnapshotResult::NotBtrfs) => {}
                    Err(e) => {
                        eprintln!("Snapshot failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            SnapshotCommand::List => {
                match snapshot::list() {
                    Ok(snapshots) => {
                        if snapshots.is_empty() {
                            eprintln!("No snapshots found.");
                        } else {
                            let id_w = snapshots.iter().map(|s| s.id.to_string().len()).max().unwrap_or(2).max(2);
                            let name_w = snapshots.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
                            println!("{:<id_w$}  {:<name_w$}  {}", "ID", "Name", "Created");
                            for s in &snapshots {
                                println!("{:<id_w$}  {:<name_w$}  {}", s.id, s.name, s.created);
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
                let (resolved, _) = match snapshot::resolve_snapshot(&name) {
                    Ok(r) => r,
                    Err(e) => { eprintln!("Snapshot delete failed: {e}"); std::process::exit(1); }
                };
                match snapshot::delete(&resolved) {
                    Ok(id) => eprintln!("Snapshot '{resolved}' with ID {id} deleted."),
                    Err(e) => {
                        eprintln!("Snapshot delete failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Command::Rollback { name } => {
            let (name, id) = match name {
                Some(arg) => match snapshot::resolve_snapshot(&arg) {
                    Ok(r) => r,
                    Err(e) => { eprintln!("Rollback failed: {e}"); std::process::exit(1); }
                },
                None => match snapshot::most_recent_snapshot() {
                    Ok(r) => r,
                    Err(e) => { eprintln!("Rollback failed: {e}"); std::process::exit(1); }
                },
            };
            println!("atomic-rollback: rolling back to '{name}' (ID {id})\n");
            if let Err(e) = rollback::rollback(&name) {
                eprintln!("Rollback failed: {e}");
                std::process::exit(1);
            }
            println!("Rollback complete. Reboot to activate.");
        }
        Command::EnsureHooks => {
            kernel_hook::ensure_hooks();
        }
        Command::KernelHook { command, kver } => {
            if let Err(e) = kernel_hook::handle(&command, &kver) {
                eprintln!("atomic-rollback kernel-hook: {e}");
                std::process::exit(1);
            }
        }
    }
}
