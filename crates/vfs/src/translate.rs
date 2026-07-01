//! Pure path-normalisation logic (no filesystem access), so it is exhaustively
//! unit-testable and can't be fooled by races.

use std::fmt;

/// Why a guest path could not be translated.
#[derive(Debug, PartialEq, Eq)]
pub enum TranslationError {
    /// The path tried to escape the sandbox root via `..`.
    Escape,
    /// The path was empty or otherwise malformed.
    Malformed,
}

impl fmt::Display for TranslationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranslationError::Escape => f.write_str("path escapes sandbox"),
            TranslationError::Malformed => f.write_str("malformed path"),
        }
    }
}

/// Normalise an iOS-style path into a list of clean components, collapsing `.`
/// and resolving `..` *lexically*. Returns [`TranslationError::Escape`] if the
/// path pops above the root — this is the primary sandbox-escape guard and runs
/// before any host path is constructed.
///
/// The result never contains `.`/`..`/empty components, so joining it onto a
/// host root can never traverse upward regardless of the input.
pub fn normalize_ios_path(path: &str) -> Result<Vec<String>, TranslationError> {
    if path.is_empty() {
        return Err(TranslationError::Malformed);
    }
    let mut out: Vec<String> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => continue, // leading slash, doubled slashes, or "."
            ".." => {
                // Refuse to pop above the sandbox root.
                if out.pop().is_none() {
                    return Err(TranslationError::Escape);
                }
            }
            // Reject NUL and raw path separators smuggled via encoding.
            c if c.contains('\0') => return Err(TranslationError::Malformed),
            c => out.push(c.to_owned()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_dots() {
        assert_eq!(
            normalize_ios_path("/var/mobile/./Documents/../Documents/a.txt").unwrap(),
            vec!["var", "mobile", "Documents", "a.txt"]
        );
    }

    #[test]
    fn rejects_escape() {
        assert_eq!(normalize_ios_path("/../../etc/passwd"), Err(TranslationError::Escape));
        assert_eq!(normalize_ios_path("a/../../b"), Err(TranslationError::Escape));
    }

    #[test]
    fn rejects_empty_and_nul() {
        assert_eq!(normalize_ios_path(""), Err(TranslationError::Malformed));
        assert_eq!(normalize_ios_path("a\0b"), Err(TranslationError::Malformed));
    }
}
