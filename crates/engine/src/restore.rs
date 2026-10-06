//! System Restore: reading its state, deciding whether the restore gate is open,
//! and creating and *verifying* a restore point.
//!
//! The gate rule (plan section 1: the dashboard is locked until a restore point
//! is verified or created): the gate is open while Windows lists a restore point
//! no older than `RESTORE_POINT_MAX_AGE_HOURS`. We read that list from Windows
//! itself; nothing the webview says can open it.
//!
//! Slow or OS-specific actions (turning protection on, creating the point) sit
//! behind `RestoreOps`, so the whole flow is tested against a fake. GitHub's
//! Windows runners are Windows Server, where System Restore does not exist, so
//! CI can only exercise these paths through the fake and check that the real
//! implementation fails cleanly (NOTES.md N24).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use ts_rs::TS;

use super::engine::Engine;
use super::error::{EngineError, Result};
use super::journal::{now_ms, RestoreMethod};
use super::probe::Probe;
use super::registry::{Hive, RegistryBackend};
use super::timeutil::cim_datetime_to_unix_ms;
use super::types::{BlockedCode, BlockedReason};
use super::wmi::{WmiRow, WmiSource, NS_CIMV2, NS_DEFAULT};

/// Description given to the restore point we create.
pub const RESTORE_DESCRIPTION: &str = "PeakTweaks: before changes";

/// A restore point older than this no longer opens the gate. Tunable (NOTES.md N25).
pub const RESTORE_POINT_MAX_AGE_HOURS: u64 = 24;

/// How long a gate answer is reused before Windows is asked again.
const GATE_TTL_MS: u64 = 10_000;

const POLICY_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows NT\SystemRestore";
const POLICY_VALUE: &str = "DisableSR";
const CONFIG_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore";
const FREQUENCY_VALUE: &str = "SystemRestorePointCreationFrequency";

pub(crate) const WQL_RESTORE_POINTS: &str =
    "SELECT SequenceNumber, Description, CreationTime, RestorePointType FROM SystemRestore";
pub(crate) const WQL_PRODUCT_TYPE: &str = "SELECT ProductType FROM Win32_OperatingSystem";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RestorePoint {
    pub sequence_number: u32,
    pub description: String,
    /// `None` when Windows gave a time we could not parse.
    pub created_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RestoreStatus {
    /// No on Windows Server, where System Restore does not exist.
    pub supported: Probe<()>,
    /// True when Group Policy turns System Restore off (`DisableSR`).
    pub disabled_by_policy: bool,
    /// Whether System Protection is on for the system drive. Windows has no
    /// single documented switch to read, so this is Unknown unless the OS-level
    /// implementation finds a reliable signal (NOTES.md N23). Creating a point
    /// is the real test.
    pub protection: Probe<()>,
    pub points: Probe<Vec<RestorePoint>>,
    /// The `SystemRestorePointCreationFrequency` setting in minutes, if set.
    pub creation_frequency_minutes: Option<u32>,
    /// Age of the newest restore point with a readable time.
    pub newest_point_age_hours: Option<u64>,
    /// True while a fresh restore point exists.
    pub gate_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatedVia {
    pub method: RestoreMethod,
    /// The sequence number Windows returned, when the method gives one.
    pub sequence_number: Option<u32>,
}

/// The parts of System Restore that change things or take a while.
pub trait RestoreOps: Send + Sync {
    /// All restore points Windows lists.
    fn list_points(&self) -> Result<Vec<RestorePoint>>;

    /// Turn System Protection on for the system drive. Must be harmless when it
    /// is already on.
    fn enable_protection(&self) -> Result<()>;

    /// Ask Windows for a restore point. Success does not prove one exists;
    /// callers verify by listing.
    fn create_point(&self, description: &str) -> Result<CreatedVia>;

    /// Whether protection is on, if a reliable signal exists.
    fn protection_enabled(&self) -> Probe<()> {
        Probe::unknown("Windows has no single documented setting for this; a restore point is created to find out")
    }
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// A clock that always says the same time. For tests.
#[cfg(any(test, feature = "test-support"))]
pub struct FixedClock(pub u64);

#[cfg(any(test, feature = "test-support"))]
impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

// ---------------------------------------------------------------------------
// Reading restore points from WMI (used by the real ops, testable with FakeWmi)
// ---------------------------------------------------------------------------

pub fn list_points_wmi(wmi: &dyn WmiSource) -> Result<Vec<RestorePoint>> {
    let rows = wmi.query(NS_DEFAULT, WQL_RESTORE_POINTS)?;
    Ok(rows.iter().filter_map(point_from).collect())
}

fn point_from(row: &WmiRow) -> Option<RestorePoint> {
    Some(RestorePoint {
        sequence_number: u32::try_from(row.u64("SequenceNumber")?).ok()?,
        description: row.str("Description").unwrap_or("").to_owned(),
        created_unix_ms: row.str("CreationTime").and_then(cim_datetime_to_unix_ms),
    })
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct RestoreService {
    ops: Arc<dyn RestoreOps>,
    reg: Arc<dyn RegistryBackend>,
    wmi: Arc<dyn WmiSource>,
    clock: Arc<dyn Clock>,
    gate_cache: Mutex<Option<(u64, bool)>>,
    verify_attempts: u32,
    verify_interval: Duration,
}

impl RestoreService {
    pub fn new(ops: Arc<dyn RestoreOps>, reg: Arc<dyn RegistryBackend>, wmi: Arc<dyn WmiSource>) -> Self {
        Self {
            ops,
            reg,
            wmi,
            clock: Arc::new(SystemClock),
            gate_cache: Mutex::new(None),
            // Windows can take a few seconds to show a new point.
            verify_attempts: 10,
            verify_interval: Duration::from_secs(1),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn without_waiting(mut self) -> Self {
        self.verify_attempts = 1;
        self.verify_interval = Duration::ZERO;
        self
    }

    pub fn invalidate(&self) {
        if let Ok(mut g) = self.gate_cache.lock() {
            *g = None;
        }
    }

    fn is_server(&self) -> Probe<bool> {
        match self.wmi.query(NS_CIMV2, WQL_PRODUCT_TYPE) {
            Ok(rows) => match rows.first().and_then(|r| r.u64("ProductType")) {
                Some(t) => Probe::yes(t != 1),
                None => Probe::unknown("the OS product type was missing"),
            },
            Err(e) => Probe::unknown(format!("cannot read the OS product type: {e}")),
        }
    }

    fn disabled_by_policy(&self) -> bool {
        matches!(
            self.reg.read_value(Hive::LocalMachine, POLICY_KEY, POLICY_VALUE),
            Ok(Some(v)) if v.as_dword() == Some(1)
        )
    }

    fn frequency(&self) -> Option<u32> {
        self.reg
            .read_value(Hive::LocalMachine, CONFIG_KEY, FREQUENCY_VALUE)
            .ok()
            .flatten()
            .and_then(|v| v.as_dword())
    }

    pub fn status(&self) -> RestoreStatus {
        let now = self.clock.now_ms();
        let supported = match self.is_server() {
            Probe::Yes { value: true } => Probe::no("System Restore does not exist on Windows Server"),
            Probe::Yes { value: false } => Probe::yes(()),
            Probe::No { reason } => Probe::No { reason },
            Probe::Unknown { reason } => Probe::Unknown { reason },
        };
        let points: Probe<Vec<RestorePoint>> = self.ops.list_points().into();
        let newest_age = points.value().and_then(|p| newest_age_hours(p, now));
        let gate_open = gate_from(&points, now);
        RestoreStatus {
            supported,
            disabled_by_policy: self.disabled_by_policy(),
            protection: self.ops.protection_enabled(),
            points,
            creation_frequency_minutes: self.frequency(),
            newest_point_age_hours: newest_age,
            gate_open,
        }
    }

    /// Is a fresh restore point on the machine right now? Cached for a few
    /// seconds; `invalidate` after creating one.
    pub fn gate_open(&self) -> bool {
        let now = self.clock.now_ms();
        if let Ok(g) = self.gate_cache.lock() {
            if let Some((at, open)) = *g {
                if now.saturating_sub(at) < GATE_TTL_MS {
                    return open;
                }
            }
        }
        let points: Probe<Vec<RestorePoint>> = self.ops.list_points().into();
        let open = gate_from(&points, now);
        if let Ok(mut g) = self.gate_cache.lock() {
            *g = Some((now, open));
        }
        open
    }
}

/// A point counts only if its time is readable and recent. An unreadable time is
/// never assumed to be recent.
pub fn gate_from(points: &Probe<Vec<RestorePoint>>, now_ms: u64) -> bool {
    newest_age_hours_probe(points, now_ms).is_some_and(|age| age <= RESTORE_POINT_MAX_AGE_HOURS)
}

fn newest_age_hours_probe(points: &Probe<Vec<RestorePoint>>, now_ms: u64) -> Option<u64> {
    points.value().and_then(|p| newest_age_hours(p, now_ms))
}

fn newest_age_hours(points: &[RestorePoint], now_ms: u64) -> Option<u64> {
    points
        .iter()
        .filter_map(|p| p.created_unix_ms)
        .max()
        .map(|newest| now_ms.saturating_sub(newest) / 3_600_000)
}

// ---------------------------------------------------------------------------
// Creating a restore point
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub sequence_number: u32,
    pub description: String,
    pub method: RestoreMethod,
    /// See `RestorePointRecord::protection_enabled_by_us`.
    pub protection_enabled_by_us: Option<bool>,
}

fn blocked(msg: &str) -> EngineError {
    EngineError::Blocked {
        reason: BlockedReason::new(BlockedCode::NoRestorePoint, msg),
    }
}

/// Turn protection on if needed, allow on-demand points, create one, and prove
/// it exists. The engine lock is held only for the two short journalled steps,
/// never across the slow Windows calls.
///
/// `progress(stage, message)` is advisory.
pub fn create_restore_point(
    engine: &Mutex<Engine>,
    svc: &RestoreService,
    progress: &dyn Fn(&str, &str),
) -> Result<RestoreOutcome> {
    progress("restore_check", "Checking System Protection");
    let status = svc.status();
    if status.supported.is_no() {
        return Err(blocked("System Restore is not available on Windows Server."));
    }
    if status.disabled_by_policy {
        return Err(blocked(
            "System Restore is turned off by Group Policy (DisableSR), so Windows will not make a restore point.",
        ));
    }
    // Off before we enabled it: we turned it on. On: we did not. Unknown stays
    // unknown rather than being recorded as either.
    let protection_enabled_by_us = match &status.protection {
        Probe::No { .. } => Some(true),
        Probe::Yes { .. } => Some(false),
        Probe::Unknown { .. } => None,
    };

    progress("restore_enable", "Turning on System Protection");
    svc.ops.enable_protection()?;

    progress("restore_frequency", "Allowing a restore point right now");
    lock(engine)?.ensure_restore_frequency()?;

    let before = svc.ops.list_points()?;
    let max_before = before.iter().map(|p| p.sequence_number).max().unwrap_or(0);

    progress("restore_create", "Creating the restore point (this can take a minute)");
    let created = svc.ops.create_point(RESTORE_DESCRIPTION)?;

    progress("restore_verify", "Checking Windows recorded it");
    let point = verify_new_point(svc, max_before, created.sequence_number)?;

    lock(engine)?.record_restore_point(
        point.sequence_number,
        &point.description,
        created.method,
        protection_enabled_by_us,
    )?;
    svc.invalidate();

    Ok(RestoreOutcome {
        sequence_number: point.sequence_number,
        description: point.description,
        method: created.method,
        protection_enabled_by_us,
    })
}

fn lock(engine: &Mutex<Engine>) -> Result<std::sync::MutexGuard<'_, Engine>> {
    engine.lock().map_err(|_| EngineError::Internal {
        detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
    })
}

fn verify_new_point(svc: &RestoreService, max_before: u32, expected: Option<u32>) -> Result<RestorePoint> {
    for attempt in 0..svc.verify_attempts.max(1) {
        if attempt > 0 {
            std::thread::sleep(svc.verify_interval);
        }
        let after = svc.ops.list_points()?;
        let newer: Vec<&RestorePoint> = after.iter().filter(|p| p.sequence_number > max_before).collect();
        // Ours only: the number Windows gave us, else our own description. A
        // newer point with neither is another program's (Windows Update makes
        // them too) and is never recorded as ours.
        let pick = expected
            .and_then(|n| newer.iter().copied().find(|p| p.sequence_number == n))
            .or_else(|| newer.iter().copied().find(|p| p.description == RESTORE_DESCRIPTION));
        if let Some(p) = pick {
            return Ok(p.clone());
        }
    }
    Err(EngineError::Command {
        what: "Create restore point".into(),
        exit_code: None,
        detail: "Windows reported success but no new restore point appeared. Common causes: System Protection has no \
                 disk space allocated, or Windows skipped the point because one was made recently."
            .into(),
    })
}

// ---------------------------------------------------------------------------
// Fake ops for tests
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "test-support"))]
pub use fake::FakeRestoreOps;

#[cfg(any(test, feature = "test-support"))]
mod fake {
    use super::*;

    #[derive(Default)]
    pub struct State {
        pub points: Vec<RestorePoint>,
        pub enable_calls: u32,
        pub create_calls: u32,
        pub enable_error: Option<String>,
        pub create_error: Option<String>,
        /// When true, `create_point` claims success but adds nothing.
        pub create_silently_does_nothing: bool,
        pub protection: Option<Probe<()>>,
        pub next_sequence: u32,
        pub created_at_ms: u64,
        /// Another program's point that appears while ours is being made.
        pub foreign_point_during_create: Option<RestorePoint>,
    }

    #[derive(Default)]
    pub struct FakeRestoreOps {
        pub state: Mutex<State>,
    }

    impl FakeRestoreOps {
        pub fn new() -> Self {
            let f = Self::default();
            f.state.lock().unwrap().next_sequence = 100;
            f
        }

        pub fn with_point(self, seq: u32, description: &str, created_unix_ms: Option<u64>) -> Self {
            self.state.lock().unwrap().points.push(RestorePoint {
                sequence_number: seq,
                description: description.to_owned(),
                created_unix_ms,
            });
            self
        }

        pub fn creating_at(self, unix_ms: u64) -> Self {
            self.state.lock().unwrap().created_at_ms = unix_ms;
            self
        }
    }

    impl RestoreOps for FakeRestoreOps {
        fn list_points(&self) -> Result<Vec<RestorePoint>> {
            Ok(self.state.lock().unwrap().points.clone())
        }

        fn enable_protection(&self) -> Result<()> {
            let mut s = self.state.lock().unwrap();
            s.enable_calls += 1;
            match &s.enable_error {
                Some(d) => Err(EngineError::Command {
                    what: "Enable-ComputerRestore".into(),
                    exit_code: Some(1),
                    detail: d.clone(),
                }),
                None => Ok(()),
            }
        }

        fn create_point(&self, description: &str) -> Result<CreatedVia> {
            let mut s = self.state.lock().unwrap();
            s.create_calls += 1;
            if let Some(d) = s.create_error.clone() {
                return Err(EngineError::Command {
                    what: "Checkpoint-Computer".into(),
                    exit_code: Some(1),
                    detail: d,
                });
            }
            if let Some(p) = s.foreign_point_during_create.take() {
                s.points.push(p);
            }
            let seq = s.next_sequence;
            s.next_sequence += 1;
            if !s.create_silently_does_nothing {
                let at = s.created_at_ms;
                s.points.push(RestorePoint {
                    sequence_number: seq,
                    description: description.to_owned(),
                    created_unix_ms: Some(at),
                });
            }
            Ok(CreatedVia {
                method: RestoreMethod::Api,
                sequence_number: Some(seq),
            })
        }

        fn protection_enabled(&self) -> Probe<()> {
            self.state
                .lock()
                .unwrap()
                .protection
                .clone()
                .unwrap_or_else(|| Probe::unknown("fake"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::registry::fake::FakeRegistry;
    use crate::testutil::*;
    use crate::types::{RawValue, Tweak};
    use crate::wmi::{FakeWmi, WmiValue};

    const HOUR: u64 = 3_600_000;
    const NOW: u64 = 1_800_000_000_000;

    struct FixedClock(AtomicU64);
    impl Clock for FixedClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn product_type(n: u64) -> FakeWmi {
        FakeWmi::new().with_rows(
            NS_CIMV2,
            WQL_PRODUCT_TYPE,
            vec![vec![("ProductType", WmiValue::UInt(n))]],
        )
    }

    struct Rig {
        svc: RestoreService,
        ops: Arc<FakeRestoreOps>,
        reg: Arc<FakeRegistry>,
    }

    fn rig(ops: FakeRestoreOps, wmi: FakeWmi) -> Rig {
        let ops = Arc::new(ops);
        let reg = Arc::new(FakeRegistry::new());
        let svc = RestoreService::new(ops.clone(), reg.clone(), Arc::new(wmi))
            .with_clock(Arc::new(FixedClock(AtomicU64::new(NOW))))
            .without_waiting();
        Rig { svc, ops, reg }
    }

    // ---- gate ------------------------------------------------------------

    #[test]
    fn gate_is_open_only_for_a_fresh_point_with_a_readable_time() {
        let pts = |v: Vec<RestorePoint>| Probe::yes(v);
        let p = |seq, at| RestorePoint {
            sequence_number: seq,
            description: String::new(),
            created_unix_ms: at,
        };
        assert!(gate_from(&pts(vec![p(1, Some(NOW - 23 * HOUR))]), NOW));
        assert!(
            gate_from(&pts(vec![p(1, Some(NOW - 24 * HOUR))]), NOW),
            "24 h exactly is still fresh"
        );
        assert!(!gate_from(&pts(vec![p(1, Some(NOW - 25 * HOUR))]), NOW));
        assert!(
            !gate_from(&pts(vec![p(1, None)]), NOW),
            "an unreadable time is never assumed recent"
        );
        assert!(!gate_from(&pts(vec![]), NOW));
        assert!(!gate_from(&Probe::unknown("no WMI"), NOW));
        // Newest wins.
        assert!(gate_from(
            &pts(vec![p(1, Some(NOW - 90 * HOUR)), p(2, Some(NOW - HOUR))]),
            NOW
        ));
    }

    #[test]
    fn gate_answers_are_cached_briefly_and_invalidate_forces_a_reread() {
        let r = rig(
            FakeRestoreOps::new().with_point(1, "old", Some(NOW - 100 * HOUR)),
            product_type(1),
        );
        assert!(!r.svc.gate_open());
        // A fresh point appears behind our back; the cached answer still stands...
        r.ops.state.lock().unwrap().points.push(RestorePoint {
            sequence_number: 2,
            description: "new".into(),
            created_unix_ms: Some(NOW),
        });
        assert!(!r.svc.gate_open());
        // ...until it is invalidated.
        r.svc.invalidate();
        assert!(r.svc.gate_open());
    }

    // ---- status ----------------------------------------------------------

    #[test]
    fn status_reports_server_policy_frequency_and_age() {
        let r = rig(
            FakeRestoreOps::new().with_point(1, "x", Some(NOW - 5 * HOUR)),
            product_type(3),
        );
        r.reg
            .set_external(Hive::LocalMachine, POLICY_KEY, POLICY_VALUE, RawValue::dword(1));
        r.reg
            .set_external(Hive::LocalMachine, CONFIG_KEY, FREQUENCY_VALUE, RawValue::dword(1440));
        let s = r.svc.status();
        assert!(s.supported.is_no());
        assert!(s.disabled_by_policy);
        assert_eq!(s.creation_frequency_minutes, Some(1440));
        assert_eq!(s.newest_point_age_hours, Some(5));
        assert!(s.gate_open);
        assert!(s.protection.is_unknown());

        let client = rig(FakeRestoreOps::new(), product_type(1)).svc.status();
        assert!(client.supported.is_yes());
        assert!(!client.disabled_by_policy);
        assert_eq!(client.newest_point_age_hours, None);
    }

    #[test]
    fn wmi_restore_points_parse_and_skip_rows_without_a_sequence_number() {
        let wmi = FakeWmi::new().with_rows(
            NS_DEFAULT,
            WQL_RESTORE_POINTS,
            vec![
                vec![
                    ("SequenceNumber", WmiValue::UInt(7)),
                    ("Description", WmiValue::Str("Windows Update".into())),
                    ("CreationTime", WmiValue::Str("20240101000000.000000+000".into())),
                ],
                vec![("Description", WmiValue::Str("broken".into()))],
                vec![
                    ("SequenceNumber", WmiValue::UInt(8)),
                    ("CreationTime", WmiValue::Str("garbage".into())),
                ],
            ],
        );
        let pts = list_points_wmi(&wmi).unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].created_unix_ms, Some(19_723 * 86_400_000));
        assert_eq!(pts[1].created_unix_ms, None);
        // On a machine without System Restore the class is missing: an error, not an empty list.
        assert!(list_points_wmi(&FakeWmi::new()).is_err());
    }

    // ---- create flow -----------------------------------------------------

    fn engine_for(reg: &Arc<FakeRegistry>, gate: Arc<RestoreService>) -> (Mutex<Engine>, tempfile::TempDir) {
        struct Probe2(Arc<RestoreService>);
        impl crate::env::EnvProbe for Probe2 {
            fn probe(&self, elevated: bool) -> crate::types::SystemEnv {
                crate::types::SystemEnv {
                    elevated,
                    restore_gate_open: self.0.gate_open(),
                    ..Default::default()
                }
            }
            fn restore_gate_open(&self) -> bool {
                self.0.gate_open()
            }
            fn invalidate(&self) {
                self.0.invalidate();
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let resolver = crate::context::ContextResolver::new(user(true), true, reg.clone());
        let journal =
            crate::journal::Journal::open(&crate::secure_dir::TrustedDir::insecure_for_tests(dir.path())).unwrap();
        let engine = Engine::new(
            resolver,
            journal,
            vec![Box::new(TestTweak::new("t", r"SOFTWARE\PeakTest", &[("A", 1)])) as Box<dyn Tweak>],
            Box::new(Probe2(gate)),
            crate::env::License::dev(crate::types::Tier::Ultimate),
        );
        (Mutex::new(engine), dir)
    }

    fn no_progress(_: &str, _: &str) {}

    #[test]
    fn creating_a_restore_point_opens_the_gate_and_journals_everything() {
        let ops = FakeRestoreOps::new().creating_at(NOW);
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());

        // Before: no point, so the engine refuses to apply anything.
        assert!(!svc.gate_open());
        assert!(matches!(
            engine.lock().unwrap().apply("t"),
            Err(EngineError::Blocked { reason }) if reason.code == BlockedCode::NoRestorePoint
        ));

        let out = create_restore_point(&engine, &svc, &no_progress).unwrap();
        assert_eq!(out.sequence_number, 100);
        assert_eq!(out.description, RESTORE_DESCRIPTION);
        assert_eq!(out.method, RestoreMethod::Api);
        assert_eq!(
            out.protection_enabled_by_us, None,
            "protection state was Unknown, so whether we turned it on is unknown"
        );
        assert_eq!(r.ops.state.lock().unwrap().enable_calls, 1);
        assert_eq!(r.ops.state.lock().unwrap().create_calls, 1);

        // The frequency setting went through Transaction: value set, backup + journal written.
        assert_eq!(
            hklm_dword(
                &r.reg,
                "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\SystemRestore",
                FREQUENCY_VALUE
            ),
            Some(0)
        );
        let view = engine.lock().unwrap().journal_view();
        assert!(view.records.iter().any(|rec| matches!(
            rec,
            crate::journal::Record::Write(w) if w.tweak_id == "system.restore.frequency"
        )));
        assert!(view.records.iter().any(|rec| matches!(
            rec,
            crate::journal::Record::RestorePoint(p) if p.sequence_number == 100 && p.method == RestoreMethod::Api
        )));

        // After: the gate is open and a real apply goes through.
        assert!(svc.gate_open());
        engine.lock().unwrap().apply("t").unwrap();
    }

    #[test]
    fn reverting_everything_also_puts_the_restore_frequency_back() {
        let r = rig(FakeRestoreOps::new().creating_at(NOW), product_type(1));
        let cfg = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\SystemRestore";
        r.reg
            .set_external(Hive::LocalMachine, cfg, FREQUENCY_VALUE, RawValue::dword(1440));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        create_restore_point(&engine, &svc, &no_progress).unwrap();
        assert_eq!(hklm_dword(&r.reg, cfg, FREQUENCY_VALUE), Some(0));

        let results = engine.lock().unwrap().revert_all();
        assert!(results.iter().all(|x| x.ok), "{results:?}");
        assert!(results.iter().any(|x| x.tweak_id == "system.restore.frequency"));
        assert_eq!(hklm_dword(&r.reg, cfg, FREQUENCY_VALUE), Some(1440));
    }

    #[test]
    fn the_bootstrap_tweak_is_not_reachable_through_apply() {
        let r = rig(FakeRestoreOps::new(), product_type(1));
        let (engine, _dir) = engine_for(&r.reg, Arc::new(r.svc));
        assert!(matches!(
            engine.lock().unwrap().apply("system.restore.frequency"),
            Err(EngineError::UnknownTweak { .. })
        ));
        assert!(engine
            .lock()
            .unwrap()
            .list()
            .unwrap()
            .iter()
            .all(|v| &*v.metadata.id != "system.restore.frequency"));
    }

    #[test]
    fn protection_that_was_definitely_off_is_recorded_as_turned_on_by_us() {
        let ops = FakeRestoreOps::new().creating_at(NOW);
        ops.state.lock().unwrap().protection = Some(Probe::no("off"));
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        assert_eq!(
            create_restore_point(&engine, &svc, &no_progress)
                .unwrap()
                .protection_enabled_by_us,
            Some(true)
        );
    }

    #[test]
    fn protection_that_was_already_on_is_recorded_as_not_turned_on_by_us() {
        let ops = FakeRestoreOps::new().creating_at(NOW);
        ops.state.lock().unwrap().protection = Some(Probe::yes(()));
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        assert_eq!(
            create_restore_point(&engine, &svc, &no_progress)
                .unwrap()
                .protection_enabled_by_us,
            Some(false)
        );
    }

    #[test]
    fn windows_server_and_group_policy_are_refused_before_anything_is_touched() {
        let r = rig(FakeRestoreOps::new(), product_type(3));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        let err = create_restore_point(&engine, &svc, &no_progress).unwrap_err();
        assert!(
            matches!(&err, EngineError::Blocked { reason } if reason.message.contains("Server")),
            "{err:?}"
        );
        assert_eq!(r.ops.state.lock().unwrap().enable_calls, 0);
        assert!(r.reg.snapshot().is_empty());

        let r = rig(FakeRestoreOps::new(), product_type(1));
        r.reg
            .set_external(Hive::LocalMachine, POLICY_KEY, POLICY_VALUE, RawValue::dword(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        let err = create_restore_point(&engine, &svc, &no_progress).unwrap_err();
        assert!(
            matches!(&err, EngineError::Blocked { reason } if reason.message.contains("Group Policy")),
            "{err:?}"
        );
        assert_eq!(r.ops.state.lock().unwrap().enable_calls, 0);
    }

    #[test]
    fn a_failure_to_enable_protection_stops_the_flow() {
        let ops = FakeRestoreOps::new();
        ops.state.lock().unwrap().enable_error = Some("Access is denied".into());
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        assert!(create_restore_point(&engine, &svc, &no_progress).is_err());
        assert_eq!(r.ops.state.lock().unwrap().create_calls, 0);
        assert!(r.reg.snapshot().is_empty(), "no registry change was made");
    }

    #[test]
    fn success_that_produces_no_restore_point_is_reported_not_believed() {
        let ops = FakeRestoreOps::new().creating_at(NOW);
        ops.state.lock().unwrap().create_silently_does_nothing = true;
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        let err = create_restore_point(&engine, &svc, &no_progress).unwrap_err();
        assert!(
            matches!(&err, EngineError::Command { detail, .. } if detail.contains("no new restore point")),
            "{err:?}"
        );
        assert!(!svc.gate_open(), "the gate must stay closed");
        assert!(!engine
            .lock()
            .unwrap()
            .journal_view()
            .records
            .iter()
            .any(|rec| matches!(rec, crate::journal::Record::RestorePoint(_))));
    }

    #[test]
    fn an_older_existing_point_does_not_count_as_the_new_one() {
        // Windows had point 50 before; creation "succeeds" without adding anything.
        let ops = FakeRestoreOps::new()
            .with_point(50, "Windows Update", Some(NOW - 2 * HOUR))
            .creating_at(NOW);
        ops.state.lock().unwrap().create_silently_does_nothing = true;
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        assert!(create_restore_point(&engine, &svc, &no_progress).is_err());
    }

    #[test]
    fn another_programs_new_point_is_never_taken_for_ours() {
        // Ours is silently skipped while Windows Update makes one at the same
        // moment, with a number Windows did not give us.
        let ops = FakeRestoreOps::new().creating_at(NOW);
        {
            let mut s = ops.state.lock().unwrap();
            s.create_silently_does_nothing = true;
            s.foreign_point_during_create = Some(RestorePoint {
                sequence_number: 300,
                description: "Windows Update".into(),
                created_unix_ms: Some(NOW),
            });
        }
        let r = rig(ops, product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        let err = create_restore_point(&engine, &svc, &no_progress).unwrap_err();
        assert!(
            matches!(&err, EngineError::Command { detail, .. } if detail.contains("no new restore point")),
            "{err:?}"
        );
        assert!(!engine
            .lock()
            .unwrap()
            .journal_view()
            .records
            .iter()
            .any(|rec| matches!(rec, crate::journal::Record::RestorePoint(_))));
    }

    #[test]
    fn progress_is_reported_in_order() {
        let r = rig(FakeRestoreOps::new().creating_at(NOW), product_type(1));
        let svc = Arc::new(r.svc);
        let (engine, _dir) = engine_for(&r.reg, svc.clone());
        let seen = Mutex::new(Vec::new());
        create_restore_point(&engine, &svc, &|stage, _| seen.lock().unwrap().push(stage.to_owned())).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "restore_check",
                "restore_enable",
                "restore_frequency",
                "restore_create",
                "restore_verify"
            ]
        );
    }
}
