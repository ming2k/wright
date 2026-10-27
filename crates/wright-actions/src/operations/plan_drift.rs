//! Plan drift: has the plan on disk changed since its parts were sealed?
//! (ADR-0055)
//!
//! A plan is more than its `plan.toml`. `[[sources]] type = "local"` files live
//! beside it, are build inputs, and are recorded in the parts' provenance as
//! content-addressed lines (ADR-0023, ADR-0055). `plan_checksum` cannot see
//! them — it hashes `plan.toml` alone — so a plan whose only change is the bytes
//! of a bundled file would otherwise report "already up to date" while the
//! installed parts still carry the old content.

use crate::error::Result;
use wright_plan::manifest::PlanManifest;

/// The per-source audit lines the plan on disk would produce now.
///
/// Fails when a `local` source cannot be resolved: a plan whose inputs cannot
/// be read cannot be declared up to date.
pub(super) fn current_source_provenance(manifest: &PlanManifest) -> Result<Vec<String>> {
    crate::foundry::charge::source_provenance_lines(manifest)
}

/// Whether the recorded provenance shows the plan's sources have changed.
///
/// `None` when the comparison cannot be made — the recorded set predates
/// content-addressed provenance (or the plan has no directory), so its `local`
/// lines carry no address to compare. Absent data must never be reported as
/// "no drift", and a missing address must never be guessed at.
pub(super) fn source_provenance_drifted(recorded: &[String], current: &[String]) -> Option<bool> {
    if !is_content_addressed(recorded) || !is_content_addressed(current) {
        return None;
    }
    Some(recorded != current)
}

/// Whether every `local` line in a provenance set carries a content address.
fn is_content_addressed(lines: &[String]) -> bool {
    lines
        .iter()
        .all(|line| !line.starts_with("local ") || line.contains(" sha256="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_provenance_is_not_drift() {
        let lines = vec![
            "http https://example.invalid/x.tar.gz sha256=abc".to_string(),
            "local unit.service sha256=deadbeef".to_string(),
        ];
        assert_eq!(source_provenance_drifted(&lines, &lines), Some(false));
    }

    #[test]
    fn edited_local_source_is_drift() {
        let recorded = vec!["local unit.service sha256=deadbeef".to_string()];
        let current = vec!["local unit.service sha256=cafebabe".to_string()];
        assert_eq!(source_provenance_drifted(&recorded, &current), Some(true));
    }

    #[test]
    fn declaration_only_provenance_cannot_be_compared() {
        let recorded = vec!["local unit.service".to_string()];
        let current = vec!["local unit.service sha256=cafebabe".to_string()];
        assert_eq!(
            source_provenance_drifted(&recorded, &current),
            None,
            "a pre-ADR-0055 line must not be guessed at"
        );
    }

    #[test]
    fn plans_without_local_sources_are_compared_on_their_declarations() {
        let lines = vec!["git https://example.invalid/x.git ref=v1".to_string()];
        assert_eq!(source_provenance_drifted(&lines, &lines), Some(false));
        let other = vec!["git https://example.invalid/x.git ref=v2".to_string()];
        assert_eq!(source_provenance_drifted(&lines, &other), Some(true));
    }
}
