//! The wlshare microphone extension: the browser's microphone, lent to a wlroots-based
//! desktop over the RFB connection the session already has.
//!
//! The camera extension's twin ([`crate::vnc_camera`]), and wlshare's fourth
//! private one. Its bytes are [`wlshare_proto::microphone`]'s, the crate wlshare
//! builds and parses them with too: pseudo-encoding
//! [`wlshare_proto::ENCODING_MICROPHONE`] (`WLSM`) and one message type, in both
//! directions:
//!
//! - A session started with the microphone offered
//!   ([`crate::config::Choices::microphone`], the picker's Microphone tick) lists the
//!   pseudo-encoding in `SetEncodings`.
//!   wlshare answers with an *available* message, and until it has, the
//!   microphone the browser offers is not plugged.
//! - The browser's mic socket attaching plugs the microphone, and the
//!   socket going away unplugs it. A plug that arrives before the server's answer
//!   waits for it ([`Device`]).
//! - The server says when an application on the desktop starts recording (*start*,
//!   naming the PCM it wants) and when the last one stops (*stop*); each becomes a
//!   [`MicSignal`] on the bridge, which the mic socket relays to the browser exactly
//!   as it relays an RDP host's.
//! - Samples are the bridge's PCM — the browser's Opus, decoded to the format the
//!   start named — sent while the desktop records and dropped otherwise.
//!
//! The mic socket, the bridge and the browser's encoder are the ones MS-RDPEAI uses
//! ([`crate::mic`]); this is the VNC engine's adapter to them. See
//! docs/wlshare-microphone.md.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use log::{info, warn};
use tokio::sync::{Notify, mpsc};
use wlshare_proto::microphone::{self, MAX_SAMPLE};

use crate::mic::{MicBridge, MicControl, MicFormat};
#[cfg(doc)]
use crate::mic::MicSignal;

/// How many buffers may wait for the engine's loop to write them: at the bridge's
/// 20 ms, a third of a second. A full queue drops its oldest, so a link that was
/// held up resumes at the live edge.
const SAMPLE_QUEUE: usize = 16;

/// The format a start names, as the bridge carries it.
pub fn bridge_format(format: microphone::MicrophoneFormat) -> MicFormat {
    MicFormat { channels: format.channels, sample_rate: format.frequency }
}

/// What a plug or an unplug decided: the message to send, if any, and whether a
/// recording the browser was feeding ended with it.
#[derive(Debug, PartialEq, Eq)]
pub struct Decision {
    pub message: Option<Vec<u8>>,
    /// The bridge must hear a close: wlshare sends no stop for a microphone it no
    /// longer has, and the bridge's standing open would otherwise outlive it.
    pub closed: bool,
}

/// Where the microphone stands on one connection: whether the server has said it
/// takes one, whether the browser has plugged one, and whether the desktop records
/// from it. Decided under the uplink, so two decisions reach the wire in the order
/// they were made.
#[derive(Debug, Default)]
pub struct Device {
    announced: bool,
    plugged: bool,
    recording: bool,
}

impl Device {
    /// The server answered `SetEncodings`. It does so on every one, so only the
    /// first is news — and it is what sends a plug the browser made before it.
    pub fn announce(&mut self) -> Option<Vec<u8>> {
        if self.announced {
            return None;
        }
        self.announced = true;
        info!("vnc: the server takes the browser's microphone");
        self.plugged.then(|| microphone::plug().to_vec())
    }

    /// The browser enabled its microphone. A microphone already plugged is replaced,
    /// and whatever recorded from the old one is over.
    pub fn plug(&mut self) -> Decision {
        let closed = std::mem::take(&mut self.recording);
        self.plugged = true;
        if !self.announced {
            info!("vnc: holding the microphone's plug until the server says it takes one");
        }
        Decision { message: self.announced.then(|| microphone::plug().to_vec()), closed }
    }

    /// The browser disabled its microphone.
    pub fn unplug(&mut self) -> Decision {
        let closed = std::mem::take(&mut self.recording);
        let was = std::mem::take(&mut self.plugged);
        Decision { message: (self.announced && was).then(|| microphone::unplug().to_vec()), closed }
    }

    /// The server's start. Heard only for a microphone plugged on the wire: a start
    /// already in flight when the browser unplugged belongs to no microphone.
    pub fn start(&mut self) -> bool {
        self.recording = self.announced && self.plugged;
        self.recording
    }

    /// The server's stop. Returns whether a recording ended.
    pub fn stop(&mut self) -> bool {
        std::mem::take(&mut self.recording)
    }

    /// One buffer of PCM, sent only while the desktop records. Its length was held
    /// to what wlshare accepts before it was queued.
    pub fn sample(&self, pcm: &[u8]) -> Option<Vec<u8>> {
        self.recording.then(|| microphone::sample(pcm))
    }
}

/// The microphone's half of one VNC session, shared by the engine's loop and its
/// read loop: the bridge the desktop's decisions go to, and the device.
#[derive(Debug)]
pub struct Link {
    pub bridge: Arc<MicBridge>,
    pub device: Mutex<Device>,
}

/// A command the mic socket gave, as the engine's loop takes it.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Plug,
    Unplug,
    Sample(Vec<u8>),
}

/// The PCM waiting for the engine's loop, newest kept.
#[derive(Default)]
struct Samples {
    buffers: Mutex<VecDeque<Vec<u8>>>,
    ready: Notify,
}

impl Samples {
    fn pop(&self) -> Option<Vec<u8>> {
        self.buffers.lock().expect("microphone sample lock").pop_front()
    }
}

/// The engine loop's end of the mic socket's traffic. Two queues, for the reason the
/// camera has two: a plug or an unplug must never be lost, and late audio is worse
/// than a moment missing.
pub struct Queues {
    commands: mpsc::UnboundedReceiver<Input>,
    samples: Arc<Samples>,
}

impl Queues {
    /// The next thing the socket wants sent, a command ahead of any PCM; `None` once
    /// the control is gone.
    pub async fn next(&mut self) -> Option<Input> {
        loop {
            match self.commands.try_recv() {
                Ok(command) => return Some(command),
                Err(mpsc::error::TryRecvError::Disconnected) => return None,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
            if let Some(pcm) = self.samples.pop() {
                return Some(Input::Sample(pcm));
            }
            tokio::select! {
                biased;
                command = self.commands.recv() => return command,
                // A buffer queued between the check and here left a permit.
                () = self.samples.ready.notified() => {}
            }
        }
    }
}

/// Register the engine's control on the bridge, returning the queues it feeds.
pub fn attach(bridge: &Arc<MicBridge>) -> (Link, Queues) {
    let (commands_tx, commands) = mpsc::unbounded_channel();
    let samples = Arc::new(Samples::default());
    bridge.set_control(Arc::new(Control { commands: commands_tx, samples: Arc::clone(&samples) }));
    (Link { bridge: Arc::clone(bridge), device: Mutex::new(Device::default()) }, Queues { commands, samples })
}

struct Control {
    commands: mpsc::UnboundedSender<Input>,
    samples: Arc<Samples>,
}

impl Control {
    fn clear(&self) {
        self.samples.buffers.lock().expect("microphone sample lock").clear();
    }
}

impl MicControl for Control {
    fn plug(&self) {
        // A closed queue is an engine that has ended, and the device went with it.
        let _ = self.commands.send(Input::Plug);
    }

    /// What was queued for the old microphone goes with it.
    fn unplug(&self) {
        self.clear();
        let _ = self.commands.send(Input::Unplug);
    }

    fn sample(&self, pcm: Vec<u8>) -> bool {
        if pcm.len() > MAX_SAMPLE {
            warn!("vnc: dropped {} bytes of microphone PCM, over the {MAX_SAMPLE} wlshare accepts", pcm.len());
            return false;
        }
        {
            let mut buffers = self.samples.buffers.lock().expect("microphone sample lock");
            if buffers.len() == SAMPLE_QUEUE {
                buffers.pop_front();
            }
            buffers.push_back(pcm);
        }
        self.samples.ready.notify_one();
        true
    }

    /// The recording ended: what it left queued is not the next one's.
    fn reset(&self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client message as wlshare's docs/architecture.md lays it out, read back without
    /// the builders.
    #[derive(Debug, PartialEq, Eq)]
    enum Sent {
        Plug,
        Unplug,
        Sample(Vec<u8>),
    }

    fn decode(bytes: &[u8]) -> Sent {
        assert_eq!(bytes[0], 0xE3);
        assert_eq!(&bytes[2..4], &[0, 0], "padding");
        match bytes[1] {
            0 => {
                assert_eq!(bytes.len(), 4);
                Sent::Plug
            }
            1 => {
                assert_eq!(bytes.len(), 4);
                Sent::Unplug
            }
            2 => {
                let len = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
                assert_eq!(bytes.len(), 8 + len, "the message is exactly its PCM");
                Sent::Sample(bytes[8..].to_vec())
            }
            other => panic!("operation {other} is not a client message"),
        }
    }

    #[test]
    fn client_messages_have_the_documented_layouts() {
        assert_eq!(microphone::plug(), [0xE3, 0, 0, 0]);
        assert_eq!(decode(&microphone::plug()), Sent::Plug);
        assert_eq!(decode(&microphone::unplug()), Sent::Unplug);
        assert_eq!(microphone::sample(&[0x34, 0x12]), [0xE3, 2, 0, 0, 0, 0, 0, 2, 0x34, 0x12]);
        assert_eq!(decode(&microphone::sample(&[1, 2, 3, 4])), Sent::Sample(vec![1, 2, 3, 4]));
    }

    /// A plug made before the server answers waits for the answer, and goes out with
    /// it; an answer repeated for a later `SetEncodings` sends nothing again. Samples
    /// go out between a start and a stop alone.
    #[test]
    fn a_plug_before_the_announcement_goes_out_with_it() {
        let mut device = Device::default();
        assert_eq!(device.plug(), Decision { message: None, closed: false });
        assert!(!device.start(), "no start is heard for a plug the server has not seen");
        assert_eq!(device.sample(&[1, 0]), None);
        assert_eq!(device.announce().map(|m| decode(&m)), Some(Sent::Plug));
        assert_eq!(device.announce(), None);
        assert_eq!(device.sample(&[1, 0]), None, "nothing records yet");
        assert!(device.start());
        assert_eq!(decode(&device.sample(&[1, 0]).unwrap()), Sent::Sample(vec![1, 0]));
        assert!(device.stop());
        assert!(!device.stop(), "a stop ends one recording once");
        assert_eq!(device.sample(&[1, 0]), None);
        let unplugged = device.unplug();
        assert_eq!(unplugged.message.map(|m| decode(&m)), Some(Sent::Unplug));
        assert!(!unplugged.closed);
        assert_eq!(device.unplug(), Decision { message: None, closed: false }, "not unplugged twice");
    }

    /// Unplugging or replugging while the desktop records ends the recording at the
    /// bridge — wlshare will not send a stop for a microphone it no longer has — and a
    /// start that was in flight when the browser unplugged is not heard.
    #[test]
    fn unplugging_a_recording_microphone_closes_it() {
        let mut device = Device::default();
        device.announce();
        device.plug();
        assert!(device.start());
        assert_eq!(device.unplug(), Decision { message: Some(microphone::unplug().to_vec()), closed: true });
        assert!(!device.start(), "a start for the unplugged microphone");
        assert_eq!(device.sample(&[1, 0]), None);

        assert_eq!(device.plug(), Decision { message: Some(microphone::plug().to_vec()), closed: false });
        assert!(device.start());
        assert_eq!(device.plug(), Decision { message: Some(microphone::plug().to_vec()), closed: true }, "a replug replaces it");
        assert_eq!(device.sample(&[1, 0]), None, "until the new microphone's start");
    }

    /// Until wlshare answers, not a byte of the extension is sent, whatever the
    /// browser does.
    #[test]
    fn a_server_that_never_answers_hears_nothing() {
        let mut device = Device::default();
        assert_eq!(device.plug().message, None);
        assert_eq!(device.unplug().message, None);
        assert_eq!(device.plug().message, None);
        assert!(!device.start());
        assert_eq!(device.sample(&[1, 0]), None);
    }

    /// Commands go ahead of the PCM queued before them, a full queue keeps the newest
    /// audio, and an unplug or a close drops what was queued.
    #[tokio::test]
    async fn the_queues_keep_commands_first_and_the_newest_audio() {
        let bridge = Arc::new(MicBridge::new());
        let (_link, mut queues) = attach(&bridge);

        bridge.plug();
        let control = Control { commands: mpsc::unbounded_channel().0, samples: Arc::clone(&queues.samples) };
        for n in 0..=SAMPLE_QUEUE {
            assert!(control.sample(vec![n as u8]));
        }
        assert_eq!(queues.next().await, Some(Input::Plug));
        assert_eq!(queues.next().await, Some(Input::Sample(vec![1])), "the oldest was dropped");

        control.reset();
        assert!(queues.samples.pop().is_none(), "a close drops the queue");

        assert!(control.sample(vec![9]));
        bridge.unplug();
        assert_eq!(queues.next().await, Some(Input::Unplug));
        assert!(queues.samples.pop().is_none(), "an unplug drops the queue");

        assert!(!control.sample(vec![0; MAX_SAMPLE + 1]));
        assert!(queues.samples.pop().is_none());
    }

    #[tokio::test]
    async fn a_waiting_loop_wakes_for_audio() {
        let bridge = Arc::new(MicBridge::new());
        let (_link, mut queues) = attach(&bridge);
        let samples = Arc::clone(&queues.samples);
        let waiting = tokio::spawn(async move { queues.next().await });
        tokio::task::yield_now().await;
        let control = Control { commands: mpsc::unbounded_channel().0, samples };
        assert!(control.sample(vec![7, 0]));
        assert_eq!(waiting.await.unwrap(), Some(Input::Sample(vec![7, 0])));
    }
}
