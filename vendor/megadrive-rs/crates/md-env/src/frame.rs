//! The picture as an agent sees it: reduced, in grey levels.

/// Side of the observation picture (the usual 84 × 84 of Atari-style agents).
pub const GRAY_SIDE: usize = 84;
/// Bytes of the observation picture.
pub const GRAY_SIZE: usize = GRAY_SIDE * GRAY_SIDE;

/// Luma of an RGBA8888 pixel (0xRRGGBBAA): ITU-R BT.601 weights, in
/// integers (they add up to 256, so white gives 255).
fn luma(pixel: u32) -> u32 {
    let [r, g, b, _] = pixel.to_be_bytes();
    (77 * u32::from(r) + 150 * u32::from(g) + 29 * u32::from(b)) >> 8
}

/// Writes into `out` (`out_w` × `out_h` bytes) the `width` × `height`
/// picture at the start of `framebuffer` (rows `stride` pixels apart),
/// reduced to grey levels: each output pixel is the mean luma of the block of
/// source pixels it covers. No allocation.
pub fn gray_downscale(
    framebuffer: &[u32],
    stride: usize,
    width: usize,
    height: usize,
    out: &mut [u8],
    out_w: usize,
    out_h: usize,
) {
    assert!(width >= 1 && height >= 1 && width <= stride, "picture size");
    assert!(
        framebuffer.len() >= stride * height,
        "framebuffer too short"
    );
    assert!(out.len() >= out_w * out_h, "output too short");
    for oy in 0..out_h {
        let y0 = oy * height / out_h;
        let y1 = ((oy + 1) * height / out_h).max(y0 + 1);
        for ox in 0..out_w {
            let x0 = ox * width / out_w;
            let x1 = ((ox + 1) * width / out_w).max(x0 + 1);
            let mut sum = 0u32;
            for y in y0..y1 {
                let row = &framebuffer[y * stride + x0..y * stride + x1];
                sum += row.iter().map(|&p| luma(p)).sum::<u32>();
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            out[oy * out_w + ox] = (sum / count) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(r: u8, g: u8, b: u8) -> u32 {
        u32::from_be_bytes([r, g, b, 0xFF])
    }

    #[test]
    fn white_and_black_map_to_the_ends() {
        assert_eq!(luma(rgba(255, 255, 255)), 255);
        assert_eq!(luma(rgba(0, 0, 0)), 0);
        // Green weighs most, blue least.
        assert!(luma(rgba(0, 255, 0)) > luma(rgba(255, 0, 0)));
        assert!(luma(rgba(255, 0, 0)) > luma(rgba(0, 0, 255)));
    }

    #[test]
    fn a_plain_picture_stays_plain_at_any_size() {
        let (w, h, stride) = (320, 224, 320);
        let grey = rgba(100, 100, 100);
        let frame = vec![grey; stride * h];
        let mut out = vec![0u8; GRAY_SIZE];
        gray_downscale(&frame, stride, w, h, &mut out, GRAY_SIDE, GRAY_SIDE);
        assert!(
            out.iter().all(|&v| v == luma(grey) as u8),
            "{:?}",
            &out[..4]
        );
    }

    #[test]
    fn four_pixels_average_into_one() {
        // Stride wider than the picture: the pixels past `width` are ignored.
        let frame = [
            rgba(0, 0, 0),
            rgba(255, 255, 255),
            0xDEAD_BEEF,
            rgba(255, 255, 255),
            rgba(0, 0, 0),
            0xDEAD_BEEF,
        ];
        let mut out = [0u8; 1];
        gray_downscale(&frame, 3, 2, 2, &mut out, 1, 1);
        assert_eq!(out[0], 127, "mean of 0, 255, 255, 0");
    }

    #[test]
    fn left_and_right_halves_stay_apart() {
        let (w, h) = (8, 4);
        let frame: Vec<u32> = (0..w * h)
            .map(|i| {
                if i % w < w / 2 {
                    rgba(0, 0, 0)
                } else {
                    rgba(255, 255, 255)
                }
            })
            .collect();
        let mut out = [0u8; 4 * 2];
        gray_downscale(&frame, w, w, h, &mut out, 4, 2);
        assert_eq!(out, [0, 0, 255, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn a_target_larger_than_the_source_repeats_pixels() {
        let frame = [rgba(255, 255, 255), rgba(0, 0, 0)];
        let mut out = [9u8; 4];
        gray_downscale(&frame, 2, 2, 1, &mut out, 4, 1);
        assert_eq!(out, [255, 255, 0, 0]);
    }
}
