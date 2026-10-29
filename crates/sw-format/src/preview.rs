//! Embedded thumbnails: `PreviewPNG` (PNG) or `Preview` (raw DIB / BMP).

/// RGBA8 image.
#[derive(Debug, Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn decode_png(b: &[u8]) -> Option<Rgba> {
    let img = image::load_from_memory_with_format(b, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    Some(Rgba {
        width: img.width(),
        height: img.height(),
        pixels: img.into_raw(),
    })
}

/// `Preview` stream: optional u32 size prefix, then BITMAPINFOHEADER + palette + pixels.
pub fn decode_dib(b: &[u8]) -> Option<Rgba> {
    let start = if b
        .get(0..4)
        .map(|x| u32::from_le_bytes(x.try_into().unwrap()))
        == Some(40)
    {
        0
    } else if b
        .get(4..8)
        .map(|x| u32::from_le_bytes(x.try_into().unwrap()))
        == Some(40)
    {
        4
    } else {
        return None;
    };
    let dib = &b[start..];
    if dib.len() < 40 {
        return None;
    }
    let rd = |o: usize| u32::from_le_bytes(dib[o..o + 4].try_into().unwrap());
    let bpp = u16::from_le_bytes(dib[14..16].try_into().unwrap()) as u32;
    let clr_used = rd(32);
    let pal = if bpp <= 8 {
        if clr_used == 0 {
            1u32 << bpp
        } else {
            clr_used
        }
    } else {
        0
    };
    let off = 14 + 40 + pal * 4;
    let mut file = Vec::with_capacity(dib.len() + 14);
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    file.extend_from_slice(&[0, 0, 0, 0]);
    file.extend_from_slice(&off.to_le_bytes());
    file.extend_from_slice(dib);
    let img = image::load_from_memory_with_format(&file, image::ImageFormat::Bmp)
        .ok()?
        .to_rgba8();
    Some(Rgba {
        width: img.width(),
        height: img.height(),
        pixels: img.into_raw(),
    })
}
