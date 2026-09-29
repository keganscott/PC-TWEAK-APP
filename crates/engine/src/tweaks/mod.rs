//! The tweak catalogue.

use crate::types::Tweak;

pub mod ifeo_priority;
pub mod mouse_accel;

/// Every tweak the engine knows about, in display order.
pub fn catalogue() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(mouse_accel::MouseAcceleration),
        Box::new(ifeo_priority::IfeoPriority::fortnite()),
    ]
}
