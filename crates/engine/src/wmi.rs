//! WMI access behind a trait, plus the one thread that talks to real WMI.
//!
//! Tauri and WebView2 initialise COM on the main thread. Creating WMI objects
//! there mixes apartments and produces `RPC_E_CHANGED_MODE`. So all WMI runs on
//! **one dedicated worker thread** that initialises COM once (multithreaded
//! apartment), owns every `WMIConnection`, takes requests over a channel and
//! answers over per-request channels. Callers never touch COM.
//!
//! Probe logic is written against `WmiSource` and untyped rows (`WmiRow`), so
//! it runs in tests against `FakeWmi` with canned rows.

use std::collections::HashMap;

#[cfg(any(windows, test, feature = "test-support"))]
use super::error::EngineError;
use super::error::Result;

/// A WMI property value, reduced to what the probes need.
#[derive(Debug, Clone, PartialEq)]
pub enum WmiValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Array(Vec<WmiValue>),
}

impl WmiValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// CIM `uint64` properties arrive as strings, so accept those too.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt(v) => Some(*v),
            Self::Int(v) => u64::try_from(*v).ok(),
            Self::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::UInt(v) => i64::try_from(*v).ok(),
            Self::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Every element as u64, or `None` if this is not an array of numbers.
    pub fn as_u64_array(&self) -> Option<Vec<u64>> {
        match self {
            Self::Array(items) => items.iter().map(WmiValue::as_u64).collect(),
            _ => None,
        }
    }
}

/// One WMI object: property name (as WMI returns it) to value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WmiRow(pub HashMap<String, WmiValue>);

impl WmiRow {
    /// Property lookup is case-insensitive, as WMI's is.
    pub fn get(&self, name: &str) -> Option<&WmiValue> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
            .filter(|v| !matches!(v, WmiValue::Null))
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(WmiValue::as_str)
    }

    pub fn u64(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(WmiValue::as_u64)
    }

    pub fn i64(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(WmiValue::as_i64)
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(WmiValue::as_bool)
    }
}

pub const NS_CIMV2: &str = r"ROOT\CIMV2";
pub const NS_DEVICEGUARD: &str = r"ROOT\Microsoft\Windows\DeviceGuard";
pub const NS_TPM: &str = r"ROOT\CIMV2\Security\MicrosoftTpm";
pub const NS_STORAGE: &str = r"ROOT\Microsoft\Windows\Storage";
pub const NS_DEFAULT: &str = r"ROOT\default";

/// Read-only WMI. The query text comes from constants in the probe modules,
/// never from the webview.
pub trait WmiSource: Send + Sync {
    fn query(&self, namespace: &str, wql: &str) -> Result<Vec<WmiRow>>;
}

#[cfg(any(windows, test, feature = "test-support"))]
pub(crate) fn wmi_error(namespace: &str, detail: impl Into<String>, timed_out: bool) -> EngineError {
    EngineError::Wmi {
        namespace: namespace.to_owned(),
        detail: detail.into(),
        timed_out,
    }
}

// ---------------------------------------------------------------------------
// Fake
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "test-support"))]
pub use fake::FakeWmi;

#[cfg(any(test, feature = "test-support"))]
mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    type Answers = HashMap<(String, String), Result<Vec<WmiRow>>>;

    /// Canned answers keyed by (namespace, query), both case-insensitive.
    /// A query with no entry fails like a missing namespace/class would.
    #[derive(Default)]
    pub struct FakeWmi {
        answers: Mutex<Answers>,
        pub calls: Mutex<Vec<(String, String)>>,
    }

    fn key(ns: &str, wql: &str) -> (String, String) {
        (
            ns.to_ascii_lowercase(),
            wql.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase(),
        )
    }

    impl FakeWmi {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_rows(self, ns: &str, wql: &str, rows: Vec<Vec<(&str, WmiValue)>>) -> Self {
            let rows = rows
                .into_iter()
                .map(|r| WmiRow(r.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()))
                .collect();
            self.answers.lock().unwrap().insert(key(ns, wql), Ok(rows));
            self
        }

        pub fn with_error(self, ns: &str, wql: &str, detail: &str) -> Self {
            self.answers
                .lock()
                .unwrap()
                .insert(key(ns, wql), Err(wmi_error(ns, detail, false)));
            self
        }
    }

    impl WmiSource for FakeWmi {
        fn query(&self, namespace: &str, wql: &str) -> Result<Vec<WmiRow>> {
            self.calls.lock().unwrap().push((namespace.to_owned(), wql.to_owned()));
            match self.answers.lock().unwrap().get(&key(namespace, wql)) {
                Some(r) => r.clone(),
                None => Err(wmi_error(
                    namespace,
                    "invalid namespace or class (no canned answer)",
                    false,
                )),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The real thing
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub use worker::WmiWorker;

#[cfg(windows)]
mod worker {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::Mutex;
    use std::time::Duration;

    use wmi::{COMLibrary, Variant, WMIConnection};

    use super::*;
    use crate::journal::now_ms;

    /// How long one query may take before the caller gives up on it.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

    struct Job {
        namespace: String,
        wql: String,
        reply: mpsc::Sender<Result<Vec<WmiRow>>>,
    }

    pub struct WmiWorker {
        tx: Mutex<mpsc::Sender<Job>>,
        /// Unix ms when the worker started its current job; 0 when idle.
        busy_since: std::sync::Arc<AtomicU64>,
        timeout: Duration,
    }

    impl WmiWorker {
        pub fn start() -> Self {
            Self::start_with_timeout(DEFAULT_TIMEOUT)
        }

        pub fn start_with_timeout(timeout: Duration) -> Self {
            let (tx, rx) = mpsc::channel::<Job>();
            let busy_since = std::sync::Arc::new(AtomicU64::new(0));
            let busy = busy_since.clone();
            std::thread::Builder::new()
                .name("peaktweaks-wmi".into())
                .spawn(move || run(rx, busy))
                .expect("spawn WMI worker thread");
            Self {
                tx: Mutex::new(tx),
                busy_since,
                timeout,
            }
        }
    }

    impl WmiSource for WmiWorker {
        fn query(&self, namespace: &str, wql: &str) -> Result<Vec<WmiRow>> {
            // A query that outlived its caller's timeout keeps the worker busy;
            // later requests would just queue behind it and time out too.
            let since = self.busy_since.load(Ordering::Relaxed);
            let grace = (self.timeout.as_millis() as u64).saturating_mul(2);
            if since != 0 && now_ms().saturating_sub(since) > grace {
                return Err(wmi_error(
                    namespace,
                    "the WMI worker is stuck on an earlier query",
                    true,
                ));
            }

            let (reply, rx) = mpsc::channel();
            self.tx
                .lock()
                .map_err(|_| wmi_error(namespace, "WMI worker channel poisoned", false))?
                .send(Job {
                    namespace: namespace.to_owned(),
                    wql: wql.to_owned(),
                    reply,
                })
                .map_err(|_| wmi_error(namespace, "WMI worker thread has exited", false))?;

            match rx.recv_timeout(self.timeout) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => Err(wmi_error(
                    namespace,
                    format!("query did not finish within {} s", self.timeout.as_secs()),
                    true,
                )),
                Err(RecvTimeoutError::Disconnected) => Err(wmi_error(namespace, "WMI worker thread died", false)),
            }
        }
    }

    fn run(rx: mpsc::Receiver<Job>, busy_since: std::sync::Arc<AtomicU64>) {
        // `COMLibrary::new` initialises COM in the multithreaded apartment and
        // then security. `RPC_E_TOO_LATE` (security already set process-wide,
        // for example by WebView2) is handled inside the crate. A fresh thread
        // cannot hit `RPC_E_CHANGED_MODE`; if it somehow does, every request
        // gets that error rather than a guess.
        let com = COMLibrary::new().map_err(|e| e.to_string());
        let mut conns: HashMap<String, WMIConnection> = HashMap::new();

        while let Ok(job) = rx.recv() {
            busy_since.store(now_ms(), Ordering::Relaxed);
            let result = match &com {
                Err(e) => Err(wmi_error(
                    &job.namespace,
                    format!("COM initialisation failed: {e}"),
                    false,
                )),
                Ok(com) => query_one(com, &mut conns, &job.namespace, &job.wql),
            };
            busy_since.store(0, Ordering::Relaxed);
            let _ = job.reply.send(result); // the caller may have timed out and gone
        }
    }

    fn query_one(
        com: &COMLibrary,
        conns: &mut HashMap<String, WMIConnection>,
        namespace: &str,
        wql: &str,
    ) -> Result<Vec<WmiRow>> {
        let key = namespace.to_ascii_lowercase();
        if !conns.contains_key(&key) {
            let c = WMIConnection::with_namespace_path(namespace, *com)
                .map_err(|e| wmi_error(namespace, format!("cannot connect: {e}"), false))?;
            conns.insert(key.clone(), c);
        }
        let conn = &conns[&key];
        let raw: Vec<HashMap<String, Variant>> = conn
            .raw_query(wql)
            .map_err(|e| wmi_error(namespace, format!("query failed: {e}"), false))?;
        Ok(raw
            .into_iter()
            .map(|row| WmiRow(row.into_iter().map(|(k, v)| (k, convert(v))).collect()))
            .collect())
    }

    fn convert(v: Variant) -> WmiValue {
        match v {
            Variant::Empty | Variant::Null => WmiValue::Null,
            Variant::String(s) => WmiValue::Str(s),
            Variant::I1(n) => WmiValue::Int(n.into()),
            Variant::I2(n) => WmiValue::Int(n.into()),
            Variant::I4(n) => WmiValue::Int(n.into()),
            Variant::I8(n) => WmiValue::Int(n),
            Variant::UI1(n) => WmiValue::UInt(n.into()),
            Variant::UI2(n) => WmiValue::UInt(n.into()),
            Variant::UI4(n) => WmiValue::UInt(n.into()),
            Variant::UI8(n) => WmiValue::UInt(n),
            Variant::R4(n) => WmiValue::Float(n.into()),
            Variant::R8(n) => WmiValue::Float(n),
            Variant::Bool(b) => WmiValue::Bool(b),
            Variant::Array(items) => WmiValue::Array(items.into_iter().map(convert).collect()),
            // Embedded objects and raw interface pointers: not used by any probe.
            Variant::Unknown(_) | Variant::Object(_) => WmiValue::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint64_strings_parse_and_lookup_ignores_case() {
        let row = WmiRow(
            [
                ("Capacity".to_owned(), WmiValue::Str("8589934592".into())),
                ("Speed".to_owned(), WmiValue::UInt(3200)),
                ("Gone".to_owned(), WmiValue::Null),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(row.u64("capacity"), Some(8_589_934_592));
        assert_eq!(row.u64("SPEED"), Some(3200));
        assert_eq!(row.u64("Gone"), None);
        assert_eq!(row.u64("Missing"), None);
    }

    #[test]
    fn arrays_of_numbers() {
        let v = WmiValue::Array(vec![WmiValue::UInt(1), WmiValue::Int(2)]);
        assert_eq!(v.as_u64_array(), Some(vec![1, 2]));
        assert_eq!(WmiValue::Array(vec![WmiValue::Str("x".into())]).as_u64_array(), None);
        assert_eq!(WmiValue::Int(-1).as_u64(), None);
    }

    #[test]
    fn fake_matches_queries_ignoring_case_and_whitespace_and_fails_for_unknown_ones() {
        let f = FakeWmi::new().with_rows(NS_CIMV2, "SELECT  A FROM  B", vec![vec![("A", WmiValue::UInt(1))]]);
        assert_eq!(f.query("root\\cimv2", "select a from b").unwrap().len(), 1);
        assert!(f.query(NS_CIMV2, "SELECT Z FROM Y").is_err());
    }
}

#[cfg(all(test, windows))]
mod live_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

    use super::*;
    use crate::hardware::WQL_OS;

    #[test]
    fn the_worker_answers_a_real_query() {
        let w = WmiWorker::start();
        let rows = w
            .query(NS_CIMV2, WQL_OS)
            .expect("Win32_OperatingSystem must be queryable");
        assert_eq!(rows.len(), 1);
        let build = rows[0].u64("BuildNumber").expect("BuildNumber");
        assert!(build >= 10_240, "build {build}");
        println!("OS: {:?} build {build}", rows[0].str("Caption"));
    }

    /// The main thread of a Tauri app is a single-threaded apartment. WMI work
    /// must not care, because it never runs there.
    #[test]
    fn a_caller_in_a_single_threaded_apartment_can_still_query() {
        let w = Arc::new(WmiWorker::start());
        let w2 = w.clone();
        std::thread::spawn(move || {
            unsafe {
                let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                assert!(hr.is_ok(), "{hr:?}");
            }
            assert_eq!(w2.query(NS_CIMV2, WQL_OS).unwrap().len(), 1);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn concurrent_callers_are_serialised_not_broken() {
        let w = Arc::new(WmiWorker::start());
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let w = w.clone();
                std::thread::spawn(move || w.query(NS_CIMV2, WQL_OS).map(|r| r.len()))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().unwrap(), 1);
        }
    }

    #[test]
    fn a_missing_class_is_an_error_not_a_hang() {
        let w = WmiWorker::start();
        let t = std::time::Instant::now();
        let e = w
            .query(NS_CIMV2, "SELECT * FROM Win32_ThisClassDoesNotExist")
            .unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(20));
        println!("missing class error: {e}");
        assert!(matches!(e, EngineError::Wmi { timed_out: false, .. }), "{e:?}");
    }

    #[test]
    fn a_missing_namespace_is_reported_with_the_hresult_the_probes_look_for() {
        let w = WmiWorker::start();
        let e = w
            .query(r"ROOT\NoSuchNamespaceForPeakTweaks", "SELECT * FROM X")
            .unwrap_err();
        println!("missing namespace error: {e}");
        assert!(
            e.to_string().to_ascii_lowercase().contains("0x8004100e"),
            "the TPM probe relies on this HRESULT appearing in the text: {e}"
        );
    }
}
