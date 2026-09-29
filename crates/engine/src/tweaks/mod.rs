//! The tweak catalogue.

use crate::types::Tweak;

pub mod mouse_accel;
pub mod priority_separation;

/// Every tweak the engine knows about, in display order.
pub fn catalogue() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(mouse_accel::MouseAcceleration),
        Box::new(priority_separation::PrioritySeparation),
    ]
}
