//! The Sisyphus logo (a placeholder for now), as SVG and drawn to pixels
//! for the window and tray icons.

/// The logo as SVG. The website serves the same file.
pub const SVG: &[u8] = include_bytes!("../assets/logo.svg");

/// The logo drawn at `size` × `size` pixels, as straight (not
/// premultiplied) RGBA, row by row. `None` if drawing failed.
pub fn rgba(size: u32) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(SVG, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
    )
}

/// The window icon (taskbar, title bar on Windows and X11).
pub fn window_icon() -> Option<iced::window::Icon> {
    const SIZE: u32 = 64;
    iced::window::icon::from_rgba(rgba(SIZE)?, SIZE, SIZE).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_the_logo() {
        let pixels = super::rgba(32).expect("logo draws");
        assert_eq!(pixels.len(), 32 * 32 * 4);
        // The middle is opaque, the outermost corner transparent.
        let at = |x: usize, y: usize| &pixels[(y * 32 + x) * 4..][..4];
        assert_eq!(at(16, 16)[3], 255);
        assert_eq!(at(0, 0)[3], 0);
    }
}
