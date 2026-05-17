use std::io::{BufWriter, ErrorKind};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use jpeg_encoder::{ColorType, Encoder as JpegEnc};
use parking_lot::Mutex;
use scrap::{Capturer, Display};
use xxhash_rust::xxh3::xxh3_64;

use crate::protocol::{write_frame, write_handshake, FrameHeader};

struct PendingFrame {
    width: u32,
    height: u32,
    jpeg: Vec<u8>,
}

type Slot = Arc<Mutex<Option<PendingFrame>>>;

pub fn run_sender(
    bind: &str,
    fps: u32,
    quality: u8,
    skip_unchanged: bool,
    filter: Vec<usize>,
) -> Result<()> {
    let total_displays = Display::all().context("enumerate displays")?.len();
    anyhow::ensure!(total_displays > 0, "no displays detected");

    // An empty filter means "every display"; otherwise validate that each
    // requested index actually exists and use it as the capture list.
    let capture_indices: Vec<usize> = if filter.is_empty() {
        (0..total_displays).collect()
    } else {
        for &idx in &filter {
            anyhow::ensure!(
                idx < total_displays,
                "display index {idx} out of range (only {total_displays} detected)"
            );
        }
        filter
    };
    let n_monitors = capture_indices.len();
    anyhow::ensure!(n_monitors > 0, "no displays selected");
    anyhow::ensure!(n_monitors <= u8::MAX as usize, "too many displays");

    let listener = TcpListener::bind(bind).with_context(|| format!("bind {bind}"))?;
    println!(
        "[share] listening on {bind} | {n_monitors} monitor(s) (indices {capture_indices:?}) | fps={fps} quality={quality} skip_unchanged={skip_unchanged}"
    );

    loop {
        let (stream, peer) = listener.accept().context("accept")?;
        println!("[share] peer connected: {peer}");
        stream.set_nodelay(true).ok();
        if let Err(e) = serve_one(
            stream,
            &capture_indices,
            fps,
            quality,
            skip_unchanged,
        ) {
            eprintln!("[share] session ended: {e}");
        }
        println!("[share] waiting for next peer on {bind}");
    }
}

fn serve_one(
    stream: TcpStream,
    capture_indices: &[usize],
    fps: u32,
    quality: u8,
    skip_unchanged: bool,
) -> Result<()> {
    let n = capture_indices.len() as u8;
    let mut writer = BufWriter::with_capacity(1 << 20, stream);
    write_handshake(&mut writer, n)?;
    std::io::Write::flush(&mut writer)?;

    let slots: Vec<Slot> = (0..n).map(|_| Arc::new(Mutex::new(None))).collect();
    let (wake_tx, wake_rx) = bounded::<()>(1);

    let mut handles = Vec::with_capacity(n as usize);
    for (logical, &physical_idx) in capture_indices.iter().enumerate() {
        let logical = logical as u8;
        let slot = slots[logical as usize].clone();
        let wake = wake_tx.clone();
        let h = thread::Builder::new()
            .name(format!("capture-{logical}"))
            .spawn(move || {
                if let Err(e) = capture_loop(
                    logical,
                    physical_idx,
                    slot,
                    wake,
                    fps,
                    quality,
                    skip_unchanged,
                ) {
                    eprintln!("[share] capture {logical} (display #{physical_idx}) stopped: {e}");
                }
            })?;
        handles.push(h);
    }
    drop(wake_tx);

    let io_result = io_loop(&mut writer, wake_rx, &slots, n);

    // Drop our slot refs so capture threads' references are the only ones; they
    // detect a closed wake channel via try_send and exit cleanly.
    drop(slots);
    for h in handles {
        let _ = h.join();
    }
    io_result
}

fn io_loop(
    writer: &mut BufWriter<TcpStream>,
    wake_rx: Receiver<()>,
    slots: &[Slot],
    n: u8,
) -> Result<()> {
    let mut stats_last = Instant::now();
    let mut stats_frames = vec![0u32; n as usize];
    let mut stats_bytes = vec![0u64; n as usize];

    while wake_rx.recv().is_ok() {
        while wake_rx.try_recv().is_ok() {}

        for (i, slot) in slots.iter().enumerate() {
            let pf = slot.lock().take();
            if let Some(pf) = pf {
                let hdr = FrameHeader {
                    monitor_id: i as u8,
                    width: pf.width,
                    height: pf.height,
                    data_len: pf.jpeg.len() as u32,
                };
                write_frame(writer, &hdr, &pf.jpeg)?;
                stats_frames[i] += 1;
                stats_bytes[i] += pf.jpeg.len() as u64 + 13;
            }
        }
        std::io::Write::flush(writer)?;

        if stats_last.elapsed() >= Duration::from_secs(1) {
            let dt = stats_last.elapsed().as_secs_f64().max(1e-9);
            let mut any = false;
            let mut line = String::from("[share]");
            for i in 0..n as usize {
                if stats_frames[i] > 0 {
                    any = true;
                    line.push_str(&format!(
                        "  m{i}:{:>4.1}fps {:>6.1}KB/s",
                        stats_frames[i] as f64 / dt,
                        stats_bytes[i] as f64 / dt / 1024.0
                    ));
                }
                stats_frames[i] = 0;
                stats_bytes[i] = 0;
            }
            if any {
                println!("{line}");
            }
            stats_last = Instant::now();
        }
    }
    Ok(())
}

fn capture_loop(
    id: u8,
    physical_idx: usize,
    slot: Slot,
    wake: Sender<()>,
    fps: u32,
    quality: u8,
    skip_unchanged: bool,
) -> Result<()> {
    let display = Display::all()?
        .into_iter()
        .nth(physical_idx)
        .with_context(|| format!("display {physical_idx} is gone"))?;
    let mut capturer = Capturer::new(display)?;
    let width = capturer.width() as u32;
    let height = capturer.height() as u32;
    anyhow::ensure!(width <= u16::MAX as u32 && height <= u16::MAX as u32, "display {id} too large for JPEG (max 65535 per side)");
    let row_bytes = (width as usize) * 4;
    let frame_pixels = (width as usize) * (height as usize);
    let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);

    let mut last_hash: Option<u64> = None;
    let mut packed_bgra = vec![0u8; row_bytes * height as usize];
    // JPEG output buffer reused across frames; cleared by encoder via Vec::write.
    let mut jpeg_buf: Vec<u8> = Vec::with_capacity(frame_pixels / 4);

    loop {
        let started = Instant::now();

        // Pull a frame from the capturer; spin until one is ready or we time out.
        let got_frame = loop {
            match capturer.frame() {
                Ok(raw) => {
                    let stride = raw.len() / height as usize;
                    if stride == row_bytes {
                        packed_bgra.copy_from_slice(&raw[..row_bytes * height as usize]);
                    } else {
                        for y in 0..height as usize {
                            let src = &raw[y * stride..y * stride + row_bytes];
                            packed_bgra[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src);
                        }
                    }
                    break true;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if started.elapsed() >= frame_interval {
                        break false;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e.into()),
            }
        };

        if got_frame {
            let same = if skip_unchanged {
                let h = xxh3_64(&packed_bgra);
                let same = Some(h) == last_hash;
                last_hash = Some(h);
                same
            } else {
                false
            };

            if !same {
                jpeg_buf.clear();
                let enc = JpegEnc::new(&mut jpeg_buf, quality);
                enc.encode(&packed_bgra, width as u16, height as u16, ColorType::Bgra)?;

                let pending = PendingFrame {
                    width,
                    height,
                    jpeg: jpeg_buf.clone(),
                };
                *slot.lock() = Some(pending);

                match wake.try_send(()) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return Ok(()),
                }
            }
        }

        let elapsed = started.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }
}
