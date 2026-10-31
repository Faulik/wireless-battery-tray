use embed_manifest::manifest::DpiAwareness;
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Per-monitor v2 DPI, common controls v6 (tooltips) and a supportedOS list that
        // includes Windows 8+, which child layered windows require.
        embed_manifest(new_manifest("BatteriesBar").dpi_awareness(DpiAwareness::PerMonitorV2)).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
