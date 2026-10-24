//! Acceptance helper: write a small cloud-optimised GeoTIFF for manual preview checks.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "tiny_cog.tif".into());
    std::fs::write(path, rustgis_data::raster::cog::example(1024, 256))?;
    Ok(())
}
