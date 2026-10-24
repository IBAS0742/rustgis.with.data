//! RustGIS Preview server.
//!
//! Configuration is environment-only; see `rustgis_preview::server::run`.

fn main() {
    if let Err(e) = rustgis_preview::server::run() {
        eprintln!("rustgis-preview-server: {e}");
        std::process::exit(1);
    }
}
