use std::path::Path;

use crate::annotation::Span;

/// The digest scheme this build computes. Digests are written `scheme:hex`.
pub const SCHEME: &str = "blake3";

/// Split a stored digest into `(scheme, hex)`.
///
/// A bare hex string (no `scheme:` prefix) is a legacy record and means
/// `blake3`, the only scheme that existed before schemes were named.
pub fn split_digest(digest: &str) -> (&str, &str) {
    match digest.split_once(':') {
        Some((scheme, hex)) if !scheme.is_empty() => (scheme, hex),
        _ => (SCHEME, digest),
    }
}

/// Compute the scheme-prefixed digest (`blake3:<hex>`) of the full lines
/// covered by a span.
///
/// Reads the file at `file_path`, extracts lines `start.line..=end.line`
/// (1-indexed), joins them with `\n`, and hashes the result.
/// Returns `None` if the file is unreadable or the span extends beyond EOF.
pub fn compute_span_hash(file_path: &Path, span: &Span) -> Option<String> {
    let content = std::fs::read_to_string(file_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();

    let start = span.start.line as usize;
    let end = span.end_or_start().line as usize;

    if start == 0 || start > all_lines.len() || end > all_lines.len() {
        return None;
    }

    let selected: Vec<&str> = all_lines[start - 1..=end - 1].to_vec();
    let joined = selected.join("\n");
    Some(format!(
        "{SCHEME}:{}",
        blake3::hash(joined.as_bytes()).to_hex()
    ))
}

/// Freshness status of an annotation's span against current file content.
#[derive(Debug, PartialEq, Eq)]
pub enum FreshnessStatus {
    /// The content hash matches — the code hasn't changed.
    Fresh,
    /// The content hash differs — the code has drifted.
    Drifted { expected: String, actual: String },
    /// The file or span lines are missing.
    Missing { reason: String },
    /// The span has no content_hash — freshness cannot be checked.
    NoHash,
    /// The recorded digest uses a scheme this build does not compute.
    /// Digests compare only within a scheme, so freshness is unknown.
    Incomparable { scheme: String },
}

/// Check whether the code under a span still matches its recorded content_hash.
pub fn check_freshness(file_path: &Path, span: &Span) -> FreshnessStatus {
    let expected = match &span.content_hash {
        Some(h) => h.clone(),
        None => return FreshnessStatus::NoHash,
    };

    let (scheme, expected_hex) = split_digest(&expected);
    if scheme != SCHEME {
        return FreshnessStatus::Incomparable {
            scheme: scheme.to_string(),
        };
    }

    match compute_span_hash(file_path, span) {
        Some(actual) => {
            if split_digest(&actual).1 == expected_hex {
                FreshnessStatus::Fresh
            } else {
                FreshnessStatus::Drifted { expected, actual }
            }
        }
        None => {
            if !file_path.exists() {
                FreshnessStatus::Missing {
                    reason: format!("file not found: {}", file_path.display()),
                }
            } else {
                FreshnessStatus::Missing {
                    reason: format!(
                        "span lines {}..={} beyond file length",
                        span.start.line,
                        span.end_or_start().line
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{Position, Span};
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_file(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    fn span(start: u32, end: Option<u32>) -> Span {
        Span {
            start: Position {
                line: start,
                col: None,
            },
            end: end.map(|line| Position { line, col: None }),
            content_hash: None,
        }
    }

    #[test]
    fn test_compute_single_line() {
        let f = make_file(&["alpha", "beta", "gamma"]);
        let s = span(2, None);
        let hash = compute_span_hash(f.path(), &s).unwrap();

        // Should hash just "beta"
        let expected = format!("blake3:{}", blake3::hash(b"beta").to_hex());
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_split_digest() {
        assert_eq!(split_digest("blake3:abc"), ("blake3", "abc"));
        assert_eq!(split_digest("sha256:def"), ("sha256", "def"));
        // Legacy bare hex means blake3.
        assert_eq!(split_digest("abc"), ("blake3", "abc"));
    }

    #[test]
    fn test_freshness_legacy_bare_hex_is_fresh() {
        let f = make_file(&["alpha", "beta"]);
        let bare = blake3::hash(b"beta").to_hex().to_string();
        let s = Span {
            start: Position { line: 2, col: None },
            end: None,
            content_hash: Some(bare),
        };
        assert_eq!(check_freshness(f.path(), &s), FreshnessStatus::Fresh);
    }

    #[test]
    fn test_freshness_foreign_scheme_is_incomparable() {
        let f = make_file(&["alpha", "beta"]);
        let s = Span {
            start: Position { line: 2, col: None },
            end: None,
            content_hash: Some("sha256:deadbeef".into()),
        };
        assert_eq!(
            check_freshness(f.path(), &s),
            FreshnessStatus::Incomparable {
                scheme: "sha256".into()
            }
        );
    }

    #[test]
    fn test_compute_range() {
        let f = make_file(&["alpha", "beta", "gamma", "delta"]);
        let s = span(2, Some(3));
        let hash = compute_span_hash(f.path(), &s).unwrap();

        let expected = format!("blake3:{}", blake3::hash(b"beta\ngamma").to_hex());
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_compute_file_not_found() {
        let s = span(1, None);
        assert!(compute_span_hash(Path::new("/no/such/file.rs"), &s).is_none());
    }

    #[test]
    fn test_compute_beyond_eof() {
        let f = make_file(&["only one line"]);
        let s = span(5, None);
        assert!(compute_span_hash(f.path(), &s).is_none());
    }

    #[test]
    fn test_compute_end_beyond_eof() {
        let f = make_file(&["line1", "line2"]);
        let s = span(1, Some(10));
        assert!(compute_span_hash(f.path(), &s).is_none());
    }

    #[test]
    fn test_freshness_fresh() {
        let f = make_file(&["alpha", "beta", "gamma"]);
        let hash = compute_span_hash(f.path(), &span(2, None)).unwrap();
        let s = Span {
            start: Position { line: 2, col: None },
            end: None,
            content_hash: Some(hash),
        };
        assert_eq!(check_freshness(f.path(), &s), FreshnessStatus::Fresh);
    }

    #[test]
    fn test_freshness_drifted() {
        let f = make_file(&["alpha", "beta", "gamma"]);
        let s = Span {
            start: Position { line: 2, col: None },
            end: None,
            content_hash: Some(
                "0000000000000000000000000000000000000000000000000000000000000000".into(),
            ),
        };
        match check_freshness(f.path(), &s) {
            FreshnessStatus::Drifted { expected, actual } => {
                assert_eq!(
                    expected,
                    "0000000000000000000000000000000000000000000000000000000000000000"
                );
                assert_ne!(actual, expected);
            }
            other => panic!("expected Drifted, got {other:?}"),
        }
    }

    #[test]
    fn test_freshness_missing_file() {
        let s = Span {
            start: Position { line: 1, col: None },
            end: None,
            content_hash: Some("abc".into()),
        };
        match check_freshness(Path::new("/no/such/file.rs"), &s) {
            FreshnessStatus::Missing { reason } => {
                assert!(reason.contains("file not found"));
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn test_freshness_missing_lines() {
        let f = make_file(&["only one line"]);
        let s = Span {
            start: Position { line: 5, col: None },
            end: None,
            content_hash: Some("abc".into()),
        };
        match check_freshness(f.path(), &s) {
            FreshnessStatus::Missing { reason } => {
                assert!(reason.contains("beyond file length"));
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn test_freshness_no_hash() {
        let f = make_file(&["alpha"]);
        let s = span(1, None);
        assert_eq!(check_freshness(f.path(), &s), FreshnessStatus::NoHash);
    }
}
