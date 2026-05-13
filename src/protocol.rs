use std::io::{Read, Write};

use anyhow::{ensure, Result};

// Wire format
// ===========
// Handshake (sharer -> viewer, once):
//     magic     : u32 LE = 0x50325053 ("P2PS")
//     version   : u8     = 1
//     monitors  : u8     = number of remote monitors that will be streamed
//
// Frame (sharer -> viewer, repeating):
//     monitor_id : u8     = 0..monitors-1
//     width      : u32 LE = source frame width in pixels
//     height     : u32 LE = source frame height in pixels
//     data_len   : u32 LE = JPEG payload length in bytes
//     data       : [u8; data_len] = JPEG-encoded frame (full intra frame)
//
// One TCP connection carries every monitor's stream multiplexed by monitor_id.

pub const MAGIC: u32 = 0x5350_3250; // "P2PS" little-endian when read as ASCII
pub const VERSION: u8 = 1;
pub const MAX_FRAME_BYTES: u32 = 32 * 1024 * 1024; // 32 MiB safety cap per JPEG

pub struct FrameHeader {
    pub monitor_id: u8,
    pub width: u32,
    pub height: u32,
    #[allow(dead_code)] // kept on the read side for protocol clarity; callers use the returned Vec's len
    pub data_len: u32,
}

pub fn write_handshake<W: Write>(w: &mut W, num_monitors: u8) -> Result<()> {
    w.write_all(&MAGIC.to_le_bytes())?;
    w.write_all(&[VERSION, num_monitors])?;
    Ok(())
}

pub fn read_handshake<R: Read>(r: &mut R) -> Result<u8> {
    let mut m = [0u8; 4];
    r.read_exact(&mut m)?;
    ensure!(u32::from_le_bytes(m) == MAGIC, "not a p2p-screenshare stream");
    let mut vb = [0u8; 2];
    r.read_exact(&mut vb)?;
    ensure!(vb[0] == VERSION, "unsupported protocol version {}", vb[0]);
    Ok(vb[1])
}

pub fn write_frame<W: Write>(w: &mut W, h: &FrameHeader, data: &[u8]) -> Result<()> {
    let mut hdr = [0u8; 1 + 4 + 4 + 4];
    hdr[0] = h.monitor_id;
    hdr[1..5].copy_from_slice(&h.width.to_le_bytes());
    hdr[5..9].copy_from_slice(&h.height.to_le_bytes());
    hdr[9..13].copy_from_slice(&(data.len() as u32).to_le_bytes());
    w.write_all(&hdr)?;
    w.write_all(data)?;
    Ok(())
}

pub fn read_frame<R: Read>(r: &mut R) -> Result<(FrameHeader, Vec<u8>)> {
    let mut hdr = [0u8; 1 + 4 + 4 + 4];
    r.read_exact(&mut hdr)?;
    let monitor_id = hdr[0];
    let width = u32::from_le_bytes(hdr[1..5].try_into().unwrap());
    let height = u32::from_le_bytes(hdr[5..9].try_into().unwrap());
    let data_len = u32::from_le_bytes(hdr[9..13].try_into().unwrap());
    ensure!(data_len <= MAX_FRAME_BYTES, "frame too large: {}", data_len);
    let mut data = vec![0u8; data_len as usize];
    r.read_exact(&mut data)?;
    Ok((
        FrameHeader { monitor_id, width, height, data_len },
        data,
    ))
}
