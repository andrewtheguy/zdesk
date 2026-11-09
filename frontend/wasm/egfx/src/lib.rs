//! The page's compositor for an RDP host's graphics pipeline.
//!
//! In a session started with the pipeline passed the gateway passes its commands
//! to the browser instead of composing them and encoding the picture. This is what
//! composes them there: the gateway's own compositor and codecs, which are the
//! `zdesk-rdp-graphics` crate both are built with, behind the few calls the paint
//! worker makes (`frontend/src/egfxCompositor.ts`).
//!
//! The framebuffer stays in this module's memory, which is shared with the threads
//! below: the page uploads the rectangles a run painted out of it, into a texture.
//!
//! H.264 is the one codec not decoded here: the page decodes a run's access units
//! with the browser's own decoder before it composes the run, and hands each
//! picture's samples into this memory ([`Egfx::units`], [`Egfx::reserve`],
//! [`Egfx::supply`]), or leaves a picture on its GPU and draws its rectangles from
//! there ([`Egfx::plan`], [`Egfx::show_on_gpu`], [`Egfx::gpu_paints`]), reading
//! back what a later command reads ([`Egfx::restore`]).
//!
//! Progressive's tiles are decoded side by side, on rayon's pool, and a page has no
//! threads to spawn: a thread is a worker the page starts, running an instance of
//! this module on the one memory. So the pool's threads are seats the page's workers
//! take ([`run_pool_thread`]) before the pool is made of them ([`start_pool`]). A
//! module whose pool was never started composes on the thread that calls it.
//!
//! The pool is the module's own, named to the graphics crate
//! ([`zdesk_rdp_graphics::pool`]), not rayon's global one, so that it can be
//! stopped ([`stop_pool`]): a thread waiting for work waits inside this module,
//! and iOS frees nothing of a worker ended there — not its instance, not the
//! memory it shares — until its tab is closed. Stopped, each thread returns from
//! [`run_pool_thread`], and its worker can be ended.
//!
//! wlshare's progressive tiles are decoded here too, by the same crate's
//! RemoteFX Progressive decoder ([`WlshareTiles`]): each a tile block the
//! page hands in order, put in a stream of its own with the protocol's tables
//! (`wlshare_proto::progressive`), its tile's pixels handed back. And so are
//! its lossless tiles, by the planar codec that draws a Windows host's lossless
//! pipeline, a batch's at once ([`WlshareTiles::lossless`]): the moving tiles'
//! zstd frames side by side on the same pool, beside the still stream's bodies in
//! order, then every tile's bitmap side by side, their pixels handed back.

use std::cell::RefCell;
use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};

use rayon::prelude::*;
use rayon::{ThreadBuilder, ThreadPoolBuilder};
use wasm_bindgen::prelude::*;
use zdesk_rdp_graphics::avc::{self, Picture, Plane, Samples, Scanned, Window};
use zdesk_rdp_graphics::{Compositor, GpuArea, Rect};

/// The pool's threads on their way from the pool that makes them to the workers
/// that run them.
struct Seats {
    offer: Mutex<Sender<ThreadBuilder>>,
    take: Mutex<Receiver<ThreadBuilder>>,
}

fn seats() -> &'static Seats {
    static SEATS: OnceLock<Seats> = OnceLock::new();
    SEATS.get_or_init(|| {
        let (offer, take) = channel();
        Seats {
            offer: Mutex::new(offer),
            take: Mutex::new(take),
        }
    })
}

/// The compiled module, for a worker to make its instance of.
#[wasm_bindgen]
pub fn module() -> JsValue {
    wasm_bindgen::module()
}

/// Run one of the pool's threads on the worker that calls this, which waits for
/// the pool to be started and returns once [`stop_pool`] has stopped it.
#[wasm_bindgen(js_name = runPoolThread)]
pub fn run_pool_thread() {
    let thread = seats()
        .take
        .lock()
        .expect("no thread panics holding a seat")
        .recv();
    if let Ok(thread) = thread {
        thread.run();
    }
}

/// Make the pool, of `threads` workers that are each in [`run_pool_thread`]
/// already: this waits for every thread to have started, and a worker cannot start
/// while the thread that made it waits.
#[wasm_bindgen(js_name = startPool)]
pub fn start_pool(threads: usize) -> Result<(), JsError> {
    let offer = seats()
        .offer
        .lock()
        .expect("no thread panics holding a seat")
        .clone();
    let pool = ThreadPoolBuilder::new()
        .num_threads(threads)
        // The thread that was not sent is dropped with the error: it is not `Sync`,
        // which an error is.
        .spawn_handler(move |thread| {
            offer
                .send(thread)
                .map_err(|_| io::Error::other("the pool's seats are gone"))
        })
        .build()
        .map_err(|e| JsError::new(&e.to_string()))?;
    zdesk_rdp_graphics::pool::set(Some(pool));
    Ok(())
}

/// Stop the pool: each of its threads finishes what it holds and returns from
/// [`run_pool_thread`]. Compositors compose on the calling thread after.
#[wasm_bindgen(js_name = stopPool)]
pub fn stop_pool() {
    zdesk_rdp_graphics::pool::set(None);
}

/// One pipeline's compositor. Made where a pipeline starts (`graphicsStart`) and
/// thrown away where the next one does: it is right only for a pipeline it has
/// followed from its first command.
#[wasm_bindgen]
pub struct Egfx {
    compositor: Compositor,
    /// What the last run painted: `x, y, width, height` for each rectangle.
    painted: Vec<u32>,
    resized: bool,
    /// Where a decoded picture's samples land on their way to the compositor:
    /// reserved, written by the page, then supplied.
    landing: Vec<u8>,
    /// What the last run left the page to draw from the pictures it shows from its
    /// GPU: [`GPU_FIELDS`] numbers each.
    gpu: Vec<u32>,
}

/// How many numbers [`Egfx::units`] says each thing it found in.
const UNIT_FIELDS: usize = 10;

/// How many numbers each rectangle of the GPU's is said in, by [`Egfx::plan`],
/// [`Egfx::gpu_areas`] and [`Egfx::gpu_paints`].
const GPU_FIELDS: usize = 7;

/// A rectangle shown from the GPU, as [`GPU_FIELDS`] numbers: its surface, its
/// `x, y, width, height` in the surface, and where its top left is on the output.
fn area_fields(area: &GpuArea) -> [u32; GPU_FIELDS] {
    let GpuArea {
        surface,
        rect,
        output,
    } = area;
    [
        u32::from(*surface),
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        output.x,
        output.y,
    ]
}

#[wasm_bindgen]
impl Egfx {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Egfx {
        Egfx {
            compositor: Compositor::new(),
            painted: Vec::new(),
            resized: false,
            landing: Vec::new(),
            gpu: Vec::new(),
        }
    }

    /// One `GRAPHICS` record's message, as the host sent it, out of its bulk
    /// compression: the commands [`Self::units`] and [`Self::compose`] take. Every
    /// record of the pipeline comes through here, in order, since each is
    /// compressed against the ones before it.
    ///
    /// Throws for a message that does not unwrap, after which none after it can be.
    pub fn unwrap(&mut self, wrapped: &[u8]) -> Result<Vec<u8>, JsError> {
        self.compositor
            .unwrap(wrapped)
            .map_err(|e| JsError::new(&format!("{e:#}")))
    }

    /// Compose one `GRAPHICS` record's commands, unwrapped, and return how many
    /// frames they ended. What they painted is [`Self::painted`] until the next call.
    ///
    /// Throws for a command that does not decode. The compositor is then no longer
    /// the host's picture of its client, and is not to be fed again.
    pub fn compose(&mut self, commands: &[u8]) -> Result<u32, JsError> {
        self.painted.clear();
        self.resized = false;
        let composed = self
            .compositor
            .compose(commands)
            .map_err(|e| JsError::new(&format!("{e:#}")))?;
        self.resized = composed.resized.is_some();
        for rect in composed.painted {
            self.painted
                .extend([rect.x, rect.y, rect.width, rect.height]);
        }
        self.gpu.clear();
        for paint in composed.gpu {
            let output = paint.output;
            self.gpu.extend([
                paint.unit,
                output.x,
                output.y,
                output.width,
                output.height,
                paint.source.0,
                paint.source.1,
            ]);
        }
        Ok(composed.frames)
    }

    /// What a `GRAPHICS` record's commands ask of the page before they are
    /// composed: how many access units they carry, then for each, by its number,
    /// `1` where its picture may be shown from the GPU and `0` where it is to be
    /// supplied, then what of what the GPU shows they read, to be restored first
    /// ([`Self::restore`]), [`GPU_FIELDS`] numbers each.
    ///
    /// Throws for a command that does not decode, which [`Self::compose`] would
    /// throw for too.
    pub fn plan(&self, commands: &[u8]) -> Result<Vec<u32>, JsError> {
        let plan = self
            .compositor
            .plan(commands)
            .map_err(|e| JsError::new(&e.to_string()))?;
        let mut out = Vec::with_capacity(1 + plan.gpu.len() + plan.restore.len() * GPU_FIELDS);
        out.push(plan.gpu.len() as u32);
        out.extend(plan.gpu.iter().map(|&gpu| u32::from(gpu)));
        out.extend(plan.restore.iter().flat_map(area_fields));
        Ok(out)
    }

    /// Show access unit `unit`'s picture, of the record composed next, from the
    /// GPU rather than supplying it: only one [`Self::plan`] said may be.
    #[wasm_bindgen(js_name = showOnGpu)]
    pub fn show_on_gpu(&mut self, unit: u32) {
        self.compositor.show_on_gpu(unit);
    }

    /// What the last run left the page to draw from the pictures it shows from its
    /// GPU, after its painted rectangles: [`GPU_FIELDS`] numbers each — the unit's
    /// number, the output's `x, y, width, height`, and the pixel of the picture at
    /// its top left.
    #[wasm_bindgen(js_name = gpuPaints)]
    pub fn gpu_paints(&self) -> Vec<u32> {
        self.gpu.clone()
    }

    /// Every rectangle shown from the GPU, [`GPU_FIELDS`] numbers each: what the
    /// page reads back to have the framebuffer whole.
    #[wasm_bindgen(js_name = gpuAreas)]
    pub fn gpu_areas(&self) -> Vec<u32> {
        self.compositor
            .gpu_areas()
            .iter()
            .flat_map(area_fields)
            .collect()
    }

    /// A rectangle shown from the GPU, read back off it: `pixels` is the rows of
    /// `width` by `height` from `x`, `y` of the surface, four bytes a pixel, red
    /// first, top row first. Its pixels are the surface's again.
    pub fn restore(
        &mut self,
        surface: u16,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), JsError> {
        self.compositor
            .restore(
                surface,
                Rect {
                    x,
                    y,
                    width,
                    height,
                },
                pixels,
            )
            .map_err(|e| JsError::new(&format!("{e:#}")))
    }

    /// What a `GRAPHICS` record's commands hold for the page to act on before it
    /// composes them, in command order, ten numbers each.
    ///
    /// An H.264 access unit: `0`, its surface, where its bytes start and end in the
    /// commands, whether a decoder can start at it, the profile, constraint and
    /// level bytes of its parameter sets as `0x01PPCCLL` or zero where it has none,
    /// and the part of its picture to supply — `left, top, right, bottom`, all
    /// zero for none of it and all `0xFFFFFFFF` for the whole. A unit's number, for
    /// [`Self::supply`], is how many units the record has before it.
    ///
    /// A surface whose stream is over: `1`, the surface, and zeros.
    ///
    /// Throws for a command that does not decode, which [`Self::compose`] would
    /// throw for too.
    pub fn units(&self, commands: &[u8]) -> Result<Vec<u32>, JsError> {
        let found = avc::scan(commands).map_err(|e| JsError::new(&e.to_string()))?;
        let mut out = Vec::with_capacity(found.len() * UNIT_FIELDS);
        for found in found {
            let fields: [u32; UNIT_FIELDS] = match found {
                Scanned::Gone { surface } => [1, surface.into(), 0, 0, 0, 0, 0, 0, 0, 0],
                Scanned::Unit(unit) => {
                    let profile = unit
                        .profile
                        .map_or(0, |[p, c, l]| u32::from_be_bytes([1, p, c, l]));
                    let [left, top, right, bottom] = match unit.window {
                        Window::Nothing => [0; 4],
                        Window::Part(rect) => {
                            [rect.left, rect.top, rect.right, rect.bottom].map(u32::from)
                        }
                        Window::Whole => [u32::MAX; 4],
                    };
                    // A record is a WebSocket message, far short of four gigabytes.
                    let (start, end) = (unit.start as u32, unit.end as u32);
                    [
                        0,
                        unit.surface.into(),
                        start,
                        end,
                        unit.key.into(),
                        profile,
                        left,
                        top,
                        right,
                        bottom,
                    ]
                }
            };
            out.extend(fields);
        }
        Ok(out)
    }

    /// Room for one decoded picture's samples, `bytes` of them, in this module's
    /// memory: where the page has its decoder copy them. Good until the next call
    /// here or to [`Self::supply`].
    pub fn reserve(&mut self, bytes: usize) -> *mut u8 {
        self.landing = vec![0; bytes];
        self.landing.as_mut_ptr()
    }

    /// Supply the picture just written into [`Self::reserve`]'s room as that of
    /// access unit `unit` of the record composed next.
    ///
    /// The picture holds `width` by `height` pixels from `left`, `top` of the
    /// decoder's, and `layout` is where each of its planes starts in the room and
    /// the bytes between its rows: three pairs for a decoder that hands back Y, U
    /// and V apart (`I420`), two for one that interleaves the chroma (`NV12`).
    pub fn supply(
        &mut self,
        unit: u32,
        left: u32,
        top: u32,
        width: u32,
        height: u32,
        layout: &[u32],
    ) -> Result<(), JsError> {
        let plane = |at: usize| Plane {
            offset: layout[at] as usize,
            stride: layout[at + 1] as usize,
        };
        let samples = match layout.len() {
            6 => Samples::I420 {
                y: plane(0),
                u: plane(2),
                v: plane(4),
            },
            4 => Samples::Nv12 {
                y: plane(0),
                uv: plane(2),
            },
            other => {
                return Err(JsError::new(&format!(
                    "a picture laid out in {other} numbers"
                )));
            }
        };
        let data = std::mem::take(&mut self.landing);
        self.compositor.supply(
            unit,
            Picture {
                data,
                samples,
                left,
                top,
                width,
                height,
            },
        );
        Ok(())
    }

    /// The rectangles the last run painted, four numbers each: `x, y, width,
    /// height`, in framebuffer pixels.
    pub fn painted(&self) -> Vec<u32> {
        self.painted.clone()
    }

    /// Whether the last run reset the output, which leaves the framebuffer at its
    /// new size and blank but for what the run painted after.
    pub fn resized(&self) -> bool {
        self.resized
    }

    pub fn width(&self) -> u32 {
        self.compositor.framebuffer().with(|frame| frame.width)
    }

    pub fn height(&self) -> u32 {
        self.compositor.framebuffer().with(|frame| frame.height)
    }

    /// Where the framebuffer is in this module's memory: `width * height * 4`
    /// bytes, `RGBX` — the fourth byte unused — top row first. Good until the next
    /// [`Self::compose`], which may move it.
    pub fn pixels(&self) -> *const u8 {
        self.compositor
            .framebuffer()
            .with(|frame| frame.pixels.as_ptr())
    }
}

impl Default for Egfx {
    fn default() -> Self {
        Self::new()
    }
}

/// wlshare's progressive tiles as the page draws them: one RemoteFX Progressive
/// decoder over the desktop, which keeps every tile's coefficients from one block
/// to the next. A desktop of another size starts it over. Its lossless tiles too,
/// a batch's at once ([`Self::lossless`]), each alone or in its still stream.
#[wasm_bindgen]
pub struct WlshareTiles {
    decoder: zdesk_rdp_graphics::proto::progressive::Progressive,
    size: (u32, u32),
    frame: u32,
    /// A batch's lossless tiles' bitmaps out of their zstd, one for each tile,
    /// kept from one batch to the next.
    bitmaps: Vec<Vec<u8>>,
    /// The zstd decoders: the moving tiles', whose frames are decoded side by
    /// side, each job's buffers made once for its run of them, by its settings;
    /// and the still stream's, kept so that its window and tables are made once,
    /// reset by a body that begins a stream.
    moving: ruzstd_decompress::decoding::FrameDecoder,
    still: ruzstd_decompress::decoding::FrameDecoder,
    /// Whether `still` holds a stream: not before a body begins one, nor after
    /// a body that did not decode.
    in_still: bool,
}

#[wasm_bindgen]
impl WlshareTiles {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WlshareTiles {
        WlshareTiles {
            decoder: Default::default(),
            size: (0, 0),
            frame: 0,
            bitmaps: Vec::new(),
            moving: ruzstd_decompress::decoding::FrameDecoder::new(),
            still: {
                let mut frame = ruzstd_decompress::decoding::FrameDecoder::new();
                frame.set_max_window_size(STILL_WINDOW_MOST);
                frame
            },
            in_still: false,
        }
    }

    /// The tile `block` is of, decoded against what this holds of it, as `R, G,
    /// B, A` rows of the rectangle `x, y, w, h` — the block's tile cut by the edge
    /// of a `width`×`height` desktop — for the page to put there.
    ///
    /// Throws for a block that does not decode, or that paints none of the
    /// rectangle, after which the tile holds what it held before.
    #[allow(clippy::too_many_arguments)]
    pub fn progressive(
        &mut self,
        width: u32,
        height: u32,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        block: &[u8],
    ) -> Result<Vec<u8>, JsError> {
        if self.size != (width, height) {
            self.decoder = Default::default();
            self.size = (width, height);
        }
        self.frame = self.frame.wrapping_add(1);
        let stream = wlshare_proto::progressive::stream(self.frame, &[(x, y, w, h)], &[block]);
        let (stride, mut out, mut painted) = (
            usize::from(w) * 4,
            vec![0u8; usize::from(w) * usize::from(h) * 4],
            false,
        );
        self.decoder.start_frame();
        self.decoder
            .decompress(1, width, height, &stream, usize::MAX, |rect, rows, from| {
                painted = true;
                let cols = usize::from(rect.right - rect.left);
                for (row, y_at) in (rect.top..rect.bottom).enumerate() {
                    let to = usize::from(y_at - y) * stride + usize::from(rect.left - x) * 4;
                    for (px, src) in out[to..to + cols * 4]
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(rows[row * from..].as_chunks::<4>().0)
                    {
                        px.copy_from_slice(&[src[0], src[1], src[2], 255]);
                    }
                }
            })
            .map_err(|e| JsError::new(&format!("a progressive tile at {x},{y}: {e}")))?;
        if !painted {
            return Err(JsError::new(&format!(
                "a progressive tile at {x},{y} painted nothing"
            )));
        }
        Ok(out)
    }

    /// The pixels of a batch's lossless tiles, in their order: each tile's `R, G,
    /// B, A` rows of its rectangle, for the page to put there, one tile after
    /// another. Tile `i` is `sizes[2i]`×`sizes[2i + 1]` and its body the next
    /// `lens[i]` bytes of `bodies`. Where `stills[i]` is 0 the body is one zstd
    /// frame of a planar bitmap of exactly that rectangle, which depends on
    /// nothing before it; otherwise it is the still stream's bytes of one, read
    /// after the bodies before it, and a body that begins a zstd frame starts the
    /// stream over.
    ///
    /// The still bodies are read in order, beside the moving ones' frames, which
    /// are decoded side by side on the module's pool, each held to what a planar
    /// bitmap of its rectangle can be ([`planar_most`]); then every tile's
    /// bitmap is decoded, side by side too. Throws for the first tile that does
    /// not decode, or whose zstd holds more than that, which is not read past; a
    /// still one loses the stream until a body starts it over.
    pub fn lossless(
        &mut self,
        stills: &[u8],
        sizes: &[u16],
        lens: &[u32],
        bodies: &[u8],
    ) -> Result<Vec<u8>, JsError> {
        let tiles = lossless_tiles(stills, sizes, lens, bodies)
            .map_err(|e| JsError::new(&format!("a batch of lossless tiles: {e}")))?;
        self.decode_lossless(&tiles).map_err(|e| JsError::new(&e))
    }

    /// [`Self::lossless`]'s tiles, laid out.
    fn decode_lossless(&mut self, tiles: &[Tile<'_>]) -> Result<Vec<u8>, String> {
        let mut rgba = vec![0u8; tiles.iter().map(Tile::rgba_len).sum()];
        zdesk_rdp_graphics::pool::install(|| {
            let read = self.read_lossless(tiles);
            let mut outs = Vec::with_capacity(tiles.len());
            let mut rest = &mut rgba[..];
            for tile in tiles {
                let (out, after) = rest.split_at_mut(tile.rgba_len());
                outs.push(out);
                rest = after;
            }
            let drawn: Vec<Result<(), String>> = tiles
                .par_iter()
                .zip(&self.bitmaps[..tiles.len()])
                .zip(outs)
                .zip(read)
                .map(|(((tile, bitmap), out), read)| read.and_then(|()| tile.draw(bitmap, out)))
                .collect();
            for (tile, drawn) in tiles.iter().zip(drawn) {
                if let Err(e) = drawn {
                    if tile.still {
                        self.in_still = false;
                    }
                    return Err(tile.failed(&e));
                }
            }
            Ok(())
        })?;
        Ok(rgba)
    }

    /// Each of `tiles` read out of its zstd into the bitmap of the same index:
    /// the still bodies in order, beside the moving ones' frames side by side.
    fn read_lossless(&mut self, tiles: &[Tile<'_>]) -> Vec<Result<(), String>> {
        let Self {
            bitmaps,
            moving,
            still,
            in_still,
            ..
        } = self;
        if bitmaps.len() < tiles.len() {
            bitmaps.resize_with(tiles.len(), Vec::new);
        }
        let (stills, movings): (Vec<_>, Vec<_>) = tiles
            .iter()
            .zip(bitmaps.iter_mut())
            .enumerate()
            .partition(|(_, (tile, _))| tile.still);
        let (stills, movings) = rayon::join(
            || {
                stills
                    .into_iter()
                    .map(|(i, (tile, bitmap))| (i, read_still(still, in_still, tile, bitmap)))
                    .collect::<Vec<_>>()
            },
            || read_moving(moving, movings),
        );
        let mut read: Vec<Result<(), String>> = (0..tiles.len()).map(|_| Ok(())).collect();
        for (i, result) in stills.into_iter().chain(movings) {
            read[i] = result;
        }
        read
    }
}

/// One of a batch's lossless tiles: a `w`×`h` rectangle's `body`, one of the
/// still stream's or a moving one's frame.
struct Tile<'a> {
    still: bool,
    w: u16,
    h: u16,
    body: &'a [u8],
}

impl Tile<'_> {
    /// How many bytes its `R, G, B, A` rows take.
    fn rgba_len(&self) -> usize {
        usize::from(self.w) * usize::from(self.h) * 4
    }

    fn failed(&self, e: &str) -> String {
        let kind = if self.still { "still " } else { "" };
        format!("a {}x{} {kind}lossless tile: {e}", self.w, self.h)
    }

    /// Its planar `bitmap` decoded into `out` as `R, G, B, A` rows, with this
    /// thread's working space.
    fn draw(&self, bitmap: &[u8], out: &mut [u8]) -> Result<(), String> {
        thread_local! {
            static PLANAR: RefCell<(Vec<u8>, Vec<u8>)> = const { RefCell::new((Vec::new(), Vec::new())) };
        }
        PLANAR.with_borrow_mut(|(planes, pixels)| {
            zdesk_rdp_graphics::proto::planar::decompress(bitmap, planes, pixels, self.w, self.h)
                .map_err(|e| e.to_string())?;
            for (to, px) in out
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(pixels.as_chunks::<4>().0)
            {
                *to = [px[2], px[1], px[0], 255];
            }
            Ok(())
        })
    }
}

/// The tiles [`WlshareTiles::lossless`] is given, each with its body.
fn lossless_tiles<'a>(
    stills: &[u8],
    sizes: &[u16],
    lens: &[u32],
    mut bodies: &'a [u8],
) -> Result<Vec<Tile<'a>>, String> {
    if sizes.len() != 2 * stills.len() || lens.len() != stills.len() {
        return Err(format!(
            "{} kinds, {} sizes and {} lengths",
            stills.len(),
            sizes.len(),
            lens.len()
        ));
    }
    let tiles = stills
        .iter()
        .zip(sizes.as_chunks::<2>().0)
        .zip(lens)
        .map(|((&still, &[w, h]), &len)| {
            let (body, rest) = bodies
                .split_at_checked(len as usize)
                .ok_or("bodies cut short")?;
            bodies = rest;
            Ok(Tile {
                still: still != 0,
                w,
                h,
                body,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if !bodies.is_empty() {
        return Err(format!("{} bytes past the bodies", bodies.len()));
    }
    Ok(tiles)
}

/// The moving tiles' frames, each `(index, tile, bitmap)`, decoded side by side
/// by `decoder`'s settings into their bitmaps, each held to what a planar bitmap
/// of its rectangle can be: one result for each, with its index.
fn read_moving(
    decoder: &ruzstd_decompress::decoding::FrameDecoder,
    tiles: Vec<(usize, (&Tile<'_>, &mut Vec<u8>))>,
) -> Vec<(usize, Result<(), String>)> {
    let mut results = Vec::with_capacity(tiles.len());
    let (mut inputs, mut outputs, mut taken) = (Vec::new(), Vec::new(), Vec::new());
    for (i, (tile, bitmap)) in tiles {
        bitmap.clear();
        let most = planar_most(tile.w, tile.h);
        let bound = one_frame(tile.body).and_then(|()| {
            ruzstd_decompress::decoding::decompressed_bound(tile.body).map_err(|e| e.to_string())
        });
        match bound {
            Err(e) => results.push((i, Err(e))),
            Ok(bound) if bound > most as u64 => results.push((
                i,
                Err(format!(
                    "a zstd frame of more than the {most} bytes a planar bitmap of it can be"
                )),
            )),
            // A frame decodes to no more than its bound, which is its room.
            Ok(bound) => {
                let mut output = std::mem::take(bitmap);
                output.reserve(bound as usize);
                inputs.push(tile.body);
                outputs.push(output);
                taken.push((i, bitmap));
            }
        }
    }
    let decoded = decoder.decode_each_parallel(&inputs, &mut outputs);
    for (((i, bitmap), output), decoded) in taken.into_iter().zip(outputs).zip(decoded) {
        *bitmap = output;
        results.push((i, decoded.map_err(|e| e.to_string())));
    }
    results
}

/// Whether `body` is one zstd frame and nothing after it, as a moving tile's body
/// is: the decoders read every frame of an input, and skip skippable ones.
fn one_frame(body: &[u8]) -> Result<(), String> {
    if !body.starts_with(&ZSTD_MAGIC) {
        return Err("a body that begins no zstd frame".into());
    }
    let len =
        ruzstd_decompress::decoding::frame_compressed_size(body).map_err(|e| e.to_string())?;
    if len != body.len() {
        return Err(format!("{} bytes past its zstd frame", body.len() - len));
    }
    Ok(())
}

/// `tile`'s body read on in the still stream `frame` into `bitmap`: a block at a
/// time, the frame held to [`STILL_WINDOW_MOST`] and the body to what a planar
/// bitmap of the rectangle can be, the bitmap what the body's blocks decode to,
/// which the frame hands out as soon as they are decoded. A body that begins a
/// zstd frame starts the stream over. `in_still` is whether `frame` holds a
/// stream: not before a body begins one, nor after a body that did not read,
/// which loses it.
fn read_still(
    frame: &mut ruzstd_decompress::decoding::FrameDecoder,
    in_still: &mut bool,
    tile: &Tile<'_>,
    bitmap: &mut Vec<u8>,
) -> Result<(), String> {
    bitmap.clear();
    let read = read_still_blocks(frame, in_still, tile, bitmap);
    if read.is_err() {
        *in_still = false;
    }
    read
}

fn read_still_blocks(
    frame: &mut ruzstd_decompress::decoding::FrameDecoder,
    in_still: &mut bool,
    tile: &Tile<'_>,
    bitmap: &mut Vec<u8>,
) -> Result<(), String> {
    let mut source = tile.body;
    if source.starts_with(&ZSTD_MAGIC) {
        *in_still = false;
        frame.init(&mut source).map_err(|e| e.to_string())?;
        *in_still = true;
    }
    if !*in_still {
        return Err("a body that continues no stream".into());
    }
    // The bodies before collected all theirs, so all there is to collect is this one's.
    let most = planar_most(tile.w, tile.h);
    while !source.is_empty() {
        let header = source.get(..3).ok_or("a block header cut short")?;
        let header = u32::from_le_bytes([header[0], header[1], header[2], 0]);
        if header & 1 != 0 {
            return Err("a last block, which ends the stream's frame".into());
        }
        // An RLE block's content is its one byte.
        let content = if (header >> 1) & 3 == 1 {
            1
        } else {
            (header >> 3) as usize
        };
        let block = source.get(..3 + content).ok_or("a block cut short")?;
        let (read, _) = frame
            .decode_from_to(block, &mut [])
            .map_err(|e| e.to_string())?;
        if read != block.len() {
            return Err("a block that does not decode".into());
        }
        source = &source[block.len()..];
        if frame.can_collect() > most {
            return Err(format!(
                "more than the {most} bytes a planar bitmap of it can be"
            ));
        }
    }
    frame.collect_to_writer(bitmap).map_err(|e| e.to_string())?;
    Ok(())
}

/// What a zstd frame begins with, and so a STILL that starts the stream over.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The largest window a still stream may have: wlshare's promise of 8 MiB.
const STILL_WINDOW_MOST: u64 = 8 << 20;

/// The most bytes a planar bitmap of a `w`×`h` rectangle can be: its format
/// header, four planes — the alpha plane the codec reads past among them — of
/// at most two bytes a pixel, a run-length coded row's worst being a segment
/// to each pixel, a control byte and the pixel's byte, and the padding byte
/// the plain form ends with.
fn planar_most(w: u16, h: u16) -> usize {
    1 + 4 * 2 * usize::from(w) * usize::from(h) + 1
}

impl Default for WlshareTiles {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw block of `content`, its frame's last or not.
    fn raw_block(content: &[u8], last: bool) -> Vec<u8> {
        let header = ((content.len() as u32) << 3) | u32::from(last);
        let mut block = header.to_le_bytes()[..3].to_vec();
        block.extend_from_slice(content);
        block
    }

    /// A lossless tile's body: a frame of `content` alone, its size given in a byte.
    fn tile(content: &[u8]) -> Vec<u8> {
        let mut body = ZSTD_MAGIC.to_vec();
        body.extend([0x20, u8::try_from(content.len()).unwrap()]);
        body.extend(raw_block(content, true));
        body
    }

    /// The body that begins a still stream, of a window of `1 << window_log`
    /// bytes, with `content`'s block.
    fn still_start(window_log: u8, content: &[u8]) -> Vec<u8> {
        let mut body = ZSTD_MAGIC.to_vec();
        body.extend([0x00, (window_log - 10) << 3]);
        body.extend(raw_block(content, false));
        body
    }

    fn bytes(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn moving<'a>(w: u16, h: u16, body: &'a [u8]) -> Tile<'a> {
        Tile {
            still: false,
            w,
            h,
            body,
        }
    }

    fn still<'a>(w: u16, h: u16, body: &'a [u8]) -> Tile<'a> {
        Tile {
            still: true,
            ..moving(w, h, body)
        }
    }

    /// A batch's moving tiles are each read as a decoder of its own would, one
    /// that does not decode, or holds more than a planar bitmap of its
    /// rectangle can be, failing alone; and again in the next batch.
    #[test]
    fn a_batch_reads_every_moving_tile() {
        let mut tiles = WlshareTiles::new();
        let contents: Vec<Vec<u8>> = [(1, 200), (2, 17), (3, 255)]
            .into_iter()
            .map(|(seed, len)| bytes(seed, len))
            .collect();
        let bodies: Vec<Vec<u8>> = contents.iter().map(|c| tile(c)).collect();
        let cut = tile(&bytes(4, 100));
        let mut past = tile(&bytes(5, 100));
        past.push(0);
        // 8x2's planar bitmap is 130 bytes at most.
        let large = tile(&bytes(6, 131));
        // A body is one frame: neither a skippable frame nor another after
        // it, nor a skippable frame before it, which the decoders would read.
        let skippable = [&0x184D_2A50u32.to_le_bytes()[..], &1u32.to_le_bytes(), &[0]].concat();
        let skipped = [&bodies[1][..], &skippable].concat();
        let two = [&bodies[1][..], &bodies[1]].concat();
        let after_skip = [&skippable[..], &bodies[1]].concat();
        let batch = [
            moving(8, 8, &bodies[0]),
            moving(8, 8, &cut[..cut.len() - 10]),
            moving(8, 8, &bodies[1]),
            moving(8, 8, &past),
            moving(8, 2, &large),
            moving(8, 8, &skipped),
            moving(8, 8, &two),
            moving(8, 8, &after_skip),
            moving(8, 8, &bodies[2]),
        ];
        let read = tiles.read_lossless(&batch);
        let failed: Vec<bool> = read.iter().map(Result::is_err).collect();
        assert_eq!(
            failed,
            [false, true, false, true, true, true, true, true, false]
        );
        for (at, content) in [(0, &contents[0]), (2, &contents[1]), (8, &contents[2])] {
            assert_eq!(&tiles.bitmaps[at], content);
        }
        let content = bytes(7, 90);
        let body = tile(&content);
        assert!(tiles.read_lossless(&[moving(8, 8, &body)])[0].is_ok());
        assert_eq!(tiles.bitmaps[0], content);
    }

    /// A body that begins a still stream starts it over in the same decoder,
    /// which a body that does not decode leaves holding no stream, whatever
    /// moving tiles are read beside them.
    #[test]
    fn a_still_stream_starts_over_in_its_decoder() {
        let mut tiles = WlshareTiles::new();
        let between = tile(&bytes(99, 40));
        for seed in [10, 20] {
            let (first, next) = (bytes(seed, 120), bytes(seed + 1, 60));
            let (start, more) = (still_start(20, &first), raw_block(&next, false));
            let read = tiles.read_lossless(&[
                still(8, 8, &start),
                moving(8, 8, &between),
                still(8, 8, &more),
            ]);
            assert!(read.iter().all(Result::is_ok));
            assert_eq!(tiles.bitmaps[0], first);
            assert_eq!(tiles.bitmaps[1], bytes(99, 40));
            assert_eq!(tiles.bitmaps[2], next);
            // A last block, which would end the stream's frame, loses it.
            let last = raw_block(&next, true);
            let read = tiles.read_lossless(&[still(8, 8, &last), still(8, 8, &more)]);
            assert!(read.iter().all(Result::is_err));
        }
        // The window is still held to wlshare's promise.
        let wide = still_start(24, &bytes(30, 10));
        assert!(tiles.read_lossless(&[still(8, 8, &wide)])[0].is_err());
        let content = bytes(40, 50);
        let start = still_start(23, &content);
        assert!(tiles.read_lossless(&[still(8, 8, &start)])[0].is_ok());
        assert_eq!(tiles.bitmaps[0], content);
    }

    /// A planar bitmap in its plain form, no alpha plane: its red, green and
    /// blue planes as they are, then the padding byte.
    fn plain_bitmap(planes: [&[u8]; 3]) -> Vec<u8> {
        let mut bitmap = vec![0x20];
        for plane in planes {
            bitmap.extend_from_slice(plane);
        }
        bitmap.push(0);
        bitmap
    }

    /// A batch's tiles come back as their pixels in their order, laid out as
    /// the page hands them in; a tile that does not decode fails the batch.
    #[test]
    fn a_batch_comes_back_as_its_pixels() {
        let mut tiles = WlshareTiles::new();
        let first = tile(&plain_bitmap([&[1, 2], &[3, 4], &[5, 6]]));
        let second = still_start(20, &plain_bitmap([&[7], &[8], &[9]]));
        let bodies = [first.as_slice(), second.as_slice()].concat();
        let batch = lossless_tiles(
            &[0, 1],
            &[2, 1, 1, 1],
            &[first.len() as u32, second.len() as u32],
            &bodies,
        )
        .unwrap();
        assert_eq!(
            tiles.decode_lossless(&batch).unwrap(),
            [1, 3, 5, 255, 2, 4, 6, 255, 7, 8, 9, 255]
        );
        assert!(lossless_tiles(&[0], &[2, 1], &[first.len() as u32 + 1], &first).is_err());
        assert!(lossless_tiles(&[0], &[2, 1], &[1], &first).is_err());
        assert!(lossless_tiles(&[0, 0], &[2, 1], &[1], &first).is_err());
        // A bitmap that is not one of its rectangle.
        let short = tile(&plain_bitmap([&[1], &[2], &[3]]));
        let err = tiles.decode_lossless(&[moving(2, 1, &short)]).unwrap_err();
        assert!(err.starts_with("a 2x1 lossless tile:"), "{err}");
    }
}
