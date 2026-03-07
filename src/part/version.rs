use std::fmt;
use std::cmp::Ordering;

use crate::error::{WrightError, Result};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum Segment {
    Num(u64),
    Alpha(String),
}

impl Ord for Segment {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Segment::Num(a), Segment::Num(b)) => a.cmp(b),
            (Segment::Alpha(a), Segment::Alpha(b)) => a.cmp(b),
            // Numbers sort after letters (rpm/pacman convention)
            (Segment::Num(_), Segment::Alpha(_)) => Ordering::Greater,
            (Segment::Alpha(_), Segment::Num(_)) => Ordering::Less,
        }
    }
}

impl PartialOrd for Segment {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Version {
    raw: String,
    segments: Vec<Segment>,
}

/// Split a string into segments: alternating runs of digits and non-digit characters.
/// Digits become `Num`, letters become `Alpha`.
fn tokenize(s: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut num = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    num.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            segments.push(Segment::Num(num.parse::<u64>().unwrap_or(0)));
        } else if c.is_ascii_alphabetic() {
            let mut alpha = String::new();
            while let Some(&a) = chars.peek() {
                if a.is_ascii_alphabetic() {
                    alpha.push(a);
                    chars.next();
                } else {
                    break;
                }
            }
            segments.push(Segment::Alpha(alpha));
        } else {
            // Skip non-alphanumeric (delimiters like `.` and `-`)
            chars.next();
        }
    }

    segments
}

impl Version {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(WrightError::VersionError(
                "version string must not be empty".to_string(),
            ));
        }

        let segments = tokenize(s);
        if segments.is_empty() {
            return Err(WrightError::VersionError(format!(
                "invalid version format: '{s}'"
            )));
        }

        Ok(Version {
            raw: s.to_string(),
            segments,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.segments.len().max(other.segments.len());
        for i in 0..len {
            let a = self.segments.get(i);
            let b = other.segments.get(i);
            let ord = match (a, b) {
                (Some(a), Some(b)) => a.cmp(b),
                // Missing segments treated as zero (sorts before any present segment)
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VersionOp {
    Ge, // >=
    Le, // <=
    Eq, // =
    Gt, // >
    Lt, // <
}

#[derive(Debug, Clone)]
pub struct VersionConstraint {
    pub op: VersionOp,
    pub version: Version,
}

impl VersionConstraint {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let (op, ver_str) = if let Some(rest) = s.strip_prefix(">=") {
            (VersionOp::Ge, rest.trim())
        } else if let Some(rest) = s.strip_prefix("<=") {
            (VersionOp::Le, rest.trim())
        } else if let Some(rest) = s.strip_prefix('>') {
            (VersionOp::Gt, rest.trim())
        } else if let Some(rest) = s.strip_prefix('<') {
            (VersionOp::Lt, rest.trim())
        } else if let Some(rest) = s.strip_prefix('=') {
            (VersionOp::Eq, rest.trim())
        } else {
            return Err(WrightError::VersionError(format!(
                "invalid version constraint: '{s}'"
            )));
        };

        let version = Version::parse(ver_str)?;
        Ok(VersionConstraint { op, version })
    }

    pub fn satisfies(&self, v: &Version) -> bool {
        match self.op {
            VersionOp::Ge => v >= &self.version,
            VersionOp::Le => v <= &self.version,
            VersionOp::Eq => v == &self.version,
            VersionOp::Gt => v > &self.version,
            VersionOp::Lt => v < &self.version,
        }
    }
}

impl fmt::Display for VersionConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = match self.op {
            VersionOp::Ge => ">=",
            VersionOp::Le => "<=",
            VersionOp::Eq => "=",
            VersionOp::Gt => ">",
            VersionOp::Lt => "<",
        };
        write!(f, "{} {}", op, self.version)
    }
}

/// Parse a dependency string like "openssl >= 3.0" into (name, optional constraint)
pub fn parse_dependency(dep: &str) -> Result<(String, Option<VersionConstraint>)> {
    let dep = dep.trim();

    // Try to find an operator
    let ops = [">=", "<=", ">", "<", "="];
    for op in &ops {
        if let Some(pos) = dep.find(op) {
            let name = dep[..pos].trim().to_string();
            let constraint = VersionConstraint::parse(&dep[pos..])?;
            return Ok((name, Some(constraint)));
        }
    }

    // No operator — just a package name
    Ok((dep.to_string(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_version() {
        let v = Version::parse("1.25.3").unwrap();
        assert_eq!(v.to_string(), "1.25.3");
    }

    #[test]
    fn test_parse_two_part_version() {
        let v = Version::parse("0.1").unwrap();
        assert_eq!(v.to_string(), "0.1");
    }

    #[test]
    fn test_parse_single_part_version() {
        let v = Version::parse("3").unwrap();
        assert_eq!(v.to_string(), "3");
    }

    #[test]
    fn test_version_ordering() {
        let v1 = Version::parse("1.0.0").unwrap();
        let v2 = Version::parse("1.0.1").unwrap();
        let v3 = Version::parse("1.1.0").unwrap();
        let v4 = Version::parse("2.0.0").unwrap();

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert_eq!(v1, Version::parse("1.0.0").unwrap());
    }

    #[test]
    fn test_version_display() {
        let v = Version::parse("1.25.3").unwrap();
        assert_eq!(v.to_string(), "1.25.3");
    }

    #[test]
    fn test_freeform_ncurses() {
        let v = Version::parse("6.5-20250809").unwrap();
        assert_eq!(v.to_string(), "6.5-20250809");
        assert!(v > Version::parse("6.5-20250808").unwrap());
        assert!(v < Version::parse("6.5-20250810").unwrap());
    }

    #[test]
    fn test_freeform_tzdata() {
        let a = Version::parse("2024a").unwrap();
        let b = Version::parse("2024b").unwrap();
        assert!(a < b);
    }

    #[test]
    fn test_freeform_openssh() {
        let v1 = Version::parse("6.2.13p2").unwrap();
        let v2 = Version::parse("6.2.13p3").unwrap();
        assert!(v1 < v2);
        assert_eq!(v1.to_string(), "6.2.13p2");
    }

    #[test]
    fn test_numbers_sort_after_letters() {
        // rpm/pacman convention: numeric segments sort higher than alpha
        let alpha = Version::parse("1.0a").unwrap();
        let numeric = Version::parse("1.0.1").unwrap();
        assert!(alpha < numeric, "1.0a should be less than 1.0.1");
    }

    #[test]
    fn test_ordering_with_different_lengths() {
        let short = Version::parse("1.0").unwrap();
        let long = Version::parse("1.0.1").unwrap();
        assert!(short < long);
    }

    #[test]
    fn test_version_equality_roundtrip() {
        let v1 = Version::parse("1.2.3").unwrap();
        let v2 = Version::parse("1.2.3").unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_constraint_ge() {
        let c = VersionConstraint::parse(">= 1.2.0").unwrap();
        assert!(c.satisfies(&Version::parse("1.2.0").unwrap()));
        assert!(c.satisfies(&Version::parse("1.3.0").unwrap()));
        assert!(!c.satisfies(&Version::parse("1.1.9").unwrap()));
    }

    #[test]
    fn test_constraint_lt() {
        let c = VersionConstraint::parse("< 2.0").unwrap();
        assert!(c.satisfies(&Version::parse("1.9.9").unwrap()));
        assert!(!c.satisfies(&Version::parse("2.0.0").unwrap()));
    }

    #[test]
    fn test_constraint_eq() {
        let c = VersionConstraint::parse("= 1.0.0").unwrap();
        assert!(c.satisfies(&Version::parse("1.0.0").unwrap()));
        assert!(!c.satisfies(&Version::parse("1.0.1").unwrap()));
    }

    #[test]
    fn test_parse_dependency_with_constraint() {
        let (name, constraint) = parse_dependency("openssl >= 3.0").unwrap();
        assert_eq!(name, "openssl");
        let c = constraint.unwrap();
        assert_eq!(c.op, VersionOp::Ge);
        assert_eq!(c.version, Version::parse("3.0").unwrap());
    }

    #[test]
    fn test_parse_dependency_without_constraint() {
        let (name, constraint) = parse_dependency("gcc").unwrap();
        assert_eq!(name, "gcc");
        assert!(constraint.is_none());
    }

    #[test]
    fn test_invalid_version() {
        assert!(Version::parse("").is_err());
        // Non-empty strings with alphanumeric content should now parse fine
        assert!(Version::parse("abc").is_ok());
        // Pure punctuation with no alphanumeric content should fail
        assert!(Version::parse("...").is_err());
    }
}
