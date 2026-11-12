use image::{ImageReader, RgbaImage};
use std::io::Cursor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub index: usize,
    pub pixels: Vec<(u32, u32)>,
    pub bounds: (u32, u32, u32, u32),
}

pub fn components(current: &RgbaImage, previous: Option<&RgbaImage>) -> Vec<Component> {
    let width = current.width();
    let height = current.height();
    let mut seen = vec![false; (width * height) as usize];
    let mut found = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let start = (x, y);
            if !fresh(current, previous, start) || seen[offset(width, x, y)] {
                continue;
            }
            let mut pixels = Vec::new();
            let mut stack = vec![start];
            seen[offset(width, x, y)] = true;
            while let Some((px, py)) = stack.pop() {
                pixels.push((px, py));
                for (nx, ny) in neighbors(px, py, width, height) {
                    let slot = offset(width, nx, ny);
                    if !seen[slot] && fresh(current, previous, (nx, ny)) {
                        seen[slot] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            if pixels.len() >= 8 {
                found.push(pixels);
            }
        }
    }
    found.sort_by_key(|pixels| {
        let min_x = pixels.iter().map(|p| p.0).min().unwrap_or(0);
        let min_y = pixels.iter().map(|p| p.1).min().unwrap_or(0);
        (min_x, min_y)
    });
    found
        .into_iter()
        .enumerate()
        .map(|(index, pixels)| {
            let min_x = pixels.iter().map(|p| p.0).min().unwrap_or(0);
            let min_y = pixels.iter().map(|p| p.1).min().unwrap_or(0);
            let max_x = pixels.iter().map(|p| p.0).max().unwrap_or(0);
            let max_y = pixels.iter().map(|p| p.1).max().unwrap_or(0);
            Component {
                index,
                pixels,
                bounds: (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1),
            }
        })
        .collect()
}

pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, String> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())
        .map(|image| image.to_rgba8())
}

pub fn opaque_pixels(image: &RgbaImage) -> Vec<(u32, u32)> {
    let mut pixels = Vec::new();
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] >= 16 {
            pixels.push((x, y));
        }
    }
    pixels
}

fn fresh(current: &RgbaImage, previous: Option<&RgbaImage>, (x, y): (u32, u32)) -> bool {
    if current.get_pixel(x, y)[3] < 16 {
        return false;
    }
    match previous {
        Some(previous) if x < previous.width() && y < previous.height() => {
            previous.get_pixel(x, y)[3] < 16
        }
        _ => true,
    }
}

fn neighbors(x: u32, y: u32, width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(4);
    if x > 0 {
        out.push((x - 1, y));
    }
    if y > 0 {
        out.push((x, y - 1));
    }
    if x + 1 < width {
        out.push((x + 1, y));
    }
    if y + 1 < height {
        out.push((x, y + 1));
    }
    out
}

fn offset(width: u32, x: u32, y: u32) -> usize {
    (y * width + x) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn block(image: &mut RgbaImage, x0: u32, y0: u32, w: u32, h: u32) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                image.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
    }

    #[test]
    fn numbers_separate_blobs_from_left_to_right() {
        let mut image = RgbaImage::new(40, 8);
        block(&mut image, 0, 0, 8, 1);
        block(&mut image, 20, 0, 8, 1);
        let found = components(&image, None);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].index, 0);
        assert!(found[0].pixels.iter().all(|p| p.0 < 10));
        assert!(found[1].pixels.iter().all(|p| p.0 >= 20));
    }

    #[test]
    fn diagonal_contact_does_not_join_components() {
        let mut image = RgbaImage::new(20, 2);
        block(&mut image, 0, 0, 8, 1);
        block(&mut image, 8, 1, 8, 1);
        let found = components(&image, None);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn drops_a_speck_and_pixels_already_committed() {
        let mut image = RgbaImage::new(20, 4);
        block(&mut image, 0, 0, 3, 2);
        block(&mut image, 10, 0, 8, 1);
        assert_eq!(components(&image, None).len(), 1);

        let mut previous = RgbaImage::new(20, 4);
        block(&mut previous, 10, 0, 8, 1);
        assert!(components(&image, Some(&previous)).is_empty());
    }

    #[test]
    fn transparent_image_has_no_components() {
        let image = RgbaImage::new(8, 8);
        assert!(components(&image, None).is_empty());
    }
}
