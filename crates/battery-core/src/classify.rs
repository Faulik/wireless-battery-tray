//! Device form factor: what each detection layer says, and a small model table for when the
//! Bluetooth/Windows layers only know "audio headset" (they can't tell earbuds from over-ears).

use serde::{Deserialize, Serialize};

use crate::Kind;

/// Which layer decided a device's kind, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KindSource {
    /// BLE GAP Appearance (e.g. 0x0941 earbud, 0x03C1 keyboard).
    BleAppearance,
    /// Bluetooth classic Class of Device (audio/video and peripheral minor classes).
    ClassOfDevice,
    /// Windows device-container category (`Communication.Headset.Bluetooth`, `Input.Keyboard`...).
    WindowsCategory,
    /// Built-in table of model names.
    ModelTable,
    /// The vendor protocol says what it is (HID++ device type, a headset-only dongle...).
    Provider,
}

impl KindSource {
    pub fn describe(self) -> &'static str {
        match self {
            KindSource::BleAppearance => "BLE appearance",
            KindSource::ClassOfDevice => "Bluetooth class of device",
            KindSource::WindowsCategory => "Windows device category",
            KindSource::ModelTable => "model table",
            KindSource::Provider => "device protocol",
        }
    }
}

/// Apply the model table when the earlier layers gave only a generic answer.
/// Generic: any audio kind from Class of Device / Windows category (neither can say "in-ear"),
/// or `Other` from anywhere. BLE appearance and vendor protocols are trusted as they are.
pub fn refine_kind(name: &str, kind: Kind, source: KindSource) -> (Kind, KindSource) {
    let audio = matches!(kind, Kind::Headset | Kind::Headphones | Kind::Earbuds | Kind::Neckband);
    let generic = kind == Kind::Other || (audio && matches!(source, KindSource::ClassOfDevice | KindSource::WindowsCategory | KindSource::Provider));
    if !generic {
        return (kind, source);
    }
    match model_kind(name) {
        Some(k) if kind == Kind::Other || audio_family(k) == audio => (k, KindSource::ModelTable),
        _ => (kind, source),
    }
}

fn audio_family(k: Kind) -> bool {
    matches!(k, Kind::Headset | Kind::Headphones | Kind::Earbuds | Kind::Neckband)
}

/// Name patterns, checked in order (more specific first: "AirPods Max" before "pods").
fn model_kind(name: &str) -> Option<Kind> {
    const TABLE: &[(&[&str], Kind)] = &[
        (&["airpods max", "wh-", "quietcomfort", "momentum", "headphone"], Kind::Headphones),
        (&["neckband", "wi-", "tone free"], Kind::Neckband),
        (&["buds", "pods", "earbud", "tws", "wf-", "elite", "soundpeats", "earfun", "in-ear"], Kind::Earbuds),
        (&["arctis", "nova ", "headset", "cloud", "kraken", "blackshark", "astro a", "g pro x", "virtuoso", "barracuda"], Kind::Headset),
    ];
    let n = format!("{} ", name.to_ascii_lowercase());
    TABLE.iter().find(|(pats, _)| pats.iter().any(|p| n.contains(p))).map(|(_, k)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers() {
        // H3: Bluetooth says "wearable headset", the model table knows they're earbuds.
        assert_eq!(refine_kind("SOUNDPEATS H3", Kind::Headset, KindSource::ClassOfDevice), (Kind::Earbuds, KindSource::ModelTable));
        assert_eq!(refine_kind("SteelSeries Arctis Nova 7P", Kind::Headset, KindSource::Provider), (Kind::Headset, KindSource::ModelTable));
        // A BLE appearance is specific enough already.
        assert_eq!(refine_kind("Galaxy Buds", Kind::Earbuds, KindSource::BleAppearance), (Kind::Earbuds, KindSource::BleAppearance));
        assert_eq!(refine_kind("AirPods Max", Kind::Headset, KindSource::ClassOfDevice), (Kind::Headphones, KindSource::ModelTable));
        // Never turns a keyboard into audio.
        assert_eq!(refine_kind("Cloud keyboard", Kind::Keyboard, KindSource::Provider), (Kind::Keyboard, KindSource::Provider));
        assert_eq!(refine_kind("Unknown thing", Kind::Other, KindSource::Provider), (Kind::Other, KindSource::Provider));
    }
}
