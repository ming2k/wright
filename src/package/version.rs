use std::fmt;
use std::cmp::Ordering;

use crate::error::{WrightError, Result};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return Err(WrightError::VersionError(format!(
                "invalid version format: '{s}'"
            )));
        }

        let parse_part = |part: &str| -> Result<u32> {
            part.parse::<u32>().map_err(|_| {
                WrightError::VersionError(format!("invalid version component: '{part}'"))
            })
        };

        let major = parse_part(parts[0])?;
        let minor = if parts.len() > 1 { parse_part(parts[1])? } else { 0 };
        let patch = if parts.len() > 2 { parse_part(parts[2])? } else { 0 };

        Ok(Version { major, minor, patch })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major.cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
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
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 25);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_parse_two_part_version() {
        let v = Version::parse("0.1").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);
    }

    #[test]
    fn test_parse_single_part_version() {
        let v = Version::parse("3").unwrap();
        assert_eq!(v.major, 3);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
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
        assert!(Version::parse("abc").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
    }
}
