#![deny(unsafe_code)]

#[cfg(feature = "native-ui")]
pub(crate) mod app_model;
#[cfg(feature = "native-ui")]
pub(crate) mod deadline_view;
#[cfg(feature = "native-ui")]
pub(crate) mod loop_state;
#[cfg(feature = "native-ui")]
pub(crate) mod review_model;
#[cfg(feature = "native-ui")]
pub(crate) mod settings;
#[cfg(feature = "native-ui")]
pub(crate) mod slint_review;
#[cfg(feature = "native-ui")]
pub mod slint_ui;

// X7: `[lints.rust] unsafe_code = "allow"` in Cargo.toml is a *package*-level
// exception that exists only so the Slint-generated module (built to
// `OUT_DIR`, not checked in under `src/`) compiles; it is not scoped to that
// module alone by Cargo. This mechanical scan is the confinement backstop:
// it fails the build if the literal `unsafe` keyword ever appears in a
// hand-written `src/**/*.rs` file, regardless of what lint configuration is
// in effect when it is added. See ADR-014's Consequences.
#[cfg(all(test, feature = "native-ui"))]
mod unsafe_confinement {
    use std::path::{Path, PathBuf};

    fn rust_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files_under(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Removes `//` line comments, `/* */` block comments (nesting-aware)
    /// and `"..."` string-literal contents, so a plain-English mention of
    /// "unsafe" in prose or a quoted status string never counts as the
    /// keyword. Not a full Rust lexer (raw strings and char literals are not
    /// special-cased), which is adequate for scanning this crate's own
    /// source rather than arbitrary Rust.
    fn strip_comments_and_strings(source: &str) -> String {
        let bytes = source.as_bytes();
        let mut out = String::with_capacity(source.len());
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == '/' && bytes.get(i + 1) == Some(&b'/') {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            } else if c == '/' && bytes.get(i + 1) == Some(&b'*') {
                let mut depth = 1usize;
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            } else if c == '"' {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            } else {
                out.push(c);
                i += 1;
            }
        }
        out
    }

    fn contains_unsafe_keyword(code: &str) -> bool {
        code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|token| token == "unsafe")
    }

    #[test]
    fn hand_written_source_never_uses_the_unsafe_keyword() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files_under(&root, &mut files);
        assert!(
            !files.is_empty(),
            "expected to find .rs files under {root:?}"
        );
        let offenders: Vec<PathBuf> = files
            .into_iter()
            .filter(|path| {
                let source = std::fs::read_to_string(path).expect("read source file");
                contains_unsafe_keyword(&strip_comments_and_strings(&source))
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "found the `unsafe` keyword in hand-written source: {offenders:?}"
        );
    }
}
