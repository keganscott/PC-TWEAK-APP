//! The tweak catalogue.

use crate::types::Tweak;

pub mod ifeo_priority;
pub mod mouse_accel;
pub mod system_restore;

/// Every tweak the engine knows about, in display order.
pub fn catalogue() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(mouse_accel::MouseAcceleration),
        Box::new(ifeo_priority::IfeoPriority::fortnite()),
    ]
}

/// Tweaks the engine applies to itself. Never listed, never callable from IPC
/// `apply_tweak`; revertable so `revert_all` leaves nothing behind.
pub fn internal() -> Vec<Box<dyn Tweak>> {
    vec![Box::new(system_restore::RestoreFrequency)]
}
