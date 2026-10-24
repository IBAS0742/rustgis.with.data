//! Acceptance helper: write a small georeferenced GeoTIFF for manual preview checks.

use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster, geotiff};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let width = 16u32;
    let height = 16u32;
    let mut band = Vec::with_capacity((height * width) as usize);
    for y in 0..height {
        for x in 0..width {
            band.push((x as f32 + y as f32) / 30.0);
        }
    }
    let raster = Raster::new(
        "tiny",
        width,
        height,
        vec![band],
        GeoTransform { origin_x: -1.0, origin_y: 1.0, pixel_width: 2.0 / f64::from(width), pixel_height: -2.0 / f64::from(height) },
        Crs::LonLat,
        None,
        ColorHint::Gray,
        "Float32",
    )?;
    let bytes = geotiff::write(&raster, 0)?;
    let path = std::env::args().nth(1).unwrap_or_else(|| "tiny.tif".into());
    std::fs::write(path, bytes)?;
    Ok(())
}
