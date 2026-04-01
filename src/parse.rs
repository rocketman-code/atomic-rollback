//! Parsers for mount options, BLS entries, and kernel cmdline parameters.
//! The verus! block contains byte-level implementations with inline
//! verification. Under cargo build, annotations are erased. Under
//! cargo verus build, loop termination, bounds, and postconditions
//! are machine-checked.

use vstd::prelude::*;

verus! {

// Byte values for the verus! block. Verus does not support byte literal
// syntax (b',') -- lit_to_vir handles LitKind::Int but not LitKind::Byte
// (rust_verify/src/rust_to_vir_expr.rs).
pub const COMMA: u8 = 44u8;
pub const EQUALS: u8 = 61u8;
pub const SPACE: u8 = 32u8;
pub const TAB: u8 = 9u8;
pub const NEWLINE: u8 = 10u8;
pub const HASH: u8 = 35u8;
pub const CR: u8 = 13u8;

pub open spec fn bytes_match_at(haystack: Seq<u8>, needle: Seq<u8>, pos: int) -> bool {
    pos >= 0
    && pos + needle.len() <= haystack.len()
    && forall|j: int| 0 <= j < needle.len() ==> haystack[pos + j] == needle[j]
}

pub open spec fn is_option_start(options: Seq<u8>, p: int) -> bool {
    p == 0 || (p > 0 && options[p - 1] == COMMA)
}

fn match_at(haystack: &[u8], needle: &[u8], pos: usize) -> (result: bool)
    requires
        pos as int + needle@.len() <= usize::MAX as int,
    ensures
        result == bytes_match_at(haystack@, needle@, pos as int),
{
    if pos + needle.len() > haystack.len() {
        return false;
    }
    let mut i: usize = 0;
    while i < needle.len()
        invariant
            i <= needle@.len(),
            pos + needle@.len() <= haystack@.len(),
            pos as int + needle@.len() <= usize::MAX as int,
            forall|j: int| 0 <= j < i as int ==> haystack@[pos as int + j] == needle@[j],
        decreases needle@.len() - i,
    {
        if haystack[pos + i] != needle[i] {
            return false;
        }
        i = i + 1;
    }
    true
}

// Comma-separated mount options: "compress=zstd:1,subvol=root"
// Returns the byte range of the value for a given key.
// Matches whole keys: "subvol" does not match "subvolid".
fn find_option(options: &[u8], key: &[u8]) -> (result: Option<(usize, usize)>)
    requires
        key@.len() + options@.len() <= usize::MAX as int,
    ensures
        match result {
            Some((s, e)) => {
                s <= e && e <= options@.len()
                && exists|p: int| #![auto]
                    0 <= p && p + key@.len() + 1 == s
                    && is_option_start(options@, p)
                    && bytes_match_at(options@, key@, p)
                    && options@[p + key@.len()] == EQUALS
                && forall|j: int| s <= j < e ==> options@[j] != COMMA
                && (e == options@.len() || options@[e as int] == COMMA)
            },
            None => true,
        },
{
    if key.len() == 0 {
        return None;
    }
    let mut i: usize = 0;
    while i < options.len()
        invariant
            i <= options@.len(),
            key@.len() + options@.len() <= usize::MAX as int,
            i < options@.len() ==> is_option_start(options@, i as int),
        decreases options@.len() - i,
    {
        let option_start = i;
        // found_comma carries the comma-detection fact through the loop
        // invariant. Verus lowers compound while-guards to opaque booleans
        // (sst_to_air.rs:2606), so a flag is needed to preserve the proof.
        let mut found_comma = false;
        let mut option_end = i;
        while option_end < options.len() && !found_comma
            invariant
                i <= option_end,
                option_end <= options@.len(),
                forall|j: int| i as int <= j < option_end as int ==> options@[j] != COMMA,
                found_comma ==> (option_end < options@.len() && options@[option_end as int] == COMMA),
            decreases options@.len() - option_end + if found_comma { 0int } else { 1int },
        {
            if options[option_end] == COMMA {
                found_comma = true;
            } else {
                option_end = option_end + 1;
            }
        }

        if match_at(options, key, option_start) {
            let after_key = option_start + key.len();
            if after_key < options.len() {
                if options[after_key] == EQUALS {
                    let val_start = after_key + 1;
                    if val_start <= option_end {
                        assert(is_option_start(options@, option_start as int));
                        assert(bytes_match_at(options@, key@, option_start as int));
                        assert(option_start as int + key@.len() + 1 == val_start as int);
                        return Some((val_start, option_end));
                    }
                }
            }
        }

        if found_comma {
            i = option_end + 1;
        } else {
            i = option_end;
        }
    }
    None
}

// Whitespace-separated kernel cmdline tokens: "root=UUID=abc-123 ro rhgb"
// Returns the byte range of the value after a given prefix.
fn find_root_uuid(options: &[u8], prefix: &[u8]) -> (result: Option<(usize, usize)>)
    requires
        prefix@.len() > 0,
        prefix@.len() + options@.len() <= usize::MAX as int,
    ensures
        match result {
            Some((s, e)) => {
                s <= e && e <= options@.len()
                && s >= prefix@.len()
                && bytes_match_at(options@, prefix@, (s - prefix@.len()) as int)
                && forall|j: int| s <= j < e ==> options@[j] != SPACE && options@[j] != TAB
            },
            None => true,
        },
{
    let mut i: usize = 0;
    while i < options.len()
        invariant
            i <= options@.len(),
            prefix@.len() > 0,
            prefix@.len() + options@.len() <= usize::MAX as int,
        decreases options@.len() - i,
    {
        let token_start = i;
        let mut token_end = i;
        while token_end < options.len() && options[token_end] != SPACE && options[token_end] != TAB
            invariant
                i <= token_end,
                token_end <= options@.len(),
                forall|j: int| i <= j < token_end ==> options@[j] != SPACE && options@[j] != TAB,
            decreases options@.len() - token_end,
        {
            token_end = token_end + 1;
        }

        if match_at(options, prefix, token_start) {
            let val_start = token_start + prefix.len();
            if val_start < token_end {
                assert(bytes_match_at(options@, prefix@, token_start as int));
                return Some((val_start, token_end));
            }
        }

        i = token_end;
        while i < options.len() && (options[i] == SPACE || options[i] == TAB)
            invariant
                token_end <= i,
                i <= options@.len(),
            decreases options@.len() - i,
        {
            i = i + 1;
        }
        if i == token_start {
            i = i + 1;
        }
    }
    None
}

// BLS entry format (Boot Loader Specification, systemd.io/BOOT_LOADER_SPECIFICATION):
// Lines of "key<whitespace>value". Lines starting with # are comments.
// Returns the byte range of the trimmed value for a given key.
fn find_field(content: &[u8], key: &[u8]) -> (result: Option<(usize, usize)>)
    requires
        key@.len() + content@.len() <= usize::MAX as int,
    ensures
        match result {
            Some((s, e)) => {
                s <= e && e <= content@.len()
                && exists|p: int| #![auto]
                    0 <= p && p + key@.len() < s
                    && bytes_match_at(content@, key@, p)
            },
            None => true,
        },
{
    if key.len() == 0 {
        return None;
    }
    let mut i: usize = 0;
    while i < content.len()
        invariant
            i <= content@.len(),
            key@.len() > 0,
            key@.len() + content@.len() <= usize::MAX as int,
        decreases content@.len() - i,
    {
        let line_start = i;

        // skip leading whitespace
        while i < content.len() && (content[i] == SPACE || content[i] == TAB)
            invariant
                line_start <= i,
                i <= content@.len(),
            decreases content@.len() - i,
        {
            i = i + 1;
        }

        if i >= content.len() {
            return None;
        }

        let content_start = i;

        // find end of line
        let mut line_end = i;
        while line_end < content.len() && content[line_end] != NEWLINE
            invariant
                i <= line_end,
                line_end <= content@.len(),
            decreases content@.len() - line_end,
        {
            line_end = line_end + 1;
        }

        if content[content_start] != HASH {
            if match_at(content, key, content_start) {
                let after_key = content_start + key.len();
                if after_key < content.len() {
                    if content[after_key] == SPACE || content[after_key] == TAB {
                        // skip whitespace between key and value
                        let mut val_start = after_key + 1;
                        while val_start < line_end && (content[val_start] == SPACE || content[val_start] == TAB)
                            invariant
                                after_key + 1 <= val_start,
                                val_start <= content@.len(),
                                line_end <= content@.len(),
                            decreases line_end - val_start,
                        {
                            val_start = val_start + 1;
                        }
                        // trim trailing whitespace and \r
                        let mut val_end = if val_start <= line_end { line_end } else { val_start };
                        while val_end > val_start && (content[val_end - 1] == SPACE || content[val_end - 1] == TAB || content[val_end - 1] == CR)
                            invariant
                                val_start <= val_end,
                                val_end <= content@.len(),
                            decreases val_end - val_start,
                        {
                            val_end = val_end - 1;
                        }
                        assert(bytes_match_at(content@, key@, content_start as int));
                        assert(content_start as int + key@.len() < val_start as int);
                        return Some((val_start, val_end));
                    }
                }
            }
        }

        i = if line_end < content.len() { line_end + 1 } else { line_end };
        if i <= line_start {
            i = line_start + 1;
        }
    }
    None
}

} // verus!

const ROOT_PREFIX: &[u8] = b"root=UUID=";

/// Extracts a value from comma-separated mount options.
/// "compress=zstd:1,subvol=root" with key "subvol" returns Some("root").
/// Matches whole keys only: "subvol" does not match "subvolid".
pub fn extract_mount_option<'a>(options: &'a str, key: &str) -> Option<&'a str> {
    let (s, e) = find_option(options.as_bytes(), key.as_bytes())?;
    Some(&options[s..e])
}

/// Extracts the root filesystem UUID from kernel cmdline options.
/// Looks for "root=UUID=<value>" in whitespace-separated tokens.
pub fn extract_root_uuid_from_options<'a>(options: &'a str) -> Option<&'a str> {
    let (s, e) = find_root_uuid(options.as_bytes(), ROOT_PREFIX)?;
    Some(&options[s..e])
}

/// Extracts a field value from a BLS entry.
/// Format: lines of "key value". Comments (#) and leading whitespace skipped.
/// Trailing whitespace and \r trimmed from values.
pub fn bls_field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let (s, e) = find_field(content.as_bytes(), key.as_bytes())?;
    Some(&content[s..e])
}

/// Parses all recognized fields from a BLS entry into a map.
/// BLS fields per systemd.io/BOOT_LOADER_SPECIFICATION:
///   title, version, linux, initrd, options
/// GRUB extensions (from grub.cfg 10_linux):
///   grub_users, grub_arg, grub_class
pub fn parse_bls_fields(content: &str) -> std::collections::HashMap<String, String> {
    let mut fields = std::collections::HashMap::new();
    for key in &["title", "version", "linux", "initrd", "options",
                 "grub_users", "grub_arg", "grub_class"] {
        if let Some(val) = bls_field(content, key) {
            fields.insert(key.to_string(), val.to_string());
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_option_basic() {
        assert_eq!(extract_mount_option("compress=zstd:1,subvol=root", "subvol"), Some("root"));
        assert_eq!(extract_mount_option("subvol=home,compress=zstd:1", "subvol"), Some("home"));
        assert_eq!(extract_mount_option("defaults", "subvol"), None);
    }

    #[test]
    fn mount_option_no_prefix_confusion() {
        assert_eq!(extract_mount_option("subvolid=256", "subvol"), None);
        assert_eq!(extract_mount_option("subvolid=256,subvol=root", "subvol"), Some("root"));
    }

    #[test]
    fn root_uuid_basic() {
        assert_eq!(extract_root_uuid_from_options("root=UUID=abc-123 ro rhgb"), Some("abc-123"));
        assert_eq!(extract_root_uuid_from_options("ro root=UUID=xyz quiet"), Some("xyz"));
        assert_eq!(extract_root_uuid_from_options("ro rhgb quiet"), None);
    }

    #[test]
    fn root_uuid_no_false_match() {
        assert_eq!(extract_root_uuid_from_options("rootflags=subvol=root"), None);
        assert_eq!(extract_root_uuid_from_options("root=/dev/vda4"), None);
    }

    #[test]
    fn bls_field_basic() {
        let content = "title Fedora Linux\nversion 6.17.1\nlinux /vmlinuz-6.17.1\n";
        assert_eq!(bls_field(content, "title"), Some("Fedora Linux"));
        assert_eq!(bls_field(content, "version"), Some("6.17.1"));
        assert_eq!(bls_field(content, "linux"), Some("/vmlinuz-6.17.1"));
        assert_eq!(bls_field(content, "missing"), None);
    }

    #[test]
    fn bls_field_comments_and_blanks() {
        let content = "# comment\n\ntitle Test\n  # indented\nversion 1.0\n";
        assert_eq!(bls_field(content, "title"), Some("Test"));
        assert_eq!(bls_field(content, "version"), Some("1.0"));
    }

    #[test]
    fn bls_field_value_with_spaces() {
        assert_eq!(bls_field("options root=UUID=abc ro rhgb\n", "options"), Some("root=UUID=abc ro rhgb"));
    }
}
