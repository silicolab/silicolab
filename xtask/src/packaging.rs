use std::{fs, path::Path};

pub(super) fn render_svg(source: &Path, width: u32, output: &Path) -> Result<(), String> {
    let source = fs::read(source).map_err(|error| error.to_string())?;
    let image = render(&source, width)?;
    image.save_png(output).map_err(|error| error.to_string())
}

fn render(source: &[u8], width: u32) -> Result<resvg::tiny_skia::Pixmap, String> {
    if width == 0 || width > 4096 {
        return Err("SVG width must be between 1 and 4096 pixels".into());
    }
    let tree = resvg::usvg::Tree::from_data(source, &resvg::usvg::Options::default())
        .map_err(|error| error.to_string())?;
    let scale = width as f32 / tree.size().width();
    let height = (tree.size().height() * scale).round() as u32;
    let mut image = resvg::tiny_skia::Pixmap::new(width, height).ok_or("Invalid SVG dimensions")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut image.as_mut(),
    );
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_brand_master_and_retina_arrow_from_vectors() {
        for width in [640, 1280] {
            let image = render(include_bytes!("../../assets/dmg-background.svg"), width).unwrap();
            assert_eq!(image.height(), width * 360 / 640);
            assert_eq!(image.pixel(0, 0).unwrap().alpha(), 255);
            assert_ne!(
                image.pixel(width / 2, width / 4).unwrap(),
                image.pixel(0, 0).unwrap()
            );
        }
        let image = render(include_bytes!("../../assets/icon/master-dark.svg"), 1024).unwrap();
        assert_eq!((image.width(), image.height()), (1024, 1024));
        assert!(render(b"bad svg", 1024).is_err());
    }
}
