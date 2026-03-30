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

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("check") => {
            let root = args.get(2).map(|s| s.as_str()).unwrap_or("/");
            println!("atomic-rollback: checking system bootability\n");
            match check::verify_bootable(Path::new(root)) {
                check::BootStatus::Pass => {
                    println!("\nSystem is bootable.");
                }
                check::BootStatus::Warn => {
                    println!("\nSystem is bootable (with warnings).");
                    std::process::exit(2);
                }
                check::BootStatus::Fail(failures) => {
                    for f in &failures { eprintln!("  {f}"); }
                    println!("\nSystem has boot problems.");
                    std::process::exit(1);
                }
            }
        }
        Some("setup") => {
            println!("atomic-rollback: setting up snapshots and rollback\n");
            if let Err(e) = migrate::setup() {
                eprintln!("Setup failed: {e}");
                std::process::exit(1);
            }
        }
        Some("migrate") => {
            println!("atomic-rollback: full boot migration for atomic rollback\n");
            if let Err(e) = migrate::migrate() {
                eprintln!("Migration failed: {e}");
                std::process::exit(1);
            }
        }
        Some("snapshot") => {
            let sub = args.get(2).map(|s| s.as_str());
            match sub {
                Some("list") => {
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
                Some("delete") => {
                    let name = args.get(3).map(|s| s.as_str()).unwrap_or("");
                    if name.is_empty() {
                        eprintln!("Usage: atomic-rollback snapshot delete <name>");
                        std::process::exit(2);
                    }
                    match snapshot::delete(name) {
                        Ok(()) => eprintln!("Deleted snapshot '{name}'."),
                        Err(e) => {
                            eprintln!("Snapshot delete failed: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                _ => {
                    match snapshot::snapshot(sub) {
                        Ok(name) => eprintln!("Snapshot '{name}' created."),
                        Err(e) => {
                            eprintln!("Snapshot failed: {e}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        Some("rollback") => {
            let name = args.get(2).map(|s| s.as_str()).unwrap_or("root.pre-update");
            println!("atomic-rollback: rolling back to '{name}'\n");
            if let Err(e) = rollback::rollback(name) {
                eprintln!("Rollback failed: {e}");
                std::process::exit(1);
            }
            println!("Rollback complete. Reboot to activate.");
        }
        Some("kernel-hook") => {
            let command = args.get(2).map(|s| s.as_str()).unwrap_or("");
            let kver = args.get(3).map(|s| s.as_str()).unwrap_or("");
            if command.is_empty() || kver.is_empty() {
                std::process::exit(0);
            }
            if let Err(e) = kernel_hook::handle(command, kver) {
                eprintln!("atomic-rollback kernel-hook: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("atomic-rollback: atomic system rollback for Fedora via Btrfs subvolume swap");
            eprintln!();
            eprintln!("usage:");
            eprintln!("  atomic-rollback check              verify the system is bootable");
            eprintln!("  atomic-rollback setup               separate /var, enable snapshots and rollback");
            eprintln!("  atomic-rollback migrate             full boot migration for complete kernel rollback");
            eprintln!("  atomic-rollback snapshot [name]     create a snapshot before updating");
            eprintln!("  atomic-rollback snapshot list       show available snapshots");
            eprintln!("  atomic-rollback snapshot delete N   delete a snapshot by name");
            eprintln!("  atomic-rollback rollback [name]     roll back to a snapshot");
            std::process::exit(2);
        }
    }
}
