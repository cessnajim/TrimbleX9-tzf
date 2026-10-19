//! `tzf` — a reader for **Trimble X‑series `.tzf`** terrestrial‑laser‑scan files.
//!
//! The format here is documented purely from the **observable byte structure**
//! of `.tzf` files (see `docs/format.md`); this project contains no vendor code
//! and is not affiliated with the manufacturer.
//!
//! A scan is a spherical range‑image on a grid, split into 512×512 **patches**,
//! each independently compressed. Points are polar (range + two angles) and
//! convert to XYZ via the grid's angular step plus a leveling/tilt correction.
//!
//! STATUS: **work in progress.** Header + section‑table parsing is implemented
//! and reliable, and patch payloads decode (raw‑deflate → 512×512×4). The
//! per‑patch pixel/plane layout, range scale, patch placement, and
//! spherical→XYZ + tilt are still being worked out, so `points()` returns
//! [`Error::Unimplemented`] for now.

use thiserror::Error;

/// File magic (first 8 bytes), as observed.
pub const MAGIC: [u8; 8] = [0x5e, 0xde, 0x73, 0x9b, 0xdb, 0x7e, 0x79, 0xab];

/// Byte positions of the `u64` section offsets in the file header.
pub const SECTION_POS: [usize; 7] = [0x10, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50];

/// Uncompressed size of one patch payload: 512 × 512 × 4 bytes.
pub const PATCH_BYTES: usize = 512 * 512 * 4;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not a .tzf file: bad magic")]
    BadMagic,
    #[error("truncated: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("decompress: {0}")]
    Decompress(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Per‑patch compression, named for the observed transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Raw DEFLATE (a `0x78 0x01` marker prefixes the stream, but the body is
    /// raw deflate — a zlib read fails the trailing checksum).
    Deflate,
    /// Byte‑plane shuffle + delta, then DEFLATE (seen on some patches).
    ShuffleDeltaDeflate,
    /// Snappy.
    Snappy,
}

/// Parsed file header.
#[derive(Debug, Clone)]
pub struct Header {
    pub magic: [u8; 8],
    pub word_08: u32,
    pub word_0c: u32,
    /// `u64` section offsets (see [`SECTION_POS`]); the first is the scan
    /// metadata block, others (near EOF) hold directories and a preview image.
    pub sections: Vec<u64>,
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Header> {
        let end = *SECTION_POS.last().unwrap() + 8;
        if data.len() < end {
            return Err(Error::Truncated { need: end, have: data.len() });
        }
        if data[..8] != MAGIC {
            return Err(Error::BadMagic);
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&data[..8]);
        let sections = SECTION_POS.iter().map(|&p| rd_u64(data, p)).collect();
        Ok(Header { magic, word_08: rd_u32(data, 0x08), word_0c: rd_u32(data, 0x0c), sections })
    }
    /// Offset of the scan metadata block (first section).
    pub fn meta_offset(&self) -> u64 { self.sections[0] }
}

/// Scan calibration needed to turn the range‑image into XYZ. Field positions in
/// the metadata block are still being confirmed; observed on a sample: grid
/// ≈ 8330×3262, angular step ≈ 7.56e‑4 rad in both axes.
#[derive(Debug, Clone)]
pub struct ScanMeta {
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub step_h_rad: f64,
    pub step_v_rad: f64,
    pub n_points: u64,
    pub patch_size: u32,
}

/// A decoded point (metres, scanner frame) with intensity.
#[derive(Debug, Clone, Copy)]
pub struct Point { pub x: f32, pub y: f32, pub z: f32, pub intensity: f32 }

/// A located, decoded patch stream.
#[derive(Clone)]
pub struct PatchStream {
    /// File offset of the `0x78 0x01` marker.
    pub offset: usize,
    /// Compressed size (including the 2‑byte marker).
    pub comp_size: usize,
    /// The 1,048,576‑byte inflated payload.
    pub raw: Vec<u8>,
}

/// Open a `.tzf` file (reads it fully into memory) and parse the header.
pub struct Tzf { pub data: Vec<u8>, pub header: Header }

impl Tzf {
    pub fn open(path: &std::path::Path) -> Result<Tzf> {
        let data = std::fs::read(path)?;
        let header = Header::parse(&data)?;
        Ok(Tzf { data, header })
    }

    /// Parse the scan metadata block. TODO: confirm field offsets (see docs).
    pub fn scan_meta(&self) -> Result<ScanMeta> {
        Err(Error::Unimplemented("scan_meta: metadata-block field offsets unconfirmed"))
    }

    /// Walk the compressed-data region `[0x9c, sections[0])` and return every
    /// patch stream: `[descriptor][0x78 0x01][raw DEFLATE → 1,048,576 B]`,
    /// raw‑deflating from `offset+2`. Jumps by the consumed length after a hit.
    pub fn patch_streams(&self) -> Vec<PatchStream> {
        let end = (self.header.meta_offset() as usize).min(self.data.len()).saturating_sub(4);
        let mut out = Vec::new();
        let mut i = 0x9c;
        while i < end {
            if self.data[i] == 0x78 && self.data[i + 1] == 0x01 {
                if let Ok((raw, consumed)) = inflate_raw_exact(&self.data[i + 2..], PATCH_BYTES) {
                    out.push(PatchStream { offset: i, comp_size: consumed + 2, raw });
                    i += 2 + consumed;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Decode a single patch at a `0x78 0x01` offset to its 1 MiB payload.
    pub fn decode_patch(&self, offset: usize) -> Result<Vec<u8>> {
        let (raw, _) = inflate_raw_exact(&self.data[offset + 2..], PATCH_BYTES)
            .map_err(|_| Error::Decompress("patch did not inflate to 1 MiB".into()))?;
        Ok(raw)
    }

    /// Decode all points. TODO: per‑patch pixel/plane layout + placement +
    /// range scale + spherical→XYZ + tilt. See `docs/format.md`.
    pub fn points(&self) -> Result<Vec<Point>> {
        Err(Error::Unimplemented("points: pixel/plane layout + placement + geometry (WIP)"))
    }
}

/// Inflate a zlib stream (helper for the plain‑deflate case).
pub fn zlib_inflate(bytes: &[u8]) -> Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let mut out = Vec::new();
    ZlibDecoder::new(bytes).read_to_end(&mut out).map_err(|e| Error::Decompress(e.to_string()))?;
    Ok(out)
}

/// Raw‑DEFLATE `buf`, succeeding only if it yields exactly `expect` bytes.
/// Returns (payload, compressed_bytes_consumed).
fn inflate_raw_exact(buf: &[u8], expect: usize) -> std::result::Result<(Vec<u8>, usize), ()> {
    use flate2::{Decompress, FlushDecompress};
    let mut dec = Decompress::new(false); // raw deflate (no zlib header)
    let mut out = vec![0u8; expect];
    match dec.decompress(buf, &mut out, FlushDecompress::Finish) {
        Ok(flate2::Status::StreamEnd) if dec.total_out() as usize == expect => {
            Ok((out, dec.total_in() as usize))
        }
        _ => Err(()),
    }
}

fn rd_u32(d: &[u8], o: usize) -> u32 { u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) }
fn rd_u64(d: &[u8], o: usize) -> u64 { u64::from_le_bytes(d[o..o + 8].try_into().unwrap()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magic_len() { assert_eq!(MAGIC.len(), 8); }
    #[test]
    fn rejects_non_tzf() {
        assert!(matches!(Header::parse(&vec![0u8; 128]), Err(Error::BadMagic)));
    }
    #[test]
    fn parses_header_with_magic() {
        let mut d = vec![0u8; 0x60];
        d[..8].copy_from_slice(&MAGIC);
        d[0x10..0x18].copy_from_slice(&1234u64.to_le_bytes());
        let h = Header::parse(&d).unwrap();
        assert_eq!(h.sections[0], 1234);
    }
}
