use crate::ModelError;

/// Isolation policy declared by a plan and enforced by the build engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IsolationLevel {
    None,
    Relaxed,
    Strict,
}

impl std::fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::Relaxed => "relaxed",
            Self::Strict => "strict",
        })
    }
}

impl std::str::FromStr for IsolationLevel {
    type Err = ModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "relax" | "relaxed" => Ok(Self::Relaxed),
            "strict" => Ok(Self::Strict),
            _ => Err(ModelError::IsolationError(format!(
                "unknown isolation level: '{value}' (valid: none, relaxed, strict)"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_levels_and_alias() {
        assert_eq!("none".parse(), Ok(IsolationLevel::None));
        assert_eq!("relax".parse(), Ok(IsolationLevel::Relaxed));
        assert_eq!("relaxed".parse(), Ok(IsolationLevel::Relaxed));
        assert_eq!("strict".parse(), Ok(IsolationLevel::Strict));
    }

    #[test]
    fn rejects_unknown_level() {
        assert!("container".parse::<IsolationLevel>().is_err());
    }

    #[test]
    fn displays_canonical_level_names() {
        assert_eq!(IsolationLevel::None.to_string(), "none");
        assert_eq!(IsolationLevel::Relaxed.to_string(), "relaxed");
        assert_eq!(IsolationLevel::Strict.to_string(), "strict");
    }

    #[test]
    fn orders_levels_from_weakest_to_strongest() {
        assert!(IsolationLevel::None < IsolationLevel::Relaxed);
        assert!(IsolationLevel::Relaxed < IsolationLevel::Strict);
    }
}
