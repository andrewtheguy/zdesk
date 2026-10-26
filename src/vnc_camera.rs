//! The wlshare camera extension: the browser's camera, lent to a wlroots-based
//! desktop over the RFB connection the session already has.
//!
//! RFB carries nothing from a client but input and a clipboard, and no registered
//! extension carries video that way, so this is one of wlshare's private ones, in
//! the shape of the density and outputs extensions. Its bytes are
//! [`wlshare_proto::camera`]'s, the crate wlshare builds and parses them with
//! too: pseudo-encoding [`wlshare_proto::ENCODING_CAMERA`] (`WLSC`) and one
//! message type, in both directions:
//!
//! - A session started with the camera offered ([`crate::config::Choices::camera`],
//!   the picker's Camera tick) lists the pseudo-encoding in `SetEncodings`.
//!   wlshare answers with an *available* message, and until it has, the camera
//!   the browser offers is not plugged.
//! - The browser's camera socket plugs the camera: its H.264's geometry and rate, as the
//!   camera socket announced them. A plug that arrives before the server's answer
//!   waits for it ([`Device`]).
//! - The server says when an application on the desktop opens the camera
//!   (*start*) and when the last one closes it (*stop*), and asks for a keyframe
//!   after a gap; each becomes a [`CameraSignal`] on the bridge, which the camera
//!   socket relays to the browser exactly as it relays an RDP host's.
//! - Samples are the browser's Annex B access units, passed through untouched.
//!
//! The camera socket, the bridge and the browser's encoder are the ones RDP's
//! MS-RDPECAM path uses ([`crate::camera`]); this is the VNC engine's adapter to
//! them. See docs/wlshare-camera.md.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use log::{info, warn};
use tokio::sync::mpsc::{self, error::TrySendError};
use wlshare_proto::camera::{self, MAX_SAMPLE};

use crate::camera::{CameraBridge, CameraControl, CameraFormat, CameraSignal};

/// How many samples may wait for the engine's loop to write them. The loop sends
/// them as fast as the socket takes them, so a full queue is a link slower than
/// the camera, and what waits past that is already late.
const SAMPLE_QUEUE: usize = 16;

/// The plug, or `None` for a format the extension cannot describe: a geometry
/// past its `u16`, or — as the RDP path's `Format::is_describable` refuses too —
/// a picture with no area or a rate with either half zero, which wlshare takes as
/// a client that does not speak the extension and ends the whole session over.
fn plug(format: CameraFormat) -> Option<[u8; 16]> {
    if format.width == 0 || format.height == 0 || format.fps_numerator == 0 || format.fps_denominator == 0 {
        return None;
    }
    Some(camera::plug(camera::CameraFormat {
        width: u16::try_from(format.width).ok()?,
        height: u16::try_from(format.height).ok()?,
        fps_numerator: format.fps_numerator,
        fps_denominator: format.fps_denominator,
    }))
}

/// The format a start names, as the bridge carries it.
pub fn bridge_format(format: camera::CameraFormat) -> CameraFormat {
    CameraFormat {
        width: u32::from(format.width),
        height: u32::from(format.height),
        fps_numerator: format.fps_numerator,
        fps_denominator: format.fps_denominator,
    }
}

/// Where the camera stands on one connection: whether the server has said it
/// takes one, and what the browser has plugged. Every method returns the message
/// to send, if any, and is decided under the uplink so two decisions reach the
/// wire in the order they were made.
#[derive(Debug, Default)]
pub struct Device {
    announced: bool,
    plugged: Option<CameraFormat>,
}

impl Device {
    /// The server answered `SetEncodings`. It does so on every one, so only the
    /// first is news — and it is what sends a plug the browser made before it.
    pub fn announce(&mut self) -> Option<Vec<u8>> {
        if self.announced {
            return None;
        }
        self.announced = true;
        info!("vnc: the server takes the browser's camera");
        self.plugged.and_then(plug).map(|msg| msg.to_vec())
    }

    /// The browser plugged its camera, or plugged it again in another format.
    pub fn plug(&mut self, format: CameraFormat) -> Option<Vec<u8>> {
        let Some(msg) = plug(format) else {
            warn!(
                "vnc: a {}x{} camera at {}/{} frames a second is not one the wlshare camera \
                 extension can describe; not plugged",
                format.width, format.height, format.fps_numerator, format.fps_denominator
            );
            return None;
        };
        self.plugged = Some(format);
        if !self.announced {
            info!("vnc: holding the camera's plug until the server says it takes one");
        }
        self.announced.then(|| msg.to_vec())
    }

    /// The browser unplugged its camera.
    pub fn unplug(&mut self) -> Option<Vec<u8>> {
        let was = self.plugged.take();
        (self.announced && was.is_some()).then(|| camera::unplug().to_vec())
    }

    /// One access unit, sent only on a plugged camera the server knows about. Its
    /// length was held to what wlshare accepts before it was queued.
    pub fn sample(&self, unit: &[u8], keyframe: bool) -> Option<Vec<u8>> {
        (self.announced && self.plugged.is_some()).then(|| camera::sample(keyframe, unit))
    }
}

/// The camera's half of one VNC session, shared by the engine's loop and its
/// read loop: the bridge the host's decisions go to, and the device.
#[derive(Debug)]
pub struct Link {
    pub bridge: Arc<CameraBridge>,
    pub device: std::sync::Mutex<Device>,
}

/// A command the camera socket gave, as the engine's loop takes it.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Plug(CameraFormat),
    Unplug,
    Sample { unit: Vec<u8>, keyframe: bool },
}

/// An [`Input`] as it waits, stamped with the plug it belongs to: every plug and
/// unplug starts a new generation, and every sample carries the one current when
/// the browser sent it.
type Stamped = (u64, Input);

/// The engine loop's end of the camera socket's traffic. Two queues, for the
/// reason [`crate::rdp_client::CameraFeed`] has two: a plug or an unplug must
/// never be lost, and a late sample is worthless.
pub struct Queues {
    commands: mpsc::UnboundedReceiver<Stamped>,
    samples: mpsc::Receiver<Stamped>,
    /// The generation of the last command taken.
    generation: u64,
}

impl Queues {
    /// The next thing the socket wants sent, or `None` once the control is gone.
    ///
    /// Commands go ahead of the samples queued behind them, so a replug can be
    /// taken while the old camera's samples still wait; those are of an older
    /// generation than the command last taken, and are dropped here rather than
    /// sent as the new camera's — a picture of the wrong size, or a frame from the
    /// middle of another stream.
    pub async fn next(&mut self) -> Option<Input> {
        loop {
            tokio::select! {
                // Biased so a plug or an unplug goes before samples queued behind it.
                biased;
                Some((generation, command)) = self.commands.recv() => {
                    self.generation = generation;
                    return Some(command);
                }
                Some((generation, sample)) = self.samples.recv() => {
                    if generation == self.generation {
                        return Some(sample);
                    }
                }
                else => return None,
            }
        }
    }
}

/// Register the engine's control on the bridge, returning the queues it feeds.
///
/// The control holds the bridge weakly: the bridge holds the control, and a strong
/// reference back would keep both alive past the engine they belong to.
pub fn attach(bridge: &Arc<CameraBridge>) -> (Link, Queues) {
    let (commands_tx, commands) = mpsc::unbounded_channel();
    let (samples_tx, samples) = mpsc::channel(SAMPLE_QUEUE);
    bridge.set_control(Arc::new(Control {
        commands: commands_tx,
        samples: samples_tx,
        generation: AtomicU64::new(0),
        gap: AtomicBool::new(false),
        bridge: Arc::downgrade(bridge),
    }));
    (
        Link { bridge: Arc::clone(bridge), device: std::sync::Mutex::new(Device::default()) },
        Queues { commands, samples, generation: 0 },
    )
}

struct Control {
    commands: mpsc::UnboundedSender<Stamped>,
    samples: mpsc::Sender<Stamped>,
    /// The current plug's generation, which the samples sent after it carry.
    generation: AtomicU64,
    /// Whether samples are being dropped until a keyframe.
    gap: AtomicBool,
    bridge: Weak<CameraBridge>,
}

impl Control {
    /// Start a new generation and queue the command that begins it. A gap in the
    /// old stream says nothing about the new one, which opens on a keyframe.
    fn command(&self, input: Input) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.gap.store(false, Ordering::Relaxed);
        // A closed queue is an engine that has ended, and the device went with it.
        let _ = self.commands.send((generation, input));
    }

    /// Drop a sample, opening a gap that only a keyframe closes, and ask the
    /// browser for that keyframe once per gap — or again, when a keyframe itself
    /// is what was dropped. Returns `false`, the refusal the bridge reports.
    fn open_gap(&self, keyframe: bool) -> bool {
        if (keyframe || !self.gap.swap(true, Ordering::Relaxed))
            && let Some(bridge) = self.bridge.upgrade()
        {
            bridge.signal(CameraSignal::Keyframe);
        }
        self.gap.store(true, Ordering::Relaxed);
        false
    }
}

impl CameraControl for Control {
    fn plug(&self, format: CameraFormat) {
        self.command(Input::Plug(format));
    }

    fn unplug(&self) {
        self.command(Input::Unplug);
    }

    /// Nothing to tell wlshare: its samples are not asked for one by one, so none is
    /// left owed, and the application letting the camera go sends the stop that the
    /// next opening's start follows.
    fn unavailable(&self) {}

    /// A sample wlshare would refuse, or one that finds the queue full, is dropped
    /// with every later one but a keyframe.
    fn sample(&self, data: &[u8], keyframe: bool) -> bool {
        if !keyframe && self.gap.load(Ordering::Relaxed) {
            return false;
        }
        if data.len() > MAX_SAMPLE {
            warn!("vnc: dropped a {}-byte camera sample, over the {MAX_SAMPLE} bytes wlshare accepts", data.len());
            return self.open_gap(keyframe);
        }
        let generation = self.generation.load(Ordering::Relaxed);
        match self.samples.try_send((generation, Input::Sample { unit: data.to_vec(), keyframe })) {
            Ok(()) => {
                self.gap.store(false, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) => self.open_gap(keyframe),
            Err(TrySendError::Closed(_)) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::error::TryRecvError;

    const VGA: CameraFormat = CameraFormat { width: 640, height: 480, fps_numerator: 30_000, fps_denominator: 1_001 };
    const QVGA: CameraFormat = CameraFormat { width: 320, height: 240, fps_numerator: 15, fps_denominator: 1 };

    /// A client message as wlshare's docs/architecture.md lays it out, read back without
    /// the builders.
    #[derive(Debug, PartialEq, Eq)]
    enum Sent {
        Plug(CameraFormat),
        Unplug,
        Sample { keyframe: bool, unit: Vec<u8> },
    }

    fn decode(bytes: &[u8]) -> Sent {
        assert_eq!(bytes[0], 0xE2);
        let be32 = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        match bytes[1] {
            0 => {
                assert_eq!((bytes.len(), &bytes[2..4]), (16, &[0u8, 0][..]));
                Sent::Plug(CameraFormat {
                    width: u32::from(u16::from_be_bytes([bytes[4], bytes[5]])),
                    height: u32::from(u16::from_be_bytes([bytes[6], bytes[7]])),
                    fps_numerator: be32(8),
                    fps_denominator: be32(12),
                })
            }
            1 => {
                assert_eq!(bytes, [0xE2, 1, 0, 0]);
                Sent::Unplug
            }
            2 => {
                assert_eq!(bytes[3], 0, "padding");
                let len = be32(4) as usize;
                assert_eq!(bytes.len(), 8 + len, "the message is exactly its unit");
                Sent::Sample { keyframe: bytes[2] & 1 != 0, unit: bytes[8..].to_vec() }
            }
            other => panic!("operation {other} is not a client message"),
        }
    }

    #[test]
    fn client_messages_have_the_documented_layouts() {
        let bytes = plug(VGA).unwrap();
        assert_eq!(bytes, [0xE2, 0, 0, 0, 0x02, 0x80, 0x01, 0xE0, 0, 0, 0x75, 0x30, 0, 0, 0x03, 0xE9]);
        assert_eq!(decode(&bytes), Sent::Plug(VGA));
        assert_eq!(decode(&camera::unplug()), Sent::Unplug);
        assert_eq!(decode(&camera::sample(true, &[0, 0, 1, 0x65])), Sent::Sample { keyframe: true, unit: vec![0, 0, 1, 0x65] });
        assert_eq!(decode(&camera::sample(false, &[7])), Sent::Sample { keyframe: false, unit: vec![7] });
        assert_eq!(plug(CameraFormat { width: 70_000, ..VGA }), None, "no geometry past a u16");
    }

    /// A format with no area or no rate would end the session at wlshare, so it is
    /// never put on the wire, and a camera already plugged stays as it was.
    #[test]
    fn a_format_the_extension_cannot_describe_is_not_plugged() {
        for format in [
            CameraFormat { width: 0, ..VGA },
            CameraFormat { height: 0, ..VGA },
            CameraFormat { fps_numerator: 0, ..VGA },
            CameraFormat { fps_denominator: 0, ..VGA },
        ] {
            assert_eq!(plug(format), None, "{format:?}");
        }
        let mut device = Device::default();
        assert_eq!(device.announce(), None);
        assert_eq!(decode(&device.plug(VGA).unwrap()), Sent::Plug(VGA));
        assert_eq!(device.plug(CameraFormat { fps_denominator: 0, ..QVGA }), None);
        assert_eq!(decode(&device.sample(&[1], true).unwrap()), Sent::Sample { keyframe: true, unit: vec![1] });
    }

    /// A plug made before the server answers waits for the answer, and goes out
    /// with it; an answer repeated for a later `SetEncodings` sends nothing again.
    #[test]
    fn a_plug_before_the_announcement_goes_out_with_it() {
        let mut device = Device::default();
        assert_eq!(device.plug(VGA), None);
        assert_eq!(device.sample(&[1], true), None, "nothing is sent to a server that has not answered");
        assert_eq!(decode(&device.announce().unwrap()), Sent::Plug(VGA));
        assert_eq!(device.announce(), None);
        assert_eq!(decode(&device.sample(&[1], true).unwrap()), Sent::Sample { keyframe: true, unit: vec![1] });
        assert_eq!(decode(&device.unplug().unwrap()), Sent::Unplug);
        assert_eq!(device.unplug(), None, "an unplugged camera is not unplugged twice");
        assert_eq!(device.sample(&[1], true), None);
    }

    /// Until wlshare answers, not a byte of the extension is sent, whatever the
    /// browser does.
    #[test]
    fn a_server_that_never_answers_hears_nothing() {
        let mut device = Device::default();
        assert_eq!(device.plug(VGA), None);
        assert_eq!(device.unplug(), None);
        assert_eq!(device.plug(VGA), None);
        assert_eq!(device.sample(&[1], true), None);
    }

    /// The socket's traffic reaches the queues; a full sample queue, or a unit
    /// wlshare would refuse, drops to a keyframe and asks the browser for one once
    /// per gap.
    #[tokio::test]
    async fn a_dropped_sample_drops_to_a_keyframe_and_asks_for_one_once() {
        let bridge = Arc::new(CameraBridge::new());
        let mut signals = bridge.subscribe();
        let (_link, mut queues) = attach(&bridge);

        bridge.plug(VGA);
        assert!(bridge.sample(&[1], true));
        assert_eq!(queues.next().await, Some(Input::Plug(VGA)));
        assert_eq!(queues.next().await, Some(Input::Sample { unit: vec![1], keyframe: true }));

        for _ in 0..SAMPLE_QUEUE {
            assert!(bridge.sample(&[2], false));
        }
        assert!(!bridge.sample(&[3], false));
        assert_eq!(signals.try_recv(), Ok(CameraSignal::Keyframe));
        assert!(!bridge.sample(&[4], false));
        assert_eq!(signals.try_recv(), Err(TryRecvError::Empty), "asked once per gap");
        assert!(!bridge.sample(&[5], true));
        assert_eq!(signals.try_recv(), Ok(CameraSignal::Keyframe), "a keyframe lost to the queue is owed again");

        while let Ok((_, sample)) = queues.samples.try_recv() {
            assert_eq!(sample, Input::Sample { unit: vec![2], keyframe: false });
        }
        assert!(!bridge.sample(&[6], false), "room is not a keyframe");
        assert!(bridge.sample(&[7], true));
        assert!(bridge.sample(&[8], false), "the gap closed");

        // An oversized unit opens the same gap, even when it is the keyframe a
        // stream starts on: without the request the stream would never start.
        assert!(!bridge.sample(&vec![0; MAX_SAMPLE + 1], true));
        assert_eq!(signals.try_recv(), Ok(CameraSignal::Keyframe));
        assert!(!bridge.sample(&[9], false), "the deltas after it are dropped too");
        assert_eq!(signals.try_recv(), Err(TryRecvError::Empty));
        assert!(bridge.sample(&[10], true));
    }

    /// A replug taken while the old camera's samples still wait leaves them behind:
    /// commands go first, and a sample of an older generation is never sent as the
    /// new camera's.
    #[tokio::test]
    async fn a_replug_leaves_the_old_cameras_samples_behind() {
        let bridge = Arc::new(CameraBridge::new());
        let (_link, mut queues) = attach(&bridge);

        bridge.plug(VGA);
        assert!(bridge.sample(&[1], true));
        bridge.unplug();
        bridge.plug(QVGA);
        assert!(bridge.sample(&[2], true));

        assert_eq!(queues.next().await, Some(Input::Plug(VGA)));
        assert_eq!(queues.next().await, Some(Input::Unplug));
        assert_eq!(queues.next().await, Some(Input::Plug(QVGA)));
        assert_eq!(queues.next().await, Some(Input::Sample { unit: vec![2], keyframe: true }));

        // And a sample from before any plug belongs to no camera at all.
        let bridge = Arc::new(CameraBridge::new());
        let (_link, mut queues) = attach(&bridge);
        assert!(bridge.sample(&[3], true));
        bridge.plug(VGA);
        assert!(bridge.sample(&[4], true));
        assert_eq!(queues.next().await, Some(Input::Plug(VGA)));
        assert_eq!(queues.next().await, Some(Input::Sample { unit: vec![4], keyframe: true }));
    }
}
