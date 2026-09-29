//! Telemetry & Proof: measure whether a change did anything.
//!
//! Nothing in the product may claim a difference without a stored run behind it.
//! This module captures frames (PresentMon), turns them into statistics, and
//! decides "Better / No measurable change / Worse" only when the difference is
//! larger than the run-to-run spread actually measured.

pub mod capture;
pub mod metrics;
pub mod nvml;
pub mod presentmon;
pub mod service;
pub mod store;
pub mod verdict;
