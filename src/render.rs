use crate::plan::{parse_color, RenderPlan};
use crate::strokes::Component;
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerPng {
    pub name: String,
    pub png: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenderError {
    MissingComponent(usize),
    BadColor(String),
    MissingLayer(usize),
    Encode(String),
}

pub fn paint(
    width: u32,
    height: u32,
    plan: &RenderPlan,
    components: &[Component],
    previous: &[LayerPng],
    replace_at: Option<usize>,
) -> Result<Vec<LayerPng>, RenderError> {
    let painted = paint_plan(width, height, plan, components)?;
    let Some(index) = replace_at else {
        return Ok(painted);
    };
    if index >= previous.len() {
        return Err(RenderError::MissingLayer(index));
    }
    if painted.len() != 1 {
        return Err(RenderError::MissingLayer(index));
    }
    let mut layers = previous.to_vec();
    layers[index] = painted.into_iter().next().expect("one painted layer");
    Ok(layers)
}

fn paint_plan(
    width: u32,
    height: u32,
    plan: &RenderPlan,
    components: &[Component],
) -> Result<Vec<LayerPng>, RenderError> {
    let mut layers = Vec::new();
    for planned in &plan.layers {
        let component = components
            .iter()
            .find(|component| component.index == planned.component)
            .ok_or(RenderError::MissingComponent(planned.component))?;
        let rgb = parse_color(&planned.color)
            .ok_or_else(|| RenderError::BadColor(planned.color.clone()))?;
        let mut image = RgbaImage::new(width, height);
        for (x, y) in &component.pixels {
            if *x < width && *y < height {
                image.put_pixel(*x, *y, Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
        layers.push(LayerPng {
            name: format!("an/{}", planned.name),
            png: encode(&image)?,
        });
    }
    Ok(layers)
}

pub fn encode(image: &RgbaImage) -> Result<Vec<u8>, RenderError> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| RenderError::Encode(error.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::pair;
    use crate::strokes::components;
    use image::Rgba;

    #[test]
    fn fills_each_component_and_leaves_the_rest_transparent() {
        let mut strokes = RgbaImage::new(16, 8);
        for x in 0..8 {
            strokes.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
        }
        let found = components(&strokes, None);
        let plan = pair("红色轮子", 1).unwrap();
        let layers = paint(16, 8, &plan, &found, &[], None).unwrap();
        assert_eq!(layers[0].name, "an/轮子");
        let decoded = crate::strokes::decode_png(&layers[0].png).unwrap();
        assert_eq!(decoded.get_pixel(0, 0).0, [0xc0, 0x39, 0x2b, 255]);
        assert_eq!(decoded.get_pixel(8, 0).0, [0, 0, 0, 0]);
    }

    #[test]
    fn a_local_repaint_keeps_the_other_layer_bytes() {
        let mut strokes = RgbaImage::new(24, 8);
        for x in 0..8 {
            strokes.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
            strokes.put_pixel(x + 12, 0, Rgba([0, 0, 0, 255]));
        }
        let found = components(&strokes, None);
        let first = paint(
            24,
            8,
            &pair("红色轮子，黄铜握把", 2).unwrap(),
            &found,
            &[],
            None,
        )
        .unwrap();
        let right = first[1].png.clone();
        let mask =
            crate::strokes::opaque_pixels(&crate::strokes::decode_png(&first[0].png).unwrap());
        let component = Component {
            index: 0,
            pixels: mask,
            bounds: (0, 0, 8, 1),
        };
        let next = paint(
            24,
            8,
            &pair("蓝色轮子", 1).unwrap(),
            &[component],
            &first,
            Some(0),
        )
        .unwrap();
        assert_eq!(next[1].png, right);
        assert_ne!(next[0].png, first[0].png);
    }

    #[test]
    fn a_missing_component_does_not_return_layers() {
        let plan = pair("红色轮子", 1).unwrap();
        assert!(paint(4, 4, &plan, &[], &[], None).is_err());
    }
}
