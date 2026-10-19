//! `tzf` — a reader for **Trimble X‑series `.tzf`** terrestrial‑laser‑scan files.
//!
//! The format here is documented purely from the **observable byte structure**
//! of `.tzf` files (see `docs/format.md`); this project contains no vendor code
//! and is not affiliated with the manufacturer.
//!
//! A scan's data region is a sequence of **blocks**. Each block is a 60‑byte
//! descriptor followed by its payload; blocks tile the scan grid in 512×512
//! tiles, with several **components** per tile (on sample files: components
//! 1–3 are raw‑DEFLATE 512×512×4 payloads, component 4 is a 512×512 JPEG).
//!
//! The file is memory‑mapped and [`Tzf::blocks`] is a lazy, allocation‑free
//! iterator; payloads are only decompressed when you ask for one.
//!
//! STATUS: **work in progress.** Header, metadata, block walk, tile placement
//! and block decode are implemented. The meaning of components 1–3, the range
//! scale and spherical→XYZ are still being worked out, so [`Tzf::points`]
//! returns [`Error::Unimplemented`].

use memmap2::Mmap;
use std::fs::File;
use std::path::Path;
use thiserror::Error;

/// File magic (first 8 bytes), as observed.
pub const MAGIC: [u8; 8] = [0x5e, 0xde, 0x73, 0x9b, 0xdb, 0x7e, 0x79, 0xab];

/// Byte positions of the `u64` section offsets in the file header.
pub const SECTION_POS: [usize; 7] = [0x10, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50];

/// Offset of the first block descriptor.
pub const FIRST_DESCRIPTOR: usize = 0x5c;

/// Size of a block descriptor, in bytes.
pub const DESCRIPTOR_LEN: usize = 60;

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
    #[error("block at 0x{offset:x} is {kind:?}, not decodable as deflate")]
    WrongKind { offset: usize, kind: BlockKind },
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Parsed file header.
#[derive(Debug, Clone)]
pub struct Header {
    pub magic: [u8; 8],
    pub word_08: u32,
    pub word_0c: u32,
    /// `u64` section offsets (see [`SECTION_POS`]); the first is the scan
    /// metadata block, which also marks the end of the block data region.
    pub sections: Vec<u64>,
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Header> {
        let end = *SECTION_POS.last().unwrap() + 8;
        if data.len() < end {
            return Err(Error::Truncated {
                need: end,
                have: data.len(),
            });
        }
        if data[..8] != MAGIC {
            return Err(Error::BadMagic);
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&data[..8]);
        let sections = SECTION_POS.iter().map(|&p| rd_u64(data, p)).collect();
        Ok(Header {
            magic,
            word_08: rd_u32(data, 0x08),
            word_0c: rd_u32(data, 0x0c),
            sections,
        })
    }
    /// Offset of the scan metadata block (= end of the block data region).
    pub fn meta_offset(&self) -> u64 {
        self.sections[0]
    }
}

/// Scan calibration from the metadata block.
#[derive(Debug, Clone)]
pub struct ScanMeta {
    pub grid_rows: u32,
    pub grid_cols: u32,
    pub tile_size: u32,
    pub n_points: u32,
    /// The two per-axis angular steps (radians). On samples they are nearly
    /// equal and `step × grid_cols ≈ 2π` (one full revolution).
    pub angle_step_rad: [f64; 2],
    /// Horizontal angle of the first grid column (radians, in `[0, 2π)`).
    /// Stored negated in the file.
    pub start_ha_rad: f64,
    /// Vertical angle of the first grid row (radians from the zenith).
    pub start_va_rad: f64,
}

impl ScanMeta {
    /// Horizontal angle (radians, `[0, 2π)`) of grid column `col`.
    pub fn ha_of_col(&self, col: f64) -> f64 {
        (self.start_ha_rad + col * self.angle_step_rad[0]).rem_euclid(std::f64::consts::TAU)
    }

    /// Vertical angle (radians from the zenith) of grid row `row`.
    pub fn va_of_row(&self, row: f64) -> f64 {
        self.start_va_rad + row * self.angle_step_rad[1]
    }
}

/// How a block's payload is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// `[u32 raw size][0x78 0x01][raw DEFLATE]` — decode with [`Tzf::decode`].
    Deflate,
    /// A baseline JPEG (`FF D8 FF …`) — read with [`Tzf::payload`].
    Jpeg,
    /// Anything else (not seen on samples).
    Unknown,
}

/// One block, located from its descriptor. Cheap to copy; holds no payload.
#[derive(Debug, Clone, Copy)]
pub struct Block {
    /// Offset of the descriptor.
    pub descriptor: usize,
    /// Payload byte range `[payload_start, payload_end)`.
    pub payload_start: usize,
    pub payload_end: usize,
    /// Component number within a tile (1..=4 on samples).
    pub component: u32,
    /// Tile position in the scan grid (each tile is `width × height` cells).
    pub tile_row: u32,
    pub tile_col: u32,
    pub width: u32,
    pub height: u32,
    /// Declared decoded size in bytes (e.g. 512×512×4, or 512×512 for JPEG).
    pub raw_size: u32,
    /// Per-block `f32` from the descriptor (role unconfirmed).
    pub scale: f32,
    pub kind: BlockKind,
}

enum Storage {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

/// An open `.tzf` file (memory-mapped).
pub struct Tzf {
    storage: Storage,
    header: Header,
}

impl Tzf {
    /// Memory-map and open a file. Nothing is decompressed up front.
    pub fn open(path: &Path) -> Result<Tzf> {
        let file = File::open(path)?;
        // SAFETY: the mapping is read-only; mutating the file while it is
        // open is undefined behaviour, as with any mmap-based reader.
        let map = unsafe { Mmap::map(&file)? };
        let header = Header::parse(&map)?;
        Ok(Tzf {
            storage: Storage::Mapped(map),
            header,
        })
    }

    /// Open from an in-memory buffer (tests, embedded data).
    pub fn from_bytes(data: Vec<u8>) -> Result<Tzf> {
        let header = Header::parse(&data)?;
        Ok(Tzf {
            storage: Storage::Owned(data),
            header,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.storage {
            Storage::Mapped(m) => m,
            Storage::Owned(v) => v,
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Parse the scan metadata block.
    pub fn scan_meta(&self) -> Result<ScanMeta> {
        let d = self.bytes();
        let m = self.header.meta_offset() as usize;
        if m + 0x40 > d.len() {
            return Err(Error::Truncated {
                need: m + 0x40,
                have: d.len(),
            });
        }
        Ok(ScanMeta {
            angle_step_rad: [rd_f64(d, m + 0x10), rd_f64(d, m + 0x18)],
            start_va_rad: rd_f64(d, m + 0x20),
            start_ha_rad: (-rd_f64(d, m + 0x28)).rem_euclid(std::f64::consts::TAU),
            grid_rows: rd_u32(d, m + 0x30),
            grid_cols: rd_u32(d, m + 0x34),
            tile_size: rd_u32(d, m + 0x38),
            n_points: rd_u32(d, m + 0x3c),
        })
    }

    /// Lazily walk every block in the data region `[FIRST_DESCRIPTOR,
    /// meta_offset)`. Deterministic: each descriptor gives its payload length,
    /// and the next descriptor starts where the payload ends. If a descriptor
    /// fails validation the iterator resynchronises by scanning forward for the
    /// next structurally valid one. Allocates nothing.
    pub fn blocks(&self) -> Blocks<'_> {
        let d = self.bytes();
        let end = (self.header.meta_offset() as usize).min(d.len());
        Blocks {
            d,
            pos: FIRST_DESCRIPTOR,
            end,
        }
    }

    /// Raw payload bytes of a block (e.g. the JPEG for a `Jpeg` block).
    pub fn payload(&self, b: &Block) -> &[u8] {
        &self.bytes()[b.payload_start..b.payload_end]
    }

    /// Decode a `Deflate` block into `out` (reused across calls to avoid
    /// reallocating). On success `out.len() == b.raw_size`.
    pub fn decode_into(&self, b: &Block, out: &mut Vec<u8>) -> Result<()> {
        if b.kind != BlockKind::Deflate {
            return Err(Error::WrongKind {
                offset: b.descriptor,
                kind: b.kind,
            });
        }
        // payload = [u32 raw size][0x78 0x01][raw deflate ...]
        let body = &self.bytes()[b.payload_start + 6..b.payload_end];
        out.clear();
        out.resize(b.raw_size as usize, 0);
        let mut dec = flate2::Decompress::new(false);
        let st = dec
            .decompress(body, out, flate2::FlushDecompress::Finish)
            .map_err(|e| Error::Decompress(e.to_string()))?;
        if st != flate2::Status::StreamEnd || dec.total_out() as usize != out.len() {
            return Err(Error::Decompress(format!(
                "block at 0x{:x}: inflated {} of {} bytes",
                b.descriptor,
                dec.total_out(),
                out.len()
            )));
        }
        Ok(())
    }

    /// Decode a `Deflate` block into a fresh buffer.
    pub fn decode(&self, b: &Block) -> Result<Vec<u8>> {
        let mut v = Vec::new();
        self.decode_into(b, &mut v)?;
        Ok(v)
    }

    /// Decode all points. TODO: component meaning + range scale +
    /// spherical→XYZ + leveling. See `docs/format.md`.
    pub fn points(&self) -> Result<Vec<Point>> {
        Err(Error::Unimplemented(
            "points: component meaning + placement + geometry (WIP)",
        ))
    }
}

/// A decoded point (metres, scanner frame) with intensity.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub intensity: f32,
}

/// Lazy block iterator returned by [`Tzf::blocks`].
pub struct Blocks<'a> {
    d: &'a [u8],
    pos: usize, // offset of the next descriptor
    end: usize, // end of the data region
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block;
    fn next(&mut self) -> Option<Block> {
        while self.pos + DESCRIPTOR_LEN <= self.end {
            if let Some(b) = parse_block(self.d, self.pos, self.end) {
                self.pos = b.payload_end;
                return Some(b);
            }
            // Resync: scan forward for the next structurally valid descriptor.
            self.pos += 1;
        }
        None
    }
}

/// Validate and parse the descriptor at `at`. Structural checks only (no magic
/// constants): plausible component and tile indices, sane dimensions, raw size
/// consistent with the dimensions, and a payload that fits in the data region.
fn parse_block(d: &[u8], at: usize, end: usize) -> Option<Block> {
    if at + DESCRIPTOR_LEN > end {
        return None;
    }
    let comp = rd_u32(d, at + 16) as usize;
    let component = rd_u32(d, at + 20);
    let (tile_row, tile_col) = (rd_u32(d, at + 24), rd_u32(d, at + 28));
    let (w, h, raw) = (rd_u32(d, at + 36), rd_u32(d, at + 40), rd_u32(d, at + 44));
    if !(1..=8).contains(&component) || tile_row >= 4096 || tile_col >= 4096 {
        return None;
    }
    if !(1..=8192).contains(&w) || !(1..=8192).contains(&h) {
        return None;
    }
    let px = w as u64 * h as u64;
    if ![1u64, 2, 3, 4, 8].iter().any(|bpp| px * bpp == raw as u64) {
        return None;
    }
    if comp < 28 {
        return None;
    }
    let payload_start = at + DESCRIPTOR_LEN;
    let payload_end = payload_start + (comp - 28);
    if payload_end > end || payload_end <= payload_start + 2 {
        return None;
    }
    let p = &d[payload_start..payload_end];
    let kind = if p.len() >= 6 && p[4] == 0x78 && p[5] == 0x01 {
        BlockKind::Deflate
    } else if p.len() >= 3 && p[..3] == [0xff, 0xd8, 0xff] {
        BlockKind::Jpeg
    } else {
        BlockKind::Unknown
    };
    Some(Block {
        descriptor: at,
        payload_start,
        payload_end,
        component,
        tile_row,
        tile_col,
        width: w,
        height: h,
        raw_size: raw,
        scale: f32::from_bits(rd_u32(d, at + 48)),
        kind,
    })
}

fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn rd_u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}
fn rd_f64(d: &[u8], o: usize) -> f64 {
    f64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_non_tzf() {
        assert!(matches!(Header::parse(&[0u8; 128]), Err(Error::BadMagic)));
    }

    fn deflate_raw(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// Append one block: 60-byte descriptor + payload.
    fn push_block(
        buf: &mut Vec<u8>,
        component: u32,
        tile: (u32, u32),
        w: u32,
        h: u32,
        raw: u32,
        payload: &[u8],
    ) {
        let comp = (payload.len() + 28) as u32;
        buf.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes()); // id
        for v in [200u32, 16, comp, component, tile.0, tile.1, 100, w, h, raw] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&1.0f32.to_le_bytes()); // scale
        buf.extend_from_slice(&0u64.to_le_bytes()); // hash
        buf.extend_from_slice(payload);
    }

    fn synthetic() -> (Vec<u8>, Vec<u8>) {
        let mut buf = vec![0u8; FIRST_DESCRIPTOR];
        buf[..8].copy_from_slice(&MAGIC);
        let raw_len = 64 * 64 * 4;
        let zeros = vec![0u8; raw_len];
        let pattern: Vec<u8> = (0..raw_len).map(|i| (i % 251) as u8).collect();
        for (c, tile, data) in [(1u32, (0u32, 0u32), &zeros), (2, (1, 3), &pattern)] {
            let mut p = (raw_len as u32).to_le_bytes().to_vec();
            p.extend_from_slice(&[0x78, 0x01]);
            p.extend_from_slice(&deflate_raw(data));
            push_block(&mut buf, c, tile, 64, 64, raw_len as u32, &p);
        }
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0xff, 0xd9,
        ];
        push_block(&mut buf, 4, (1, 3), 64, 64, 64 * 64, &jpeg);
        let data_end = buf.len() as u64;
        buf[0x10..0x18].copy_from_slice(&data_end.to_le_bytes());
        buf.extend_from_slice(&[0u8; 0x40]); // metadata block
        (buf, pattern)
    }

    #[test]
    fn walks_and_decodes_blocks() {
        let (buf, pattern) = synthetic();
        let t = Tzf::from_bytes(buf).unwrap();
        let bs: Vec<Block> = t.blocks().collect();
        assert_eq!(bs.len(), 3);
        assert_eq!(
            bs.iter().map(|b| b.component).collect::<Vec<_>>(),
            vec![1, 2, 4]
        );
        assert_eq!(bs[0].kind, BlockKind::Deflate);
        assert_eq!(bs[2].kind, BlockKind::Jpeg);
        assert_eq!((bs[0].tile_row, bs[0].tile_col), (0, 0));
        assert_eq!((bs[1].tile_row, bs[1].tile_col), (1, 3));
        assert!(t.decode(&bs[0]).unwrap().iter().all(|&b| b == 0));
        assert_eq!(t.decode(&bs[1]).unwrap(), pattern);
        let j = t.payload(&bs[2]);
        assert_eq!(&j[..2], &[0xff, 0xd8]);
        assert_eq!(&j[j.len() - 2..], &[0xff, 0xd9]);
        assert!(matches!(t.decode(&bs[2]), Err(Error::WrongKind { .. })));
    }

    #[test]
    fn scan_meta_angles() {
        let (mut buf, _) = synthetic();
        let m = u64::from_le_bytes(buf[0x10..0x18].try_into().unwrap()) as usize;
        let step = 1.5e-3_f64;
        for (off, v) in [(0x10, step), (0x18, step), (0x20, 1.2), (0x28, -5.0)] {
            buf[m + off..m + off + 8].copy_from_slice(&f64::to_le_bytes(v));
        }
        buf[m + 0x30..m + 0x34].copy_from_slice(&100u32.to_le_bytes());
        buf[m + 0x34..m + 0x38].copy_from_slice(&200u32.to_le_bytes());
        let meta = Tzf::from_bytes(buf).unwrap().scan_meta().unwrap();
        assert_eq!((meta.grid_rows, meta.grid_cols), (100, 200));
        assert!((meta.start_ha_rad - 5.0).abs() < 1e-12);
        assert!((meta.va_of_row(10.0) - (1.2 + 10.0 * step)).abs() < 1e-12);
        // 5.0 + 1000 * 1.5e-3 = 6.5 rad wraps past 2π
        let wrapped = 6.5 - std::f64::consts::TAU;
        assert!((meta.ha_of_col(1000.0) - wrapped).abs() < 1e-12);
    }

    #[test]
    fn resyncs_past_garbage() {
        let (mut buf, _) = synthetic();
        // corrupt the first descriptor's width: walker should resync past it
        buf[FIRST_DESCRIPTOR + 36..FIRST_DESCRIPTOR + 40].copy_from_slice(&0u32.to_le_bytes());
        let t = Tzf::from_bytes(buf).unwrap();
        let comps: Vec<u32> = t.blocks().map(|b| b.component).collect();
        assert_eq!(comps, vec![2, 4]);
    }
}
