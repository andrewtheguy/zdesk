//! wlshare's audio extension: the desktop's sound carried on the RFB connection
//! itself, as Opus or as FLAC, and passed to the browser as it came.
//!
//! The extension's bytes are [`wlshare_proto::audio`]'s, the crate wlshare
//! builds and parses them with too. This is the client it is for:
//!
//! - The client lists the pseudo-encoding [`wlshare_proto::ENCODING_AUDIO`]
//!   (`WLSF`) in `SetEncodings`, as this engine does in every `wlshare` session
//!   started with sound, and beside it [`wlshare_proto::ENCODING_AUDIO_OPUS`]
//!   (`WLOP`) unless that sound is lossless: wlshare codes the sound as FLAC for
//!   a list without it and as Opus for one with. The codec is this client's to
//!   choose, from what was chosen at the picker, and nothing in wlshare's
//!   configuration.
//! - A server that speaks it announces so with an **empty pseudo-rectangle** of
//!   the first inside a `FramebufferUpdate` — the only announcement there is,
//!   the same shape ExtendedDesktopSize uses. A server that does not speak it
//!   says nothing, which is how the extension is discovered rather than
//!   configured.
//! - The client then names the format it wants ([`WANTED`]), the rate Opus is
//!   to be coded at, and turns the stream on; it may turn it off again.
//!   Set-format, enable and disable are the client messages of the QEMU Audio
//!   extension `rfbproto` registers, taken as they are; set-bitrate is
//!   wlshare's own operation beside them, and may be sent again while the
//!   stream runs.
//! - The server answers with a begin, a run of frames, one to a frame message,
//!   and an end. Begin and end are QEMU's messages again; the frames are
//!   wlshare's own, each one FLAC frame or one Opus packet.
//!
//! QEMU's own pseudo-encoding, -259, is not listed: what it promises is raw
//! samples, and a server that answers it — QEMU itself — would send them. A
//! target on such a server gets a desktop and no sound, as on any server that
//! does not speak this extension.
//!
//! Nothing is decoded or coded here. An Opus packet is what the browser's own
//! decoder takes, so it goes to the bridge as it came ([`PASSED_OPUS`]), made by
//! the encoder this gateway codes an RDP host's sound with (`sound-opus`), at
//! the rate the target's audio keys and their walk arrive at. A FLAC frame goes
//! the same way in a session started with lossless sound, for
//! the browser's decoder ([`PASSED_FLAC`]). Neither stream's header is sent by
//! wlshare: everything in one follows from the format this client set and the
//! extension's one rule, that every frame is [`BLOCK_FRAMES`] frames of it.
//!
//! wlshare is the server this was built against; see docs/wlshare-audio.md.
//! Apple's dialects are not asked: neither Screen Sharing subtype speaks this
//! extension.

#[cfg(feature = "wlshare")]
use wlshare_proto::audio::{AudioFormat, SampleFormat};

use crate::audio::PcmFormat;

/// What this client asks every server for, and so the one format the sound is
/// coded from: 48 kHz stereo signed 16-bit, little-endian.
///
/// The format is the *client's* to choose — the server converts whatever the
/// desktop plays into it — so there is nothing to negotiate: this is Opus's own
/// rate, and one the browser plays without resampling.
pub const SOURCE_FORMAT: PcmFormat = PcmFormat {
    channels: 2,
    sample_rate: 48_000,
    bits_per_sample: 16,
};

/// The frames in every FLAC frame and Opus packet: twenty milliseconds of [`SOURCE_FORMAT`],
/// which the extension fixes at the frequency over fifty.
pub const BLOCK_FRAMES: u16 = (SOURCE_FORMAT.sample_rate / 50) as u16;

/// [`SOURCE_FORMAT`] as the extension states it.
#[cfg(feature = "wlshare")]
pub const WANTED: AudioFormat = AudioFormat {
    sample: SampleFormat::S16,
    channels: 2,
    frequency: 48_000,
};

/// A lossless session's FLAC frames as the browser is told of them:
/// [`SOURCE_FORMAT`] in blocks of [`BLOCK_FRAMES`],
/// with no head: the page writes the stream header the browser's decoder takes
/// from this format.
pub const PASSED_FLAC: crate::audio::PassedFormat = crate::audio::PassedFormat {
    codec: crate::audio::FLAC_CODEC,
    sample_rate: SOURCE_FORMAT.sample_rate,
    channels: SOURCE_FORMAT.channels,
    packet_frames: BLOCK_FRAMES as u32,
    head: &[],
};

/// An Opus stream's packets as the browser is told of them: the same stream an
/// encoder here makes of an RDP host's sound, behind the `OpusHead` wlshare
/// never sends — two channels, the encoder's lookahead as the pre-skip
/// ([`sound_opus::PRE_SKIP`], 312) and [`SOURCE_FORMAT`]'s rate, as
/// [`sound_opus::Stream::head`] lays it out.
pub const PASSED_OPUS: crate::audio::PassedFormat = crate::audio::PassedFormat {
    codec: crate::opus_stream::OPUS_CODEC,
    sample_rate: SOURCE_FORMAT.sample_rate,
    channels: SOURCE_FORMAT.channels,
    packet_frames: BLOCK_FRAMES as u32,
    head: b"OpusHead\x01\x02\x38\x01\x80\xbb\x00\x00\x00\x00\x00",
};

/// What a session's sound is passed to the browser as: wlshare's Opus, or its
/// FLAC in one started with its sound lossless.
pub fn passed(lossless: bool) -> crate::audio::PassedFormat {
    if lossless { PASSED_FLAC } else { PASSED_OPUS }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format asked for is the format the wave buffers are declared in, and
    /// a frame is twenty milliseconds of it, as the extension has every frame.
    #[cfg(feature = "wlshare")]
    #[test]
    fn the_format_asked_for_is_the_one_declared() {
        assert_eq!(WANTED.sample, SampleFormat::S16);
        assert_eq!(u32::from(WANTED.channels), u32::from(SOURCE_FORMAT.channels));
        assert_eq!(WANTED.frequency, SOURCE_FORMAT.sample_rate);
        assert_eq!(SOURCE_FORMAT.bits_per_sample, 16);
        assert_eq!(WANTED.check(), Ok(()));
        assert_eq!(usize::from(BLOCK_FRAMES), WANTED.block_frames(), "20 ms at 48 kHz");
        assert_eq!(BLOCK_FRAMES, 960);
    }

    /// What the browser is told of a passed stream is what wlshare was asked
    /// for: the head an Opus decoder is configured from is the one the shared
    /// encoder lays out for this format, and FLAC has none.
    #[test]
    fn a_passed_stream_is_described_as_the_format_that_was_asked_for() {
        let stream = sound_opus::Stream { rate: SOURCE_FORMAT.sample_rate, channels: SOURCE_FORMAT.channels as u8 };
        assert_eq!(PASSED_OPUS.head, stream.head(sound_opus::PRE_SKIP, SOURCE_FORMAT.sample_rate).unwrap());
        assert_eq!(stream.block(), usize::from(BLOCK_FRAMES));
        for format in [PASSED_OPUS, PASSED_FLAC] {
            assert_eq!((format.sample_rate, format.channels, format.packet_frames), (48_000, 2, 960));
        }
        assert_eq!((passed(false).codec, passed(true).codec), ("opus", "flac"));
        assert!(PASSED_FLAC.head.is_empty());
    }
}
