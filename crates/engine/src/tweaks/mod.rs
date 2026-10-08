//! The tweak catalogue.

use crate::types::Tweak;

pub mod dns;
pub mod fullscreen;
pub mod ifeo_priority;
pub mod mouse_accel;
pub mod nagle;
pub mod power;
pub mod registry_values;
pub mod services;
pub mod system_restore;
pub mod tasks;

/// Every tweak the engine knows about, in display order.
pub fn catalogue() -> Vec<Box<dyn Tweak>> {
    let mut all: Vec<Box<dyn Tweak>> = vec![Box::new(mouse_accel::MouseAcceleration)];
    all.extend(registry_values::all());
    all.push(Box::new(nagle::Nagle));
    all.push(Box::new(dns::CloudflareDns));
    all.extend(power::all());
    all.extend(services::all());
    all.extend(tasks::all());
    all.push(Box::new(ifeo_priority::CsrssPriority));
    // Per game, for each offered game (`env::KNOWN_GAMES`).
    for t in ifeo_priority::IfeoPriority::offered() {
        all.push(Box::new(t));
    }
    for t in fullscreen::FullscreenOptimizations::offered() {
        all.push(Box::new(t));
    }
    all
}

/// Tweaks the engine applies to itself. Never listed, never callable from IPC
/// `apply_tweak`; revertable so `revert_all` leaves nothing behind.
pub fn internal() -> Vec<Box<dyn Tweak>> {
    vec![Box::new(system_restore::RestoreFrequency)]
}
