//! Apple High Performance's sound: what the stream is.
//!
//! The Mac's `RemoteDesktopSystemAudio` transmitter encodes AAC-ELD (MPEG-4 audio
//! object type 39) whatever the negotiation agreed — see `docs/apple-vnc-889.md`.
//! Nothing here decodes it: every browser is passed the Mac's units as they came
//! ([`crate::vnc_apple_media::PASSED_SOUND`]), described by the constants below,
//! and decodes them itself.
//!
//! RTP carries bare access units with no header naming the stream, so the
//! AudioSpecificConfig is stated here, once, from measurement: 48 kHz, stereo,
//! 480-sample frames (the RTP timestamp advances exactly 480 per packet), no SBR
//! and no error-resilience tools. Run against 377 captured frames, a decoder
//! configured with it decoded 375 cleanly and concealed two; every other reading —
//! 512-sample frames, the resilience flags, SBR — either refused the configuration
//! or concealed most of the stream.

/// AudioSpecificConfig for what the Mac sends, bit by bit:
///
/// ```text
/// 11111   audioObjectType escape
/// 000111  audioObjectType 39 - 32 = ER AAC ELD
/// 0011    samplingFrequencyIndex 3 = 48 000 Hz
/// 0010    channelConfiguration 2 = stereo
/// 1       frameLengthFlag: 480-sample frames
/// 0 0 0   section / scalefactor / spectral data resilience: off
/// 0       ldSbrPresentFlag: no SBR
/// 0000    eldExtType ELDEXT_TERM
/// ```
pub const AUDIO_SPECIFIC_CONFIG: [u8; 4] = [0xf8, 0xe6, 0x50, 0x00];

/// Samples per channel in one access unit, which is also one RTP packet: 10 ms.
pub const FRAME_SAMPLES: usize = 480;

/// Channels in the stream, fixed by the configuration above.
pub const CHANNELS: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    /// The bit layout above, re-derived: the constant is the bytes and this is the
    /// arithmetic, so a wrong transcription of either shows up here.
    #[test]
    fn the_audio_specific_config_says_what_its_comment_says() {
        let mut bits = Vec::new();
        let mut push = |value: u32, width: u32| {
            for i in (0..width).rev() {
                bits.push(((value >> i) & 1) as u8);
            }
        };
        push(31, 5);
        push(39 - 32, 6);
        push(3, 4);
        push(2, 4);
        push(1, 1); // 480-sample frames
        push(0, 3); // resilience tools off
        push(0, 1); // no SBR
        push(0, 4); // ELDEXT_TERM
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let bytes: Vec<u8> = bits
            .chunks(8)
            .map(|byte| byte.iter().fold(0u8, |acc, b| (acc << 1) | b))
            .collect();
        assert_eq!(bytes, AUDIO_SPECIFIC_CONFIG);
    }
}
