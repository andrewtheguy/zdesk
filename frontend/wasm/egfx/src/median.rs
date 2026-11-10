//! wlshare's lossless tiles where the desktop moves, by a median predictor
//! (`wlshare_proto::tiles`' `MEDIAN`): a rectangle's pixels as three planes of a
//! byte a pixel, green, red less green and blue less green, each carried as its
//! residuals from JPEG-LS's predictor and framed in zstd alone, the three frames
//! one after another. Here are the body's frames ([`frames`]), a plane's samples
//! out of its residuals ([`samples`]) and the planes' pixels ([`rgba`]); the zstd
//! between them is the caller's, as its threads are.
//!
//! The gateway's end-to-end test reads wlshare's tiles with this file too, so
//! nothing here is the module's alone.

/// What a zstd frame begins with.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Whether `body` is the three zstd frames of a `w`×`h` rectangle's planes and
/// nothing else: three frames one after another, each beginning a frame of its
/// own, since the decoders skip skippable ones, and each saying it holds exactly
/// a plane's `w * h` bytes, so the body decodes to three planes or fails to.
pub fn check(body: &[u8], w: u16, h: u16) -> Result<(), String> {
    let plane = u64::from(w) * u64::from(h);
    let mut rest = body;
    for _ in 0..3 {
        if !rest.starts_with(&ZSTD_MAGIC) {
            return Err("a plane that begins no zstd frame".into());
        }
        let len =
            ruzstd_decompress::decoding::frame_compressed_size(rest).map_err(|e| e.to_string())?;
        let (frame, after) = rest.split_at(len);
        let bound =
            ruzstd_decompress::decoding::decompressed_bound(frame).map_err(|e| e.to_string())?;
        if bound != plane {
            return Err(format!(
                "a zstd frame of {bound} bytes, where a plane of it is {plane}"
            ));
        }
        rest = after;
    }
    if !rest.is_empty() {
        return Err(format!("{} bytes past its three zstd frames", rest.len()));
    }
    Ok(())
}

/// How many bytes a `w`×`h` rectangle's three planes are.
pub fn planes_len(w: u16, h: u16) -> usize {
    3 * usize::from(w) * usize::from(h)
}

/// A plane's samples out of its residuals, in place, its rows `w` long: each
/// sample its residual and its prediction modulo 256. The prediction is nothing
/// for the first sample, the sample to the left along the top row, the one above
/// down the left column, and elsewhere the median of the sample to the left, the
/// one above and left + above − above-left, which is that sum held between the
/// two.
pub fn samples(plane: &mut [u8], w: usize) {
    if w == 0 || plane.is_empty() {
        return;
    }
    let (top, below) = plane.split_at_mut(w);
    let mut left = 0u8;
    for sample in top.iter_mut() {
        left = left.wrapping_add(*sample);
        *sample = left;
    }
    let mut above: &[u8] = top;
    for row in below.chunks_exact_mut(w) {
        let mut corner = above[0];
        let mut left = row[0].wrapping_add(corner);
        row[0] = left;
        for (sample, &up) in row[1..].iter_mut().zip(&above[1..]) {
            let (lo, hi) = (left.min(up), left.max(up));
            let gradient = i32::from(left) + i32::from(up) - i32::from(corner);
            let prediction = gradient.clamp(i32::from(lo), i32::from(hi)) as u8;
            left = sample.wrapping_add(prediction);
            *sample = left;
            corner = up;
        }
        above = row;
    }
}

/// The pixels of three planes of samples, green, red less green and blue less
/// green, as `R, G, B, A` into `out`, which is four bytes to each of theirs.
pub fn rgba(green: &[u8], red: &[u8], blue: &[u8], out: &mut [u8]) {
    for (((to, &g), &r), &b) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(green)
        .zip(red)
        .zip(blue)
    {
        *to = [r.wrapping_add(g), g, b.wrapping_add(g), 255];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prediction as wlshare's protocol words it, a sample at a time.
    fn predicted(plane: &[u8], w: usize, x: usize, y: usize) -> i32 {
        let at = |x: usize, y: usize| i32::from(plane[y * w + x]);
        match (x, y) {
            (0, 0) => 0,
            (_, 0) => at(x - 1, 0),
            (0, _) => at(0, y - 1),
            _ => {
                let (a, b, c) = (at(x - 1, y), at(x, y - 1), at(x - 1, y - 1));
                let mut three = [a, b, a + b - c];
                three.sort_unstable();
                three[1]
            }
        }
    }

    /// Every shape of plane comes back from its residuals, taken by the
    /// protocol's words: noise, whose medians are every case of the three.
    #[test]
    fn a_planes_samples_come_back_from_their_residuals() {
        for (w, h) in [(1, 1), (1, 7), (7, 1), (2, 2), (3, 5), (64, 64), (200, 9)] {
            let mut n = (w * 31 + h) as u32;
            let plane: Vec<u8> = (0..w * h)
                .map(|i| {
                    n = n.wrapping_mul(0x2C1B_3C6D) ^ (n >> 13) ^ i as u32;
                    // Smooth in places, so the gradient is often between the two.
                    if i % 3 == 0 {
                        (n >> 9) as u8
                    } else {
                        (i / 2) as u8
                    }
                })
                .collect();
            let mut residuals: Vec<u8> = (0..w * h)
                .map(|i| {
                    (i32::from(plane[i]) - predicted(&plane, w, i % w, i / w)).rem_euclid(256) as u8
                })
                .collect();
            samples(&mut residuals, w);
            assert_eq!(residuals, plane, "{w}x{h}");
        }
    }

    #[test]
    fn the_pixels_are_green_and_the_two_less_it() {
        let mut out = [0u8; 8];
        rgba(&[10, 250], &[5, 10], &[255, 6], &mut out);
        assert_eq!(out, [15, 10, 9, 255, 4, 250, 0, 255]);
    }

    /// A zstd frame of `content` as one raw block, its size in its header.
    fn frame(content: &[u8]) -> Vec<u8> {
        let mut frame = ZSTD_MAGIC.to_vec();
        // Single segment, a one-byte content size.
        frame.extend_from_slice(&[0x20, content.len() as u8]);
        let header = ((content.len() as u32) << 3) | 1;
        frame.extend_from_slice(&header.to_le_bytes()[..3]);
        frame.extend_from_slice(content);
        frame
    }

    /// A body is three frames of a plane each, and nothing else.
    #[test]
    fn a_body_is_three_frames_of_a_plane_each() {
        let plane = frame(&[7; 6]);
        let body = [&plane[..], &plane, &plane].concat();
        assert_eq!(check(&body, 3, 2), Ok(()));
        assert_eq!(check(&body, 2, 3), Ok(()));
        assert!(check(&body, 3, 3).is_err(), "a plane of another size");
        assert!(check(&body[..body.len() - 1], 3, 2).is_err(), "cut short");
        assert!(check(&[&plane[..], &plane].concat(), 3, 2).is_err(), "two");
        assert!(check(&[&body[..], &plane].concat(), 3, 2).is_err(), "four");
        let skippable = [&0x184D_2A50u32.to_le_bytes()[..], &1u32.to_le_bytes(), &[0]].concat();
        for with in [
            [&skippable[..], &body].concat(),
            [&body[..], &skippable].concat(),
            [&plane[..], &skippable, &plane, &plane].concat(),
        ] {
            assert!(check(&with, 3, 2).is_err(), "a skippable frame among them");
        }
    }
}
