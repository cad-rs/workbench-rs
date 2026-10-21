//! License tier model (Phase 4 licensing — types and gating rules land now,
//! the license service and activation flow come with the licensing milestone).
//!
//! A plugin manifest may declare `tier = "pro"`: the plugin *requires* that
//! tier or better. With no license service yet, everything loads; the loader
//! records the requirement and the plugins panel shows it, so the gate in
//! Phase 4 is a one-line check against [`LicenseTier::satisfies`].

/// What a user's license grants (or what a plugin requires).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LicenseTier {
    /// Full-function trial; every Pro feature is unlocked.
    Trial = 0,
    /// Paid tier.
    Pro = 1,
    /// White-label/OEM license: unlocks everything.
    OEM = 2,
}

impl LicenseTier {
    /// Parse a tier name (case-insensitive): "trial" / "pro" / "oem".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trial" => Some(Self::Trial),
            "pro" => Some(Self::Pro),
            "oem" => Some(Self::OEM),
            _ => None,
        }
    }

    /// Canonical spelling (manifest / UI display).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trial => "trial",
            Self::Pro => "pro",
            Self::OEM => "oem",
        }
    }

    /// Whether a license at `self` satisfies a `required` tier. `None`
    /// requires nothing (any license, or none at all).
    pub fn satisfies(self, required: Option<LicenseTier>) -> bool {
        match required {
            None => true,
            Some(req) => self >= req,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_tiers() {
        assert_eq!(LicenseTier::parse("pro"), Some(LicenseTier::Pro));
        assert_eq!(LicenseTier::parse("Trial"), Some(LicenseTier::Trial));
        assert_eq!(LicenseTier::parse(" OEM "), Some(LicenseTier::OEM));
        assert_eq!(LicenseTier::parse("basic"), None);
    }

    #[test]
    fn ordering_pro_over_trial() {
        assert!(LicenseTier::Pro > LicenseTier::Trial);
        assert!(LicenseTier::OEM > LicenseTier::Pro);
    }

    #[test]
    fn satisfies_rules() {
        assert!(LicenseTier::Trial.satisfies(None), "no requirement");
        assert!(LicenseTier::Trial.satisfies(Some(LicenseTier::Trial)));
        assert!(!LicenseTier::Trial.satisfies(Some(LicenseTier::Pro)));
        assert!(LicenseTier::Pro.satisfies(Some(LicenseTier::Trial)));
        assert!(LicenseTier::OEM.satisfies(Some(LicenseTier::Pro)));
    }
}
