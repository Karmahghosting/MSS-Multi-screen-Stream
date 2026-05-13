use std::collections::HashMap;
use std::io::{BufReader, Cursor};
use std::net::TcpStream;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use winit::event::{ElementState, Event, KeyEvent, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowBuilder, WindowId};
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::protocol::read_handshake;

#[derive(Debug, Clone)]
enum UiMsg {
    Frame(u8),
    Stats,
    NetDown,
}

struct EncodedSlot {
    jpeg: Option<Vec<u8>>,
}

struct DecodedSlot {
    /// BGRA bytes reinterpreted as u32 XRGB; cheap to share/swap by `Arc::clone`.
    pixels: Arc<Vec<u32>>,
    w: u32,
    h: u32,
}

struct ScaleLut {
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    off_x: usize,
    off_y: usize,
    out_w: usize,
    out_h: usize,
    x_lut: Vec<u32>,
    y_lut: Vec<u32>,
}

impl ScaleLut {
    fn build(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Self {
        let scale = f64::min(dst_w as f64 / src_w as f64, dst_h as f64 / src_h as f64);
        let out_w = ((src_w as f64) * scale).round().max(1.0) as usize;
        let out_h = ((src_h as f64) * scale).round().max(1.0) as usize;
        let out_w = out_w.min(dst_w as usize);
        let out_h = out_h.min(dst_h as usize);
        let off_x = (dst_w as usize - out_w) / 2;
        let off_y = (dst_h as usize - out_h) / 2;
        let x_lut: Vec<u32> = (0..out_w)
            .map(|x| (x as u64 * src_w as u64 / out_w as u64) as u32)
            .collect();
        let y_lut: Vec<u32> = (0..out_h)
            .map(|y| (y as u64 * src_h as u64 / out_h as u64) as u32)
            .collect();
        ScaleLut { src_w, src_h, dst_w, dst_h, off_x, off_y, out_w, out_h, x_lut, y_lut }
    }
}

struct PerWindow {
    window: Arc<Window>,
    _context: softbuffer::Context<Arc<Window>>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    scale_lut: Option<ScaleLut>,
}

struct ReceiverStats {
    frames: Vec<u32>,
    bytes_enc: Vec<u64>,
    last: Instant,
}

pub fn run_receiver(addr: &str) -> Result<()> {
    let stream = TcpStream::connect(addr).with_context(|| format!("connect {addr}"))?;
    stream.set_nodelay(true).ok();
    let mut reader = BufReader::with_capacity(1 << 20, stream);
    let n = read_handshake(&mut reader)? as usize;
    println!("[view] sharer advertises {n} monitor(s)");
    anyhow::ensure!(n > 0, "sharer has no monitors");

    let event_loop = EventLoopBuilder::<UiMsg>::with_user_event().build()?;
    let local_monitors: Vec<_> = event_loop.available_monitors().collect();
    anyhow::ensure!(!local_monitors.is_empty(), "no local monitors");
    println!("[view] local monitors: {}", local_monitors.len());

    let mut windows: Vec<PerWindow> = Vec::with_capacity(n);
    let mut window_to_idx: HashMap<WindowId, usize> = HashMap::new();
    for i in 0..n {
        let monitor = local_monitors[i % local_monitors.len()].clone();
        let window = WindowBuilder::new()
            .with_title(format!("p2p-screenshare — remote screen {i}"))
            .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor))))
            .build(&event_loop)?;
        let window = Arc::new(window);
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;
        window_to_idx.insert(window.id(), i);
        windows.push(PerWindow { window, _context: context, surface, scale_lut: None });
    }

    let encoded: Arc<Vec<Mutex<EncodedSlot>>> =
        Arc::new((0..n).map(|_| Mutex::new(EncodedSlot { jpeg: None })).collect());
    let decoded: Arc<Vec<Mutex<DecodedSlot>>> = Arc::new(
        (0..n).map(|_| Mutex::new(DecodedSlot {
            pixels: Arc::new(Vec::new()),
            w: 0,
            h: 0,
        })).collect(),
    );

    let stats = Arc::new(Mutex::new(ReceiverStats {
        frames: vec![0; n],
        bytes_enc: vec![0; n],
        last: Instant::now(),
    }));

    // One decode worker per monitor.
    let decode_wakes: Vec<Sender<()>> = (0..n)
        .map(|i| spawn_decoder(i, encoded.clone(), decoded.clone(), stats.clone(), event_loop.create_proxy()))
        .collect();

    // Stats ticker thread.
    {
        let proxy = event_loop.create_proxy();
        thread::Builder::new().name("stats".into()).spawn(move || loop {
            thread::sleep(Duration::from_secs(1));
            if proxy.send_event(UiMsg::Stats).is_err() {
                return;
            }
        })?;
    }

    // Network thread: pump frames from the wire into per-monitor encoded slots.
    {
        let proxy = event_loop.create_proxy();
        let encoded = encoded.clone();
        let wakes = decode_wakes.clone();
        thread::Builder::new()
            .name("net".into())
            .spawn(move || {
                let mut reader = reader;
                loop {
                    match crate::protocol::read_frame(&mut reader) {
                        Ok((hdr, jpeg)) => {
                            let i = (hdr.monitor_id as usize) % encoded.len();
                            encoded[i].lock().jpeg = Some(jpeg);
                            let _ = wakes[i].try_send(());
                        }
                        Err(e) => {
                            eprintln!("[view] connection ended: {e}");
                            let _ = proxy.send_event(UiMsg::NetDown);
                            return;
                        }
                    }
                }
            })?;
    }

    event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::Wait);
        match event {
            Event::WindowEvent { window_id, event } => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::KeyboardInput {
                    event: KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                    ..
                } => elwt.exit(),
                WindowEvent::Resized(size) => {
                    if let Some(&i) = window_to_idx.get(&window_id) {
                        if let (Some(w), Some(h)) =
                            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                        {
                            let _ = windows[i].surface.resize(w, h);
                            windows[i].scale_lut = None;
                            windows[i].window.request_redraw();
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Some(&i) = window_to_idx.get(&window_id) {
                        present(&mut windows[i], &decoded[i]);
                    }
                }
                _ => {}
            },
            Event::UserEvent(UiMsg::Frame(i)) => {
                let i = i as usize;
                if let Some(pw) = windows.get(i) {
                    pw.window.request_redraw();
                }
            }
            Event::UserEvent(UiMsg::Stats) => {
                let mut s = stats.lock();
                if s.last.elapsed() >= Duration::from_secs(1) {
                    let dt = s.last.elapsed().as_secs_f64().max(1e-9);
                    let mut any = false;
                    let mut line = String::from("[view ]");
                    for i in 0..s.frames.len() {
                        if s.frames[i] > 0 {
                            any = true;
                            line.push_str(&format!(
                                "  m{i}:{:>4.1}fps {:>6.1}KB/s",
                                s.frames[i] as f64 / dt,
                                s.bytes_enc[i] as f64 / dt / 1024.0
                            ));
                        }
                        s.frames[i] = 0;
                        s.bytes_enc[i] = 0;
                    }
                    if any {
                        println!("{line}");
                    }
                    s.last = Instant::now();
                }
            }
            Event::UserEvent(UiMsg::NetDown) => elwt.exit(),
            _ => {}
        }
    })?;
    Ok(())
}

fn spawn_decoder(
    i: usize,
    encoded: Arc<Vec<Mutex<EncodedSlot>>>,
    decoded: Arc<Vec<Mutex<DecodedSlot>>>,
    stats: Arc<Mutex<ReceiverStats>>,
    proxy: winit::event_loop::EventLoopProxy<UiMsg>,
) -> Sender<()> {
    let (tx, rx) = bounded::<()>(1);
    thread::Builder::new()
        .name(format!("decode-{i}"))
        .spawn(move || {
            let mut bytes_buf: Vec<u8> = Vec::new();
            while rx.recv().is_ok() {
                while rx.try_recv().is_ok() {}
                let jpeg_opt = encoded[i].lock().jpeg.take();
                let Some(jpeg) = jpeg_opt else { continue };
                let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
                let mut dec = JpegDecoder::new_with_options(Cursor::new(&jpeg[..]), opts);
                if let Err(e) = dec.decode_headers() {
                    eprintln!("[view] m{i} decode headers: {e}");
                    continue;
                }
                let Some((w, h)) = dec.dimensions() else {
                    eprintln!("[view] m{i} no dims after headers");
                    continue;
                };
                let needed = dec.output_buffer_size().unwrap_or(w * h * 4);
                if bytes_buf.len() < needed {
                    bytes_buf.resize(needed, 0);
                }
                if let Err(e) = dec.decode_into(&mut bytes_buf[..needed]) {
                    eprintln!("[view] m{i} decode: {e}");
                    continue;
                }
                // Reinterpret BGRA bytes as a packed Vec<u32>; we copy into a fresh
                // Vec<u32> for clean alignment so softbuffer can slurp it directly.
                let px_count = w * h;
                let mut px: Vec<u32> = vec![0; px_count];
                {
                    let src: &[u32] =
                        bytemuck::cast_slice::<u8, u32>(&bytes_buf[..px_count * 4]);
                    px.copy_from_slice(src);
                }
                {
                    let mut d = decoded[i].lock();
                    d.pixels = Arc::new(px);
                    d.w = w as u32;
                    d.h = h as u32;
                }
                {
                    let mut s = stats.lock();
                    s.frames[i] += 1;
                    s.bytes_enc[i] += jpeg.len() as u64;
                }
                if proxy.send_event(UiMsg::Frame(i as u8)).is_err() {
                    return;
                }
            }
        })
        .expect("spawn decoder");
    tx
}

fn present(pw: &mut PerWindow, slot: &Mutex<DecodedSlot>) {
    let size = pw.window.inner_size();
    let (Some(dw), Some(dh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
        return;
    };
    if let Err(e) = pw.surface.resize(dw, dh) {
        eprintln!("[view] resize: {e}");
        return;
    }
    let mut buf = match pw.surface.buffer_mut() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[view] buffer_mut: {e}");
            return;
        }
    };

    let (pixels, src_w, src_h) = {
        let d = slot.lock();
        (d.pixels.clone(), d.w, d.h)
    };

    if pixels.is_empty() || src_w == 0 || src_h == 0 {
        buf.fill(0);
        let _ = buf.present();
        return;
    }

    let dst_w = dw.get();
    let dst_h = dh.get();

    if src_w == dst_w && src_h == dst_h {
        // 1:1 fast path — straight memcpy of pre-formatted pixels.
        buf.copy_from_slice(&pixels[..]);
    } else {
        let need_lut = match &pw.scale_lut {
            Some(lut) => {
                lut.src_w != src_w || lut.src_h != src_h || lut.dst_w != dst_w || lut.dst_h != dst_h
            }
            None => true,
        };
        if need_lut {
            pw.scale_lut = Some(ScaleLut::build(src_w, src_h, dst_w, dst_h));
        }
        let lut = pw.scale_lut.as_ref().unwrap();
        let dst_w_us = dst_w as usize;
        let src_w_us = src_w as usize;
        buf.fill(0);
        for y in 0..lut.out_h {
            let sy = lut.y_lut[y] as usize;
            let src_row = sy * src_w_us;
            let dst_row = (lut.off_y + y) * dst_w_us + lut.off_x;
            // Tight inner loop: index into precomputed x table, copy a u32.
            for x in 0..lut.out_w {
                let sx = lut.x_lut[x] as usize;
                buf[dst_row + x] = pixels[src_row + sx];
            }
        }
    }

    if let Err(e) = buf.present() {
        eprintln!("[view] present: {e}");
    }
}
