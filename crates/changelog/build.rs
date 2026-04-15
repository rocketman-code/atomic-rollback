use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../changelog-core/src/lib.rs");
    println!("cargo:rerun-if-changed=../../CHANGELOG.md");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let changelog_path = PathBuf::from(&manifest_dir).join("../../CHANGELOG.md");

    let committed = fs::read_to_string(&changelog_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", changelog_path.display(), e));

    let generated = changelog_core::generate();

    if committed != generated {
        panic!(
            "\n\
             CHANGELOG.md is out of sync with crates/changelog-core/src/lib.rs.\n\
             \n\
             Regenerate with:\n\
             \n    cargo run -p changelog > CHANGELOG.md\n\
             \n\
             Then commit the updated CHANGELOG.md alongside your source changes.\n"
        );
    }
}
