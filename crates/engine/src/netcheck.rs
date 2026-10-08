//! Connection check (catalogue E4, ExitLag's "Network Analyzer"): round
//! trips, jitter and lost packets to this PC's router and to two public DNS
//! servers, so the user can see whether a problem is in the home network or
//! past it.
//!
//! Started by the user only, and the only thing sent is ICMP echo requests
//! (what `ping` sends) with a fixed payload: nothing about the user or the PC
//! (DECISIONS 15.22). It changes nothing, so there is nothing to undo and no
//! restore point.
//!
//! Each target gets `ECHOES` echoes `GAP_MS` apart, the targets side by side.
//! Jitter is the mean difference between one answered round trip and the
//! next, as RFC 3550 (section 6.4.1) describes it without its smoothing.
//! Game servers are not asked yet: their addresses are not published in a
//! form this can use (NOTES N88).
//!
//! Windows: `IcmpSendEcho` (IP Helper, no networking library) and the router
//! from `GetBestRoute` towards the first public server. IPv4 only.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{EngineError, Result};

/// Echoes per target.
pub const ECHOES: u32 = 20;
/// How long to wait for each answer.
pub const TIMEOUT_MS: u32 = 1000;
/// Pause between one echo and the next to the same target.
pub const GAP_MS: u64 = 50;

/// What was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PingTarget {
    /// The router this PC sends internet traffic through.
    Router,
    /// Cloudflare's public DNS server, 1.1.1.1.
    Cloudflare,
    /// Google's public DNS server, 8.8.8.8.
    Google,
}

impl PingTarget {
    pub const ALL: [Self; 3] = [Self::Router, Self::Cloudflare, Self::Google];

    /// The fixed address, for the public servers.
    pub const fn address(self) -> Option<Ipv4Addr> {
        match self {
            Self::Router => None,
            Self::Cloudflare => Some(Ipv4Addr::new(1, 1, 1, 1)),
            Self::Google => Some(Ipv4Addr::new(8, 8, 8, 8)),
        }
    }
}

/// One target's answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    pub target: PingTarget,
    /// The address asked; for the router, the one Windows uses, when found.
    pub address: Option<String>,
    pub sent: u32,
    pub received: u32,
    /// Round trips of the answered echoes, in milliseconds (Windows counts
    /// whole milliseconds, so 0 means under 1 ms).
    pub min_ms: Option<u32>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<u32>,
    /// Mean change from one answered round trip to the next; needs two.
    pub jitter_ms: Option<f64>,
    /// Why this target was not asked, or stopped early, in plain words.
    pub problem: Option<String>,
}

/// Where the trouble seems to be, from the pattern of answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionReading {
    /// Every target answered every echo.
    AllAnswered,
    /// The router itself lost echoes: the trouble is between this PC and the
    /// router (often Wi-Fi).
    LossToRouter,
    /// The router answered every echo, a public server did not: the trouble
    /// is past the router.
    LossPastRouter,
    /// Neither public server answered at all: no internet, or something on
    /// the way drops these echoes.
    NoInternetAnswers,
    /// Not enough answered to tell (no router found, a target not asked).
    Unclear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NetworkCheck {
    pub results: Vec<PingResult>,
    pub reading: ConnectionReading,
    pub unix_ms: u64,
}

/// What the check needs from the operating system.
pub trait Pinger: Send + Sync {
    /// The router traffic to `towards` goes through, or `None` when there is
    /// none (no network, or the address is on this network).
    fn router(&self, towards: Ipv4Addr) -> Result<Option<Ipv4Addr>>;
    /// One echo: its round trip in milliseconds, or `None` when no answer came
    /// (timed out, unreachable). `Err` only when the echo could not be sent.
    fn echo(&self, to: Ipv4Addr, timeout_ms: u32) -> Result<Option<u32>>;
}

/// Ask every target `echoes` times, side by side.
pub fn run(pinger: &dyn Pinger, echoes: u32, gap: Duration, unix_ms: u64) -> NetworkCheck {
    let first_public = PingTarget::Cloudflare.address().expect("a fixed address");
    let results: Vec<PingResult> = std::thread::scope(|scope| {
        let workers: Vec<_> = PingTarget::ALL
            .into_iter()
            .map(|target| {
                scope.spawn(move || {
                    let address = match target.address() {
                        Some(a) => Ok(Some(a)),
                        None => pinger.router(first_public),
                    };
                    match address {
                        Ok(Some(a)) => ask(pinger, target, a, echoes, gap),
                        Ok(None) => not_asked(target, "This PC has no router to the internet right now."),
                        Err(e) => not_asked(target, &format!("The router could not be found: {e}")),
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .zip(PingTarget::ALL)
            .map(|(w, target)| {
                w.join()
                    .unwrap_or_else(|_| not_asked(target, "The check stopped unexpectedly."))
            })
            .collect()
    });
    let reading = read(&results);
    NetworkCheck {
        results,
        reading,
        unix_ms,
    }
}

fn not_asked(target: PingTarget, why: &str) -> PingResult {
    PingResult {
        target,
        address: None,
        sent: 0,
        received: 0,
        min_ms: None,
        avg_ms: None,
        max_ms: None,
        jitter_ms: None,
        problem: Some(why.to_owned()),
    }
}

fn ask(pinger: &dyn Pinger, target: PingTarget, to: Ipv4Addr, echoes: u32, gap: Duration) -> PingResult {
    let mut rtts = Vec::with_capacity(echoes as usize);
    let mut problem = None;
    for i in 0..echoes {
        if i > 0 {
            std::thread::sleep(gap);
        }
        match pinger.echo(to, TIMEOUT_MS) {
            Ok(rtt) => rtts.push(rtt),
            Err(e) => {
                problem = Some(format!("Echoes could not be sent: {e}"));
                break;
            }
        }
    }
    let mut result = summarize(target, &rtts);
    result.address = Some(to.to_string());
    result.problem = problem;
    result
}

/// The figures for one target from its round trips in order, `None` for an
/// echo that got no answer.
pub fn summarize(target: PingTarget, rtts: &[Option<u32>]) -> PingResult {
    let answered: Vec<u32> = rtts.iter().flatten().copied().collect();
    let n = answered.len();
    let round = |x: f64| (x * 10.0).round() / 10.0;
    let avg = (n > 0).then(|| round(answered.iter().map(|&r| f64::from(r)).sum::<f64>() / n as f64));
    let jitter = (n > 1).then(|| {
        let total: f64 = answered
            .windows(2)
            .map(|w| (f64::from(w[1]) - f64::from(w[0])).abs())
            .sum();
        round(total / (n - 1) as f64)
    });
    PingResult {
        target,
        address: target.address().map(|a| a.to_string()),
        sent: rtts.len() as u32,
        received: n as u32,
        min_ms: answered.iter().min().copied(),
        avg_ms: avg,
        max_ms: answered.iter().max().copied(),
        jitter_ms: jitter,
        problem: None,
    }
}

/// Where the trouble seems to be.
pub fn read(results: &[PingResult]) -> ConnectionReading {
    let get = |t: PingTarget| results.iter().find(|r| r.target == t);
    let asked = |r: &&PingResult| r.problem.is_none() && r.sent > 0;
    let all = |r: &PingResult| r.received == r.sent;
    let (Some(router), publics) = (
        get(PingTarget::Router).filter(asked),
        [PingTarget::Cloudflare, PingTarget::Google]
            .into_iter()
            .filter_map(get)
            .filter(|r| asked(r))
            .collect::<Vec<_>>(),
    ) else {
        return ConnectionReading::Unclear;
    };
    if publics.len() < 2 {
        return ConnectionReading::Unclear;
    }
    if !all(router) {
        return ConnectionReading::LossToRouter;
    }
    if publics.iter().all(|r| r.received == 0) {
        return ConnectionReading::NoInternetAnswers;
    }
    if publics.iter().any(|r| !all(r)) {
        return ConnectionReading::LossPastRouter;
    }
    ConnectionReading::AllAnswered
}

/// The real thing on Windows; elsewhere, a refusal.
pub fn system() -> Box<dyn Pinger> {
    #[cfg(windows)]
    {
        Box::new(imp::WinPinger)
    }
    #[cfg(not(windows))]
    {
        Box::new(Unavailable)
    }
}

#[cfg(not(windows))]
struct Unavailable;

#[cfg(not(windows))]
impl Pinger for Unavailable {
    fn router(&self, _: Ipv4Addr) -> Result<Option<Ipv4Addr>> {
        Err(unavailable())
    }
    fn echo(&self, _: Ipv4Addr, _: u32) -> Result<Option<u32>> {
        Err(unavailable())
    }
}

#[cfg(not(windows))]
fn unavailable() -> EngineError {
    EngineError::Internal {
        detail: "the connection check needs Windows".into(),
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::net::Ipv4Addr;

    use windows::Win32::Foundation::{GetLastError, ERROR_NETWORK_UNREACHABLE, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetBestRoute, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY, MIB_IPFORWARDROW,
    };

    use super::{EngineError, Pinger, Result};

    /// What every echo carries: the letters Windows' `ping` sends.
    const PAYLOAD: &[u8; 32] = b"abcdefghijklmnopqrstuvwabcdefghi";
    /// `IP_STATUS` codes (`ipexport.h`) run from 11000 (`IP_STATUS_BASE`) to
    /// 11050 (`IP_GENERAL_FAILURE`); each means no answer came back.
    const IP_STATUS: std::ops::RangeInclusive<u32> = 11000..=11050;
    /// `ERROR_HOST_UNREACHABLE`: no route to the host, also no answer.
    const ERROR_HOST_UNREACHABLE: u32 = 1232;

    /// An address as Windows' IPv4 functions take it: the four bytes in
    /// network order, in memory as they are.
    fn raw(a: Ipv4Addr) -> u32 {
        u32::from_ne_bytes(a.octets())
    }

    pub struct WinPinger;

    impl Pinger for WinPinger {
        fn router(&self, towards: Ipv4Addr) -> Result<Option<Ipv4Addr>> {
            let mut row = MIB_IPFORWARDROW::default();
            let rc = unsafe { GetBestRoute(raw(towards), 0, &mut row) };
            if rc == ERROR_NETWORK_UNREACHABLE.0 || rc == ERROR_HOST_UNREACHABLE {
                return Ok(None);
            }
            if rc != NO_ERROR.0 {
                return Err(EngineError::Internal {
                    detail: format!("GetBestRoute failed with Windows error {rc}"),
                });
            }
            let hop = Ipv4Addr::from(row.dwForwardNextHop.to_ne_bytes());
            Ok((!hop.is_unspecified() && hop != towards).then_some(hop))
        }

        fn echo(&self, to: Ipv4Addr, timeout_ms: u32) -> Result<Option<u32>> {
            let handle = unsafe { IcmpCreateFile() }.map_err(|e| EngineError::Internal {
                detail: format!("IcmpCreateFile failed: {e}"),
            })?;
            // Room for one reply, its copy of the payload and an ICMP error
            // message (8 bytes), as the documentation asks.
            let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + PAYLOAD.len() + 8];
            let n = unsafe {
                IcmpSendEcho(
                    handle,
                    raw(to),
                    PAYLOAD.as_ptr().cast::<c_void>(),
                    PAYLOAD.len() as u16,
                    None,
                    reply.as_mut_ptr().cast::<c_void>(),
                    reply.len() as u32,
                    timeout_ms,
                )
            };
            let error = unsafe { GetLastError() }.0;
            let _ = unsafe { IcmpCloseHandle(handle) };
            if n == 0 {
                if IP_STATUS.contains(&error) || error == ERROR_NETWORK_UNREACHABLE.0 || error == ERROR_HOST_UNREACHABLE
                {
                    return Ok(None);
                }
                return Err(EngineError::Internal {
                    detail: format!("IcmpSendEcho failed with Windows error {error}"),
                });
            }
            // The buffer is bytes; read the reply without assuming alignment.
            let first = unsafe { std::ptr::read_unaligned(reply.as_ptr().cast::<ICMP_ECHO_REPLY>()) };
            // Status 0 is IP_SUCCESS; anything else (unreachable, TTL
            // expired on the way) is a message from elsewhere, not an answer.
            Ok((first.Status == 0 && first.Address == raw(to)).then_some(first.RoundTripTime))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::netcheck::{run, PingTarget, ECHOES, GAP_MS};

        /// This PC answers itself: proves the echo call and the reply layout.
        #[test]
        fn this_pc_answers_its_own_echoes() {
            let p = WinPinger;
            let answers: Vec<_> = (0..4).map(|_| p.echo(Ipv4Addr::LOCALHOST, 1000)).collect();
            println!("127.0.0.1: {answers:?}");
            assert!(answers.iter().all(|a| matches!(a, Ok(Some(_)))), "{answers:?}");
        }

        /// The whole check on this machine. A cloud runner may drop echoes to
        /// the internet, so only the shape is asserted; the figures are
        /// printed for the record.
        #[test]
        fn the_connection_check_runs_on_this_pc() {
            let p = WinPinger;
            println!("router: {:?}", p.router(PingTarget::Cloudflare.address().unwrap()));
            let check = run(&p, ECHOES, std::time::Duration::from_millis(GAP_MS), 0);
            for r in &check.results {
                println!("{r:?}");
                assert!(r.received <= r.sent);
            }
            println!("reading: {:?}", check.reading);
            assert_eq!(check.results.len(), PingTarget::ALL.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Answers from a script: per address, the round trips in order.
    struct Scripted {
        router: Result<Option<Ipv4Addr>>,
        answers: Mutex<HashMap<Ipv4Addr, Vec<Result<Option<u32>>>>>,
    }

    impl Scripted {
        fn new(router: Option<Ipv4Addr>) -> Self {
            Self {
                router: Ok(router),
                answers: Mutex::new(HashMap::new()),
            }
        }
        fn answers(self, to: Ipv4Addr, rtts: &[Option<u32>]) -> Self {
            self.answers
                .lock()
                .unwrap()
                .insert(to, rtts.iter().rev().map(|r| Ok(*r)).collect());
            self
        }
    }

    impl Pinger for Scripted {
        fn router(&self, _: Ipv4Addr) -> Result<Option<Ipv4Addr>> {
            self.router.clone()
        }
        fn echo(&self, to: Ipv4Addr, _: u32) -> Result<Option<u32>> {
            self.answers
                .lock()
                .unwrap()
                .get_mut(&to)
                .and_then(Vec::pop)
                .unwrap_or(Ok(None))
        }
    }

    const ROUTER: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
    const CF: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
    const G: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);

    fn check(p: &Scripted, echoes: u32) -> NetworkCheck {
        run(p, echoes, Duration::ZERO, 7)
    }

    #[test]
    fn figures_come_from_the_answered_echoes_only() {
        let r = summarize(PingTarget::Google, &[Some(10), None, Some(14), Some(12), None]);
        assert_eq!((r.sent, r.received), (5, 3));
        assert_eq!((r.min_ms, r.max_ms), (Some(10), Some(14)));
        assert_eq!(r.avg_ms, Some(12.0));
        // |14-10| and |12-14|, over two steps.
        assert_eq!(r.jitter_ms, Some(3.0));
        assert_eq!(r.address.as_deref(), Some("8.8.8.8"));
    }

    #[test]
    fn one_answer_has_no_jitter_and_none_has_no_figures() {
        let one = summarize(PingTarget::Router, &[None, Some(3)]);
        assert_eq!((one.avg_ms, one.jitter_ms), (Some(3.0), None));
        let none = summarize(PingTarget::Router, &[None, None]);
        assert_eq!(
            (none.min_ms, none.avg_ms, none.max_ms, none.jitter_ms),
            (None, None, None, None)
        );
    }

    #[test]
    fn every_target_is_asked_and_named_in_order() {
        let p = Scripted::new(Some(ROUTER))
            .answers(ROUTER, &[Some(1), Some(2), Some(1)])
            .answers(CF, &[Some(12), Some(13), Some(12)])
            .answers(G, &[Some(15), Some(15), Some(16)]);
        let c = check(&p, 3);
        let targets: Vec<_> = c.results.iter().map(|r| r.target).collect();
        assert_eq!(targets, PingTarget::ALL);
        assert_eq!(c.results[0].address.as_deref(), Some("192.168.1.1"));
        assert!(c
            .results
            .iter()
            .all(|r| r.sent == 3 && r.received == 3 && r.problem.is_none()));
        assert_eq!(c.reading, ConnectionReading::AllAnswered);
        assert_eq!(c.unix_ms, 7);
    }

    #[test]
    fn loss_at_the_router_points_at_the_home_network() {
        let p = Scripted::new(Some(ROUTER))
            .answers(ROUTER, &[Some(1), None, Some(2)])
            .answers(CF, &[Some(12), None, Some(13)])
            .answers(G, &[Some(15), Some(15), Some(16)]);
        assert_eq!(check(&p, 3).reading, ConnectionReading::LossToRouter);
    }

    #[test]
    fn loss_only_past_the_router_points_past_it() {
        let p = Scripted::new(Some(ROUTER))
            .answers(ROUTER, &[Some(1), Some(1), Some(2)])
            .answers(CF, &[Some(12), None, Some(13)])
            .answers(G, &[Some(15), Some(15), Some(16)]);
        assert_eq!(check(&p, 3).reading, ConnectionReading::LossPastRouter);
    }

    #[test]
    fn no_answer_from_either_server_reads_as_no_internet_answers() {
        let p = Scripted::new(Some(ROUTER)).answers(ROUTER, &[Some(1), Some(1)]);
        let c = check(&p, 2);
        assert_eq!(c.reading, ConnectionReading::NoInternetAnswers);
        assert_eq!(c.results[1].received, 0);
        assert_eq!(c.results[1].avg_ms, None);
    }

    #[test]
    fn no_router_is_said_plainly_and_the_servers_are_still_asked() {
        let p = Scripted::new(None).answers(CF, &[Some(12)]).answers(G, &[Some(15)]);
        let c = check(&p, 1);
        assert_eq!(c.results[0].sent, 0);
        assert_eq!(
            c.results[0].problem.as_deref(),
            Some("This PC has no router to the internet right now.")
        );
        assert_eq!(c.results[1].received, 1);
        assert_eq!(c.reading, ConnectionReading::Unclear);
    }

    #[test]
    fn an_echo_that_cannot_be_sent_stops_that_target_and_says_why() {
        let p = Scripted::new(Some(ROUTER))
            .answers(CF, &[Some(12)])
            .answers(G, &[Some(15)]);
        p.answers.lock().unwrap().insert(
            ROUTER,
            vec![
                Err(EngineError::Internal {
                    detail: "no handle".into(),
                }),
                Ok(Some(1)),
            ],
        );
        let c = check(&p, 3);
        let router = &c.results[0];
        assert_eq!((router.sent, router.received), (1, 1));
        assert!(router.problem.as_deref().unwrap().contains("no handle"));
        assert_eq!(c.reading, ConnectionReading::Unclear);
    }
}
