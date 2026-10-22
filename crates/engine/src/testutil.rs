//! Shared test scaffolding: a configurable tweak and a harness that wires an
//! engine to the in-memory registry and a temp journal directory.

use std::sync::Arc;

use crate::context::{ContextResolver, UserContext, UserResolution};
use crate::engine::Engine;
use crate::env::{License, StubProbe};
use crate::error::{EngineError, Result};
use crate::journal::Journal;
use crate::registry::fake::FakeRegistry;
use crate::registry::Hive;
use crate::secure_dir::TrustedDir;
use crate::system::FakeSystem;
use crate::transaction::Transaction;
use crate::types::{
    ExecutionContext, Impact, PredicateOutcome, RawValue, RegRoot, RegTarget, SafetyTier, SystemEnv, Tier, Tweak,
    TweakMetadata, TweakState,
};

pub const SID: &str = "S-1-5-21-1-2-3-1001";

/// Writes fixed DWORDs under one key. `allow` can be narrower than `writes` to
/// provoke an allowlist violation.
pub struct TestTweak {
    pub id: String,
    pub context: ExecutionContext,
    pub key: String,
    /// The key declared in `touches()`; may hold a `*` segment. Defaults to `key`.
    pub allow_key: String,
    pub writes: Vec<(String, u32)>,
    pub allow: Vec<String>,
    pub tier: Tier,
    pub block: bool,
    pub fail_read: bool,
}

impl TestTweak {
    pub fn new(id: &str, key: &str, writes: &[(&str, u32)]) -> Self {
        Self {
            id: id.into(),
            context: ExecutionContext::Service,
            key: key.into(),
            allow_key: key.into(),
            writes: writes.iter().map(|(n, v)| ((*n).to_owned(), *v)).collect(),
            allow: writes.iter().map(|(n, _)| (*n).to_owned()).collect(),
            tier: Tier::Free,
            block: false,
            fail_read: false,
        }
    }

    pub fn root(&self) -> RegRoot {
        match self.context {
            ExecutionContext::User => RegRoot::InteractiveUser,
            _ => RegRoot::LocalMachine,
        }
    }
}

impl Tweak for TestTweak {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id.clone().into(),
            name: "test".into(),
            summary: "test".into(),
            target: "test".into(),
            category: "test".into(),
            tier: self.tier,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        self.context
    }

    fn touches(&self) -> Vec<RegTarget> {
        let names: Vec<&str> = self.allow.iter().map(String::as_str).collect();
        vec![RegTarget::new(self.root(), self.allow_key.clone(), &names)]
    }

    fn evaluate_predicate(&self, _env: &SystemEnv) -> PredicateOutcome {
        if self.block {
            PredicateOutcome::Block(crate::types::BlockedReason::new(
                crate::types::BlockedCode::HardwareUnsupported,
                "test block",
            ))
        } else {
            PredicateOutcome::Allow
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        if self.fail_read {
            return Err(EngineError::registry_msg("test", None, "cannot read"));
        }
        let all_set = self.writes.iter().all(|(n, v)| {
            res.read_dword(self.root(), &self.key, n)
                .ok()
                .flatten()
                .is_some_and(|cur| cur == *v)
        });
        Ok(match (all_set, has_journal_entry) {
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::Foreign,
            _ => TweakState::Default,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        for (n, v) in &self.writes {
            tx.set_dword(self.root(), &self.key, n, *v)?;
        }
        Ok(())
    }
}

pub fn user(is_self: bool) -> UserContext {
    UserContext {
        sid: SID.into(),
        resolution: if is_self {
            UserResolution::OwnToken
        } else {
            UserResolution::InteractiveShell
        },
        is_self,
    }
}

pub struct Harness {
    pub fake: Arc<FakeRegistry>,
    /// Non-registry state (power plans, services, files, side effects).
    pub sys: Arc<FakeSystem>,
    pub dir: tempfile::TempDir,
    pub engine: Engine,
}

impl Harness {
    pub fn new(tweaks: Vec<Box<dyn Tweak>>) -> Self {
        Self::with(
            tweaks,
            Arc::new(FakeRegistry::new()),
            tempfile::tempdir().unwrap(),
            true,
        )
    }

    pub fn with(tweaks: Vec<Box<dyn Tweak>>, fake: Arc<FakeRegistry>, dir: tempfile::TempDir, gate_open: bool) -> Self {
        let sys = Arc::new(FakeSystem::new());
        let engine = build_engine_with_system(&fake, &sys, dir.path(), tweaks, gate_open, Tier::Ultimate);
        Self { fake, sys, dir, engine }
    }

    /// A new engine over the same registry and journal directory, as after an
    /// app restart.
    pub fn restart(&mut self, tweaks: Vec<Box<dyn Tweak>>) {
        self.engine = build_engine_with_system(&self.fake, &self.sys, self.dir.path(), tweaks, true, Tier::Ultimate);
    }
}

pub fn build_engine(
    fake: &Arc<FakeRegistry>,
    dir: &std::path::Path,
    tweaks: Vec<Box<dyn Tweak>>,
    gate_open: bool,
    tier: Tier,
) -> Engine {
    let resolver = ContextResolver::new(user(true), true, fake.clone());
    let journal = Journal::open(&TrustedDir::insecure_for_tests(dir)).unwrap();
    let probe = if gate_open {
        StubProbe::open_for_dev()
    } else {
        StubProbe::closed()
    };
    Engine::new(resolver, journal, tweaks, Box::new(probe), License::dev(tier))
}

/// `build_engine` with a fake for non-registry changes.
pub fn build_engine_with_system(
    fake: &Arc<FakeRegistry>,
    sys: &Arc<FakeSystem>,
    dir: &std::path::Path,
    tweaks: Vec<Box<dyn Tweak>>,
    gate_open: bool,
    tier: Tier,
) -> Engine {
    let resolver = ContextResolver::new(user(true), true, fake.clone()).with_system(sys.clone());
    let journal = Journal::open(&TrustedDir::insecure_for_tests(dir)).unwrap();
    let probe = if gate_open {
        StubProbe::open_for_dev()
    } else {
        StubProbe::closed()
    };
    Engine::new(resolver, journal, tweaks, Box::new(probe), License::dev(tier))
}

/// Where Windows' list of profiles says the test user's profile folder is.
pub fn set_profile_dir(fake: &FakeRegistry, path: &str) {
    fake.set_external(
        Hive::LocalMachine,
        &format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{SID}"),
        "ProfileImagePath",
        RawValue::sz(path),
    );
}

pub fn hklm_dword(fake: &FakeRegistry, key: &str, name: &str) -> Option<u32> {
    fake.read_value_for_test(Hive::LocalMachine, key, name)
        .and_then(|v| v.as_dword())
}

pub fn dword(v: u32) -> RawValue {
    RawValue::dword(v)
}

/// A temp folder, named by its real path without `\\?\`, so checks that
/// compare against where Windows says a file is see the same names (Windows
/// runners keep temp under an 8.3 short name, `RUNNER~1`).
pub fn real_temp() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let c = std::fs::canonicalize(tmp.path()).unwrap();
    let base = std::path::PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\"));
    (tmp, base)
}
