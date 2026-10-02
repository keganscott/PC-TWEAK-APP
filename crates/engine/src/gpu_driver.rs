//! Graphics drivers: which driver each adapter runs, and whether NVIDIA still
//! ships new game drivers for its generation. Plan section 6.2 item 8.
//!
//! DXGI (`hardware::GpuAdapter`) names the adapters but not their drivers, so
//! this reads `Win32_VideoController`. Everything below the query is pure and
//! tested here; the scanner turns the result into a finding.
//!
//! The generation table is matched on the product name Windows reports. It is
//! from memory (VERIFY against NVIDIA's product and driver-support pages;
//! NOTES.md N47), so a name it does not know is "not recognised", never a guess.

use serde::Serialize;
use ts_rs::TS;

use super::error::Result;
use super::probe::Probe;
use super::wmi::WmiRow;

pub(crate) const WQL_VIDEO: &str =
    "SELECT Name, AdapterCompatibility, DriverVersion, DriverDate, PNPDeviceID FROM Win32_VideoController";

pub const VENDOR_NVIDIA: u32 = 0x10DE;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GpuDriver {
    pub name: String,
    /// PCI vendor from the device id (`PCI\VEN_10DE&...`), when it has one.
    pub vendor_id: Option<u32>,
    /// Windows' version of the driver package, e.g. `32.0.15.8180`.
    pub driver_version: Option<String>,
    /// NVIDIA's own number for the same driver, e.g. `581.80`. NVIDIA only.
    pub nvidia_version: Option<String>,
    /// `YYYY-MM-DD`, the date on the driver package.
    pub driver_date: Option<String>,
}

/// Physical adapters and their drivers. Adapters that are not on the PCI bus
/// (Microsoft Basic Display, Hyper-V and remote-desktop display adapters) are
/// left out: they are not graphics cards.
pub fn drivers_from(rows: &Result<Vec<WmiRow>>) -> Probe<Vec<GpuDriver>> {
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Probe::unknown(format!("cannot read the graphics drivers: {e}")),
    };
    let drivers: Vec<GpuDriver> = rows
        .iter()
        .filter_map(|row| {
            let pnp = row.str("PNPDeviceID")?;
            if !pnp.to_ascii_uppercase().starts_with("PCI\\") {
                return None;
            }
            let vendor_id = pci_vendor(pnp);
            let driver_version = row.str("DriverVersion").map(str::trim).filter(|v| !v.is_empty());
            Some(GpuDriver {
                name: row.str("Name").unwrap_or("").trim().to_owned(),
                vendor_id,
                driver_version: driver_version.map(str::to_owned),
                nvidia_version: driver_version
                    .filter(|_| vendor_id == Some(VENDOR_NVIDIA))
                    .and_then(nvidia_version),
                driver_date: row.str("DriverDate").and_then(cim_date),
            })
        })
        .collect();
    if drivers.is_empty() {
        return Probe::no("no graphics card on the PCI bus reported a driver");
    }
    Probe::yes(drivers)
}

/// `PCI\VEN_10DE&DEV_1C03&...` -> `0x10DE`.
fn pci_vendor(pnp: &str) -> Option<u32> {
    let upper = pnp.to_ascii_uppercase();
    let at = upper.find("VEN_")? + 4;
    u32::from_str_radix(upper.get(at..at + 4)?, 16).ok()
}

/// NVIDIA's number from Windows' version: the last five digits of the last two
/// fields, so `32.0.15.8180` -> `581.80` and `31.0.15.3623` -> `536.23`. VERIFY
/// (NOTES.md N47): the convention is well known but from memory here.
pub fn nvidia_version(windows_version: &str) -> Option<String> {
    let parts: Vec<&str> = windows_version.split('.').collect();
    let [_, _, third, fourth] = parts.as_slice() else {
        return None;
    };
    if !third.bytes().chain(fourth.bytes()).all(|c| c.is_ascii_digit()) || fourth.len() > 4 {
        return None;
    }
    let digits = format!("{third}{fourth:0>4}");
    let tail = digits.get(digits.len().checked_sub(5)?..)?;
    Some(format!("{}.{}", &tail[..3], &tail[3..]))
}

/// `20250820000000.000000-000` -> `2025-08-20`.
fn cim_date(s: &str) -> Option<String> {
    let d = s.get(..8)?;
    if !d.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (m, day) = (&d[4..6], &d[6..8]);
    if !(1..=12).contains(&m.parse::<u32>().ok()?) || !(1..=31).contains(&day.parse::<u32>().ok()?) {
        return None;
    }
    Some(format!("{}-{m}-{day}", &d[..4]))
}

/// Which of NVIDIA's driver lines a card is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvidiaBranch {
    /// Pascal and older: no new Game Ready drivers, security updates only
    /// (plan 6.2 item 8). `generation` names it for the user.
    Legacy { generation: &'static str },
    /// Turing and newer (GTX 16, RTX, MX 450 and later): still current.
    Current,
    /// A name the table does not know. Never guessed.
    Unrecognised,
}

/// Classify by the product name, e.g. `NVIDIA GeForce GTX 1060 6GB`.
pub fn nvidia_branch(name: &str) -> NvidiaBranch {
    use NvidiaBranch::*;
    let n = name.to_ascii_uppercase();
    const KEPLER_OR_OLDER: &str = "Kepler or older";

    if n.contains("RTX") {
        return Current;
    }
    if n.contains("TITAN") {
        return if n.contains("TITAN XP") || n.contains("(PASCAL)") {
            Legacy { generation: "Pascal" }
        } else if n.contains("TITAN X") {
            Legacy { generation: "Maxwell" }
        } else if n.contains("TITAN BLACK") || n.contains("TITAN Z") || n.contains("GTX TITAN") {
            Legacy {
                generation: KEPLER_OR_OLDER,
            }
        } else {
            Unrecognised
        };
    }
    if let Some(num) = model_number(&n, "GTX ") {
        return match num {
            1600..=1699 => Current,
            1000..=1099 => Legacy { generation: "Pascal" },
            900..=999 | 745 | 750 => Legacy { generation: "Maxwell" },
            // 800M laptops mix Kepler and Maxwell; both are past support.
            800..=899 => Legacy {
                generation: "Maxwell or older",
            },
            400..=799 => Legacy {
                generation: KEPLER_OR_OLDER,
            },
            _ => Unrecognised,
        };
    }
    if let Some(num) = model_number(&n, "GT ") {
        return match num {
            1010 | 1030 => Legacy { generation: "Pascal" },
            200..=799 => Legacy {
                generation: KEPLER_OR_OLDER,
            },
            _ => Unrecognised,
        };
    }
    if let Some(num) = model_number(&n, "MX") {
        return match num {
            110 | 130 => Legacy { generation: "Maxwell" },
            150 | 230 | 250 | 330 | 350 => Legacy { generation: "Pascal" },
            450..=999 => Current,
            _ => Unrecognised,
        };
    }
    for (prefix, branch) in [
        ("QUADRO P", Legacy { generation: "Pascal" }),
        ("QUADRO M", Legacy { generation: "Maxwell" }),
        (
            "QUADRO K",
            Legacy {
                generation: KEPLER_OR_OLDER,
            },
        ),
        ("QUADRO T", Current),
    ] {
        if let Some(rest) = n.find(prefix).map(|i| &n[i + prefix.len()..]) {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                return branch;
            }
        }
    }
    Unrecognised
}

/// The number right after `prefix`, e.g. `GTX 1060 6GB` -> 1060, `MX250` -> 250.
fn model_number(name: &str, prefix: &str) -> Option<u32> {
    let rest = &name[name.find(prefix)? + prefix.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use crate::wmi::{WmiRow, WmiValue};

    fn row(name: &str, pnp: &str, version: &str, date: &str) -> WmiRow {
        WmiRow(
            [
                ("Name", name),
                ("PNPDeviceID", pnp),
                ("DriverVersion", version),
                ("DriverDate", date),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), WmiValue::Str(v.into())))
            .collect(),
        )
    }

    #[test]
    fn nvidia_numbers_come_from_the_last_two_fields() {
        assert_eq!(nvidia_version("32.0.15.8180").as_deref(), Some("581.80"));
        assert_eq!(nvidia_version("31.0.15.3623").as_deref(), Some("536.23"));
        assert_eq!(nvidia_version("30.0.14.7212").as_deref(), Some("472.12"));
        assert_eq!(nvidia_version("27.21.14.5671").as_deref(), Some("456.71"));
        assert_eq!(
            nvidia_version("32.0.15.180").as_deref(),
            Some("501.80"),
            "short last field is zero-padded"
        );
        assert_eq!(nvidia_version("1.2.3"), None);
        assert_eq!(nvidia_version("a.b.c.d"), None);
    }

    #[test]
    fn only_pci_adapters_count_and_vendor_and_date_are_parsed() {
        let p = drivers_from(&Ok(vec![
            row(
                "Microsoft Basic Display Adapter",
                "ROOT\\BASICDISPLAY\\0000",
                "10.0.26100.1",
                "",
            ),
            row("Microsoft Hyper-V Video", "VMBUS\\{abc}", "10.0.26100.1", ""),
            row(
                "NVIDIA GeForce GTX 1060 6GB",
                "PCI\\VEN_10DE&DEV_1C03&SUBSYS_00000000",
                "32.0.15.8180",
                "20250820000000.000000-000",
            ),
            row(
                "Intel(R) UHD Graphics 630",
                "PCI\\VEN_8086&DEV_3E92",
                "31.0.101.2125",
                "20230601000000.000000-000",
            ),
        ]));
        let d = p.value().expect("yes");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].vendor_id, Some(VENDOR_NVIDIA));
        assert_eq!(d[0].nvidia_version.as_deref(), Some("581.80"));
        assert_eq!(d[0].driver_date.as_deref(), Some("2025-08-20"));
        assert_eq!(d[1].vendor_id, Some(0x8086));
        assert_eq!(d[1].nvidia_version, None, "only NVIDIA gets an NVIDIA number");
    }

    #[test]
    fn no_physical_adapter_is_no_and_a_failed_query_is_unknown() {
        assert!(drivers_from(&Ok(vec![row("Microsoft Hyper-V Video", "VMBUS\\x", "1.0", "")])).is_no());
        assert!(drivers_from(&Ok(vec![])).is_no());
        assert!(drivers_from(&Err(EngineError::Internal { detail: "x".into() })).is_unknown());
    }

    #[test]
    fn bad_dates_are_dropped_not_guessed() {
        assert_eq!(cim_date("20251399000000.000000-000"), None);
        assert_eq!(cim_date("2025"), None);
        assert_eq!(cim_date("abcd0101000000.000000-000"), None);
        assert_eq!(cim_date("20240229000000.000000+060").as_deref(), Some("2024-02-29"));
    }

    #[test]
    fn generations_by_name() {
        use NvidiaBranch::*;
        let pascal = Legacy { generation: "Pascal" };
        let maxwell = Legacy { generation: "Maxwell" };
        for (name, want) in [
            ("NVIDIA GeForce GTX 1060 6GB", pascal),
            ("NVIDIA GeForce GTX 1080 Ti", pascal),
            ("NVIDIA GeForce GTX 1050 Ti with Max-Q Design", pascal),
            ("NVIDIA GeForce GT 1030", pascal),
            ("NVIDIA GeForce MX250", pascal),
            ("NVIDIA TITAN Xp", pascal),
            ("NVIDIA TITAN X (Pascal)", pascal),
            ("NVIDIA Quadro P2000", pascal),
            ("NVIDIA GeForce GTX 970", maxwell),
            ("NVIDIA GeForce GTX 750 Ti", maxwell),
            ("NVIDIA GeForce 940MX", Unrecognised),
            ("NVIDIA GeForce MX130", maxwell),
            (
                "NVIDIA GeForce GTX 760",
                Legacy {
                    generation: "Kepler or older",
                },
            ),
            (
                "NVIDIA GeForce GT 710",
                Legacy {
                    generation: "Kepler or older",
                },
            ),
            ("NVIDIA GeForce GTX 1660 SUPER", Current),
            ("NVIDIA GeForce GTX 1650", Current),
            ("NVIDIA GeForce RTX 3060", Current),
            ("NVIDIA GeForce RTX 5090", Current),
            ("NVIDIA RTX A2000", Current),
            ("NVIDIA TITAN RTX", Current),
            ("NVIDIA GeForce MX450", Current),
            ("NVIDIA T1000", Unrecognised),
            ("NVIDIA Quadro T2000", Current),
            ("NVIDIA TITAN V", Unrecognised),
            ("Example GPU", Unrecognised),
        ] {
            assert_eq!(nvidia_branch(name), want, "{name}");
        }
    }
}
