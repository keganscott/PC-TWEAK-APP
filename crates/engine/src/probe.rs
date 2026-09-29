//! The tri-state every probe returns.
//!
//! A probe answers Yes, No, or "I could not tell". The third answer is not a
//! failure mode to be papered over: callers must treat `Unknown` as neither
//! yes nor no (an anti-cheat gate that reads `Unknown` as `Yes` would be a lie,
//! and one that reads it as `No` would block tweaks for no reason). Nothing in
//! this crate turns `Unknown` into a bool except through the explicit helpers
//! below, which say which way they lean.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(tag = "state", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Probe<T> {
    /// The thing is present / enabled, with what we learned about it.
    Yes { value: T },
    /// The thing is absent or off. `reason` says how we know.
    No { reason: String },
    /// We could not determine it. `reason` says why (query failed, no
    /// documented signal exists, unsupported format).
    Unknown { reason: String },
}

impl<T> Probe<T> {
    pub fn yes(value: T) -> Self {
        Self::Yes { value }
    }

    pub fn no(reason: impl Into<String>) -> Self {
        Self::No { reason: reason.into() }
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown { reason: reason.into() }
    }

    pub fn is_yes(&self) -> bool {
        matches!(self, Self::Yes { .. })
    }

    pub fn is_no(&self) -> bool {
        matches!(self, Self::No { .. })
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Yes { value } => Some(value),
            _ => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Probe<U> {
        match self {
            Self::Yes { value } => Probe::Yes { value: f(value) },
            Self::No { reason } => Probe::No { reason },
            Self::Unknown { reason } => Probe::Unknown { reason },
        }
    }

    /// Only a definite Yes counts. Use where acting on a wrong "yes" is worse
    /// than missing a real one.
    pub fn definitely_yes(&self) -> bool {
        self.is_yes()
    }

    /// Only a definite No counts. Use where blocking on a wrong "no" is worse
    /// than missing a real one.
    pub fn definitely_no(&self) -> bool {
        self.is_no()
    }
}

impl<T> From<Result<T, crate::error::EngineError>> for Probe<T> {
    fn from(r: Result<T, crate::error::EngineError>) -> Self {
        match r {
            Ok(v) => Probe::Yes { value: v },
            Err(e) => Probe::Unknown { reason: e.to_string() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_neither_yes_nor_no() {
        let p: Probe<bool> = Probe::unknown("query failed");
        assert!(!p.definitely_yes());
        assert!(!p.definitely_no());
        assert!(p.is_unknown());
    }

    #[test]
    fn serializes_with_a_state_tag() {
        let y = serde_json::to_string(&Probe::yes(3u32)).unwrap();
        assert_eq!(y, r#"{"state":"yes","value":3}"#);
        let n = serde_json::to_string(&Probe::<u32>::no("absent")).unwrap();
        assert_eq!(n, r#"{"state":"no","reason":"absent"}"#);
        let u = serde_json::to_string(&Probe::<u32>::unknown("why")).unwrap();
        assert_eq!(u, r#"{"state":"unknown","reason":"why"}"#);
    }

    #[test]
    fn map_preserves_the_reason() {
        let p = Probe::<u32>::no("r").map(|v| v + 1);
        assert_eq!(p, Probe::<u32>::no("r"));
        assert_eq!(Probe::yes(1u32).map(|v| v + 1), Probe::yes(2u32));
    }
}
