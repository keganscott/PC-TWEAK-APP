//! PeakTweaks field check: run once on a real Windows PC to collect the evidence
//! the development gates need, in one JSON report. See docs/FIELD_CHECK.md.
//!
//! By default it only reads. It changes something only when asked with a flag
//! and confirmed by typing "yes":
//!   --create-restore-point  the real restore-point flow (journalled, like the app)
//!   --revert-all            undo everything PeakTweaks has applied on this PC
//!
//! Everything it does is listed in the report, including what failed and why.

#[cfg(not(windows))]
fn main() {
    eprintln!("peaktweaks-field-check only runs on Windows.");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(windows_main::run());
}

#[cfg(windows)]
mod windows_main {
    use std::io::{BufRead, Write};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use serde_json::{json, Map, Value};

    use peaktweaks_engine::env::License;
    use peaktweaks_engine::identity::is_elevated;
    use peaktweaks_engine::journal::now_ms;
    use peaktweaks_engine::proc::run_limited;
    use peaktweaks_engine::proof::capture::{CaptureRequest, CaptureTool, PresentMonTool};
    use peaktweaks_engine::proof::metrics::{compute_stats, parse_frame_times};
    use peaktweaks_engine::proof::nvml::{NvmlSampler, ThrottleSampler, ThrottleTally};
    use peaktweaks_engine::proof::presentmon;
    use peaktweaks_engine::registry::windows::WinRegistry;
    use peaktweaks_engine::registry::{Hive, RegistryBackend};
    use peaktweaks_engine::restore::create_restore_point;
    use peaktweaks_engine::secure_dir::TrustedDir;
    use peaktweaks_engine::wmi::{WmiSource, WmiValue, WmiWorker, NS_CIMV2, NS_STORAGE};
    use peaktweaks_engine::Engine;

    const USAGE: &str = "\
usage: peaktweaks-field-check [options]

Read-only unless a flag below says otherwise. Run from an elevated PowerShell.

  --out DIR                where to write the report (default: a new folder here)
  --capture EXE            also record frames from EXE with PresentMon, e.g. --capture msedge.exe
                           (start the game or a video first and keep it in the foreground)
  --seconds N              capture length, 10..600 (default 20)
  --nvml-seconds N         how long to sample NVIDIA GPU throttle reasons when not capturing (default 5)
  --create-restore-point   CHANGES THINGS: create a real restore point the way the app does
  --revert-all             CHANGES THINGS: undo everything PeakTweaks has applied
  --yes                    do not ask before the two flags above
  --help";

    #[derive(Default)]
    struct Args {
        out: Option<PathBuf>,
        capture: Option<String>,
        seconds: u32,
        nvml_seconds: u32,
        create_restore_point: bool,
        revert_all: bool,
        yes: bool,
    }

    fn parse_args() -> Result<Args, String> {
        let mut a = Args {
            seconds: 20,
            nvml_seconds: 5,
            ..Args::default()
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--out" => a.out = Some(PathBuf::from(value("--out")?)),
                "--capture" => a.capture = Some(value("--capture")?),
                "--seconds" => a.seconds = value("--seconds")?.parse().map_err(|_| "--seconds needs a number")?,
                "--nvml-seconds" => {
                    a.nvml_seconds = value("--nvml-seconds")?
                        .parse()
                        .map_err(|_| "--nvml-seconds needs a number")?
                }
                "--create-restore-point" => a.create_restore_point = true,
                "--revert-all" => a.revert_all = true,
                "--yes" => a.yes = true,
                "--help" | "-h" => return Err(String::new()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(a)
    }

    fn confirm(what: &str) -> bool {
        print!("{what}\nType yes to continue: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("yes")
    }

    /// Runs one section, timing it and turning a panic or error into data.
    fn section(report: &mut Map<String, Value>, name: &str, f: impl FnOnce() -> Result<Value, String>) {
        println!("- {name} ...");
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .unwrap_or_else(|_| Err("this section panicked; see the console output".into()));
        let ms = started.elapsed().as_millis() as u64;
        let entry = match outcome {
            Ok(v) => json!({ "ok": true, "ms": ms, "result": v }),
            Err(e) => {
                println!("  failed: {e}");
                json!({ "ok": false, "ms": ms, "error": e })
            }
        };
        report.insert(name.to_owned(), entry);
    }

    fn wmi_json(v: &WmiValue) -> Value {
        match v {
            WmiValue::Null => Value::Null,
            WmiValue::Str(s) => json!(s),
            WmiValue::Int(n) => json!(n),
            WmiValue::UInt(n) => json!(n),
            WmiValue::Float(n) => json!(n),
            WmiValue::Bool(b) => json!(b),
            WmiValue::Array(items) => Value::Array(items.iter().map(wmi_json).collect()),
        }
    }

    fn wmi_rows(wmi: &dyn WmiSource, ns: &str, wql: &str) -> Result<Value, String> {
        let rows = wmi.query(ns, wql).map_err(|e| e.to_string())?;
        Ok(Value::Array(
            rows.iter()
                .map(|r| {
                    let mut m: Vec<(&String, &WmiValue)> = r.0.iter().collect();
                    m.sort_by(|a, b| a.0.cmp(b.0));
                    Value::Object(m.into_iter().map(|(k, v)| (k.clone(), wmi_json(v))).collect())
                })
                .collect(),
        ))
    }

    fn system32(exe: &str) -> Result<PathBuf, String> {
        let dir =
            peaktweaks_engine::sysdirs::system32().map_err(|e| format!("could not find the System32 folder: {e}"))?;
        Ok(dir.join(exe))
    }

    fn run_tool(exe: &str, args: &[&str]) -> Result<Value, String> {
        let mut cmd = Command::new(system32(exe)?);
        cmd.args(args);
        let out =
            run_limited(cmd, exe, Duration::from_secs(60), Duration::from_millis(50)).map_err(|e| e.to_string())?;
        Ok(json!({ "exitCode": out.exit_code, "stdout": out.stdout, "stderr": out.stderr }))
    }

    fn reg_values(key: &str, names: &[&str]) -> Value {
        let reg = WinRegistry::new();
        let mut m = Map::new();
        for n in names {
            let v = match reg.read_value(Hive::LocalMachine, key, n) {
                Ok(Some(raw)) => json!({ "type": raw.vtype, "dword": raw.as_dword(), "string": raw.as_sz() }),
                Ok(None) => json!("absent"),
                Err(e) => json!({ "error": e.to_string() }),
            };
            m.insert((*n).to_owned(), v);
        }
        Value::Object(m)
    }

    fn sample_gpu(stop: &AtomicBool, max: Duration) -> Value {
        let sampler = NvmlSampler::new();
        let mut tally = ThrottleTally::default();
        let first = sampler.sample();
        let first_json = serde_json::to_value(&first).unwrap_or(Value::Null);
        tally.add(first);
        let started = Instant::now();
        while !stop.load(Ordering::Relaxed) && started.elapsed() < max {
            std::thread::sleep(Duration::from_millis(500));
            tally.add(sampler.sample());
        }
        json!({ "firstSample": first_json, "summary": tally.finish() })
    }

    fn write_fixture(csv: &str, path: &Path) -> std::io::Result<usize> {
        let mut lines = csv.lines();
        let header = lines.next().unwrap_or_default();
        let rows: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).take(600).collect();
        let mut text = String::from(header);
        text.push('\n');
        for r in &rows {
            text.push_str(r);
            text.push('\n');
        }
        std::fs::write(path, text)?;
        Ok(rows.len())
    }

    pub fn run() -> i32 {
        let args = match parse_args() {
            Ok(a) => a,
            Err(e) => {
                if !e.is_empty() {
                    eprintln!("{e}\n");
                }
                eprintln!("{USAGE}");
                return 2;
            }
        };
        if !is_elevated() {
            eprintln!("Run this from an elevated (Administrator) PowerShell: the probes and PresentMon need it.");
            return 2;
        }
        if args.create_restore_point
            && !args.yes
            && !confirm("This will turn on System Protection if it is off, allow an on-demand restore point, and create one (it can take a minute).")
        {
            eprintln!("Cancelled.");
            return 1;
        }
        if args.revert_all && !args.yes && !confirm("This will undo every change PeakTweaks has applied on this PC.") {
            eprintln!("Cancelled.");
            return 1;
        }

        let out_dir = args
            .out
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("peaktweaks-field-check-{}", now_ms() / 1000)));
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            eprintln!("cannot create {}: {e}", out_dir.display());
            return 1;
        }
        println!(
            "PeakTweaks field check {} -> {}",
            env!("CARGO_PKG_VERSION"),
            out_dir.display()
        );

        let mut report = Map::new();
        report.insert(
            "tool".into(),
            json!({
                "name": "peaktweaks-field-check",
                "version": env!("CARGO_PKG_VERSION"),
                "startedUnixMs": now_ms(),
                "elevated": true,
                "flags": {
                    "capture": args.capture,
                    "seconds": args.seconds,
                    "createRestorePoint": args.create_restore_point,
                    "revertAll": args.revert_all,
                },
            }),
        );

        // The engine, started exactly as the app starts it (no dev stubs).
        let engine = match Engine::start_windows(peaktweaks_engine::tweaks::catalogue(), License::free()) {
            Ok(e) => Some(Arc::new(Mutex::new(e))),
            Err(e) => {
                println!("  the PeakTweaks engine did not start: {e}");
                if matches!(e, peaktweaks_engine::error::EngineError::AlreadyRunning) {
                    println!("  close the PeakTweaks app, then run this again");
                }
                report.insert("engineStart".into(), json!({ "ok": false, "error": e.to_string() }));
                None
            }
        };

        if let Some(engine) = &engine {
            let e = engine.clone();
            section(&mut report, "audit", move || {
                let mut g = e.lock().map_err(|_| "engine lock poisoned".to_string())?;
                serde_json::to_value(g.audit()).map_err(|e| e.to_string())
            });
        }

        // N43: the power-plan GUIDs we hard-code, against Windows' own names.
        section(&mut report, "powercfgList", || run_tool("powercfg.exe", &["/list"]));
        section(&mut report, "powercfgActive", || {
            run_tool("powercfg.exe", &["/getactivescheme"])
        });

        // N22: the raw rows behind the memory and disk readings.
        let wmi = WmiWorker::start();
        section(&mut report, "wmiPhysicalMemory", || {
            wmi_rows(&wmi, NS_CIMV2, "SELECT * FROM Win32_PhysicalMemory")
        });
        section(&mut report, "wmiPhysicalDisks", || {
            wmi_rows(
                &wmi,
                NS_STORAGE,
                "SELECT DeviceId, FriendlyName, MediaType, BusType FROM MSFT_PhysicalDisk",
            )
        });
        section(&mut report, "wmiVideoControllers", || {
            wmi_rows(
                &wmi,
                NS_CIMV2,
                "SELECT Name, AdapterCompatibility, DriverVersion, DriverDate, AdapterRAM, PNPDeviceID FROM Win32_VideoController",
            )
        });

        // N23: the values a System Protection on/off reading could be built from.
        section(&mut report, "systemRestoreRegistry", || {
            Ok(reg_values(
                r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore",
                &[
                    "RPSessionInterval",
                    "DisableSR",
                    "DisableConfig",
                    "SystemRestorePointCreationFrequency",
                ],
            ))
        });
        section(&mut report, "systemRestorePolicy", || {
            Ok(reg_values(
                r"SOFTWARE\Policies\Microsoft\Windows NT\SystemRestore",
                &["DisableSR", "DisableConfig"],
            ))
        });
        section(&mut report, "shadowStorage", || {
            run_tool("vssadmin.exe", &["list", "shadowstorage"])
        });

        // N39 + N33: a real PresentMon capture with GPU sampling alongside, or
        // just the GPU sampling.
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(exe) = args.capture.clone() {
            let seconds = args.seconds;
            let out = out_dir.clone();
            let stop2 = stop.clone();
            let gpu = std::thread::spawn(move || sample_gpu(&stop2, Duration::from_secs(u64::from(seconds) + 30)));
            section(&mut report, "presentmonCapture", move || {
                let dir = TrustedDir::ensure_program_data().map_err(|e| e.to_string())?;
                let pm = presentmon::provision(&dir).map_err(|e| e.to_string())?;
                let tool = PresentMonTool::new(pm.clone());
                let csv_path = out.join("capture.csv");
                let req = CaptureRequest {
                    exe: exe.clone(),
                    delay_seconds: 2,
                    seconds,
                    out_csv: csv_path.clone(),
                };
                println!("  recording {exe} for {seconds} s; keep it in the foreground");
                tool.capture(&req).map_err(|e| e.to_string())?;
                let text = std::fs::read_to_string(&csv_path).map_err(|e| e.to_string())?;
                let header = text.lines().next().unwrap_or_default().to_owned();
                let frames = parse_frame_times(&text).map_err(|e| e.to_string())?;
                let stats = compute_stats(&frames.ms).map_err(|e| e.to_string())?;
                let fixture = out.join("presentmon-real.csv");
                let fixture_rows = write_fixture(&text, &fixture).map_err(|e| e.to_string())?;
                Ok(json!({
                    "presentmon": pm.display().to_string(),
                    "version": tool.version(),
                    "header": header,
                    "framesUsed": frames.ms.len(),
                    "ignoredRows": frames.ignored_rows,
                    "unusableRows": frames.unusable_rows,
                    "stats": stats,
                    "fixture": fixture.display().to_string(),
                    "fixtureRows": fixture_rows,
                }))
            });
            stop.store(true, Ordering::Relaxed);
            let gpu = gpu.join().unwrap_or(json!({ "error": "sampler thread panicked" }));
            report.insert(
                "gpuThrottle".into(),
                json!({ "ok": true, "duringCapture": true, "result": gpu }),
            );
        } else {
            let secs = args.nvml_seconds;
            section(&mut report, "gpuThrottle", || {
                Ok(sample_gpu(
                    &AtomicBool::new(false),
                    Duration::from_secs(u64::from(secs)),
                ))
            });
        }

        // N24: the real restore-point flow, only when asked.
        if let (true, Some(engine)) = (args.create_restore_point, &engine) {
            let e = engine.clone();
            section(&mut report, "createRestorePoint", move || {
                let svc = e
                    .lock()
                    .map_err(|_| "engine lock poisoned".to_string())?
                    .restore_service()
                    .ok_or("this build has no restore service")?;
                let progress = |stage: &str, msg: &str| println!("  [{stage}] {msg}");
                let outcome = create_restore_point(&e, &svc, &progress).map_err(|e| e.to_string())?;
                let after = svc.status();
                Ok(json!({ "outcome": outcome, "statusAfter": after }))
            });
        }

        if let (true, Some(engine)) = (args.revert_all, &engine) {
            let e = engine.clone();
            section(&mut report, "revertAll", move || {
                let mut g = e.lock().map_err(|_| "engine lock poisoned".to_string())?;
                serde_json::to_value(g.revert_all()).map_err(|e| e.to_string())
            });
        }

        if let Some(engine) = &engine {
            let e = engine.clone();
            section(&mut report, "journal", move || {
                let g = e.lock().map_err(|_| "engine lock poisoned".to_string())?;
                serde_json::to_value(g.journal_view()).map_err(|e| e.to_string())
            });
        }

        report.insert("finishedUnixMs".into(), json!(now_ms()));
        let path = out_dir.join("report.json");
        let text = serde_json::to_string_pretty(&Value::Object(report.clone())).unwrap_or_default();
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("cannot write {}: {e}", path.display());
            return 1;
        }
        let failed: Vec<&String> = report
            .iter()
            .filter(|(_, v)| v.get("ok") == Some(&Value::Bool(false)))
            .map(|(k, _)| k)
            .collect();
        println!("\nWrote {}", path.display());
        if failed.is_empty() {
            println!("Every section ran.");
        } else {
            println!("Sections that failed (details in the report): {failed:?}");
        }
        0
    }
}
