//! The tweak catalogue.

use crate::types::Tweak;

pub mod adapter_props;
pub mod amd;
pub mod cable;
pub mod dns;
pub mod fullscreen;
pub mod ifeo_priority;
pub mod mouse_accel;
pub mod msi;
pub mod nagle;
pub mod nvidia;
pub mod power;
pub mod registry_values;
pub mod services;
pub mod session;
pub mod startup;
pub mod system_restore;
pub mod tasks;
pub mod tcp;

/// Every tweak the engine knows about, in display order.
pub fn catalogue() -> Vec<Box<dyn Tweak>> {
    let mut all: Vec<Box<dyn Tweak>> = vec![Box::new(mouse_accel::MouseAcceleration)];
    all.extend(registry_values::all());
    all.push(Box::new(nagle::Nagle));
    all.push(Box::new(tcp::TcpSettings));
    all.push(Box::new(dns::CloudflareDns));
    all.push(Box::new(cable::PreferCable));
    all.extend(adapter_props::all());
    all.extend(power::all());
    all.extend(services::all());
    all.extend(tasks::all());
    all.extend(nvidia::all());
    all.extend(amd::all());
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
    let mut all: Vec<Box<dyn Tweak>> = vec![Box::new(system_restore::RestoreFrequency)];
    all.extend(session::all());
    all
}
