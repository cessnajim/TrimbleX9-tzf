//! `trimble_tzf` — a reader for **Trimble `.tzf`** laser‑scan point‑cloud files.
//!
//! The format here is documented purely from the **observable byte structure**
//! of `.tzf` files (see `docs/format.md`); this project contains no vendor code
//! and is not affiliated with the manufacturer.
//!
//! A scan's data region is a sequence of **blocks**. Each block is a 60‑byte
//! descriptor followed by its payload; blocks tile the scan grid in 512×512
//! tiles, with several **components** per tile (on sample files: components
//! 1–3 are 512×512×4 raw‑DEFLATE or raw‑Snappy payloads, component 4 is a
//! 512×512 JPEG).
//!
//! The file is memory‑mapped and [`Tzf::blocks`] is a lazy, allocation‑free
//! iterator; payloads are only decompressed when you ask for one.
//!
//! Components 1–3 of each tile are range, vertical angle and horizontal angle
//! (component 4, when present, is a JPEG intensity image);
//! [`Tzf::points`] turns them into levelled XYZ (see `docs/format.md`).
//! Payloads are raw DEFLATE or raw Snappy blocks. Validated on X9 scans
//! recorded for this project (`docs/PROVENANCE.md`).

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
    /// [`Tzf::decode`] was asked for a block that is not DEFLATE or Snappy.
    #[error("block at 0x{offset:x} is {kind:?}, not a DEFLATE or Snappy block")]
    WrongKind { offset: usize, kind: BlockKind },
    /// The blocks are individually readable but do not fit together: a missing
    /// or duplicated component, or planes whose sizes disagree.
    #[error("malformed scan: {0}")]
    Malformed(String),
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
    /// Levelling rotation stored at metadata offset 0x44 (row-major 3×3 f64).
    /// Points are levelled with `p' = tiltᵀ · p`. Identity when the stored
    /// matrix is absent or not a proper rotation; [`ScanMeta::levelling`]
    /// says which.
    pub tilt: [[f64; 3]; 3],
    /// Whether `tilt` is the stored rotation or an identity stand-in.
    pub levelling: Levelling,
}

/// Where [`ScanMeta::tilt`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Levelling {
    /// The stored rotation is used: points are levelled.
    Applied,
    /// The metadata block ends before the rotation: identity used, points are
    /// **not** levelled.
    Missing,
    /// The stored matrix is not a proper rotation (orthonormal, det +1, to
    /// 1e-6): identity used, points are **not** levelled.
    NotARotation,
}

impl Levelling {
    pub fn is_applied(self) -> bool {
        self == Levelling::Applied
    }
}

const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Accept a 3×3 matrix only if it is a proper rotation (orthonormal, det +1).
fn as_rotation(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    for i in 0..3 {
        for j in 0..3 {
            let dot: f64 = (0..3).map(|k| m[i][k] * m[j][k]).sum();
            let want = if i == j { 1.0 } else { 0.0 };
            if !dot.is_finite() || (dot - want).abs() > 1e-6 {
                return None;
            }
        }
    }
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    ((det - 1.0).abs() < 1e-6).then_some(m)
}

impl ScanMeta {
    /// Instrument horizontal angle (radians, `[0, 2π)`) of grid column `col`,
    /// as used by area-scan commands. This is **not** the azimuth of the
    /// decoded points: see [`ScanMeta::azimuth_of_col`].
    pub fn ha_of_col(&self, col: f64) -> f64 {
        (self.start_ha_rad + col * self.angle_step_rad[0]).rem_euclid(std::f64::consts::TAU)
    }

    /// Azimuth `atan2(y, x)` (radians, `[0, 2π)`) that decoded points of grid
    /// column `col` have, before levelling: `2π − ha_of_col(col)`. The file
    /// stores the horizontal angle negated and [`Tzf::points`] uses it as
    /// stored, so x/y azimuth runs the opposite way to the instrument's HA.
    /// Levelling tilts points by up to the tilt angle (a few hundredths of a
    /// radian on samples), so levelled points match only approximately.
    pub fn azimuth_of_col(&self, col: f64) -> f64 {
        ha_to_azimuth(self.ha_of_col(col))
    }

    /// Vertical angle (radians from the zenith) of grid row `row`.
    pub fn va_of_row(&self, row: f64) -> f64 {
        self.start_va_rad + row * self.angle_step_rad[1]
    }
}

/// Convert an instrument horizontal angle to the x/y azimuth of decoded points
/// (and back: the map is its own inverse): `(2π − a) mod 2π`.
pub fn ha_to_azimuth(a: f64) -> f64 {
    (-a).rem_euclid(std::f64::consts::TAU)
}

/// Component number of the range plane (see [`Tzf::points`]).
pub const COMPONENT_RANGE: u32 = 1;
/// Component number of the vertical-angle plane.
pub const COMPONENT_VERTICAL_ANGLE: u32 = 2;
/// Component number of the horizontal-angle plane.
pub const COMPONENT_HORIZONTAL_ANGLE: u32 = 3;

/// How a block's payload is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// `[u32 raw size][0x78 0x01][raw DEFLATE]` — decode with [`Tzf::decode`].
    Deflate,
    /// A raw Snappy block (public block format, no framing) whose declared
    /// length equals the raw size — decode with [`Tzf::decode`].
    Snappy,
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
        let m = usize::try_from(self.header.meta_offset()).unwrap_or(usize::MAX);
        let need = m.saturating_add(0x40);
        if need > d.len() {
            return Err(Error::Truncated {
                need,
                have: d.len(),
            });
        }
        let (tilt, levelling) = if m + 0x44 + 72 <= d.len() {
            let mut t = [[0.0; 3]; 3];
            for (k, v) in t.iter_mut().flatten().enumerate() {
                *v = rd_f64(d, m + 0x44 + 8 * k);
            }
            match as_rotation(t) {
                Some(r) => (r, Levelling::Applied),
                None => (IDENTITY, Levelling::NotARotation),
            }
        } else {
            (IDENTITY, Levelling::Missing)
        };
        Ok(ScanMeta {
            angle_step_rad: [rd_f64(d, m + 0x10), rd_f64(d, m + 0x18)],
            start_va_rad: rd_f64(d, m + 0x20),
            start_ha_rad: (-rd_f64(d, m + 0x28)).rem_euclid(std::f64::consts::TAU),
            grid_rows: rd_u32(d, m + 0x30),
            grid_cols: rd_u32(d, m + 0x34),
            tile_size: rd_u32(d, m + 0x38),
            n_points: rd_u32(d, m + 0x3c),
            tilt,
            levelling,
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

    /// Decode a `Deflate` or `Snappy` block into `out` (reused across calls to
    /// avoid reallocating). On success `out.len() == b.raw_size`.
    ///
    /// The output buffer is sized from the declared raw size only after
    /// checking the payload could expand that far, so a corrupt descriptor
    /// cannot force a large allocation from a small payload.
    pub fn decode_into(&self, b: &Block, out: &mut Vec<u8>) -> Result<()> {
        let max_ratio = match b.kind {
            BlockKind::Deflate => DEFLATE_MAX_RATIO,
            BlockKind::Snappy => SNAPPY_MAX_RATIO,
            _ => 0,
        };
        let payload_len = b.payload_end - b.payload_start;
        if max_ratio > 0 && b.raw_size as usize > payload_len.saturating_mul(max_ratio) {
            return Err(Error::Decompress(format!(
                "block at 0x{:x}: {} payload bytes cannot expand to the declared {}",
                b.descriptor, payload_len, b.raw_size
            )));
        }
        if b.kind == BlockKind::Snappy {
            return snappy_decompress_into(self.payload(b), b.raw_size as usize, out).map_err(|_| {
                Error::Decompress(format!("snappy block at 0x{:x} did not decode", b.descriptor))
            });
        }
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

    /// Decode a `Deflate` or `Snappy` block into a fresh buffer.
    pub fn decode(&self, b: &Block) -> Result<Vec<u8>> {
        let mut v = Vec::new();
        self.decode_into(b, &mut v)?;
        Ok(v)
    }

    /// Decode all valid points (metres, levelled scanner frame: +z is up).
    /// `intensity` is 0: the intensity image (component 4, a JPEG per tile,
    /// present only in some scans) is not decoded yet; the scanner's preview
    /// image is grid-aligned and can supply it.
    pub fn points(&self) -> Result<Vec<Point>> {
        Ok(self.grid_points()?.into_iter().map(|g| g.point).collect())
    }

    /// Like [`Tzf::points`], but each point also carries its grid cell.
    ///
    /// Per tile, components 1/2/3 hold range / vertical angle / horizontal
    /// angle. Cells are stored one vertical scan line at a time: cell
    /// `line × height + k` is grid column `tile_col × tile + line`, row
    /// `tile_row × tile + k`. Angle planes hold µrad: the first cell of each
    /// line is absolute, the rest are `i32` deltas. Range = value / scale / 1000 m.
    ///
    /// Structural problems are errors, never panics: a tile missing one of
    /// the three components or holding two blocks for one, planes that are not
    /// 4 bytes per cell, planes of one tile that differ in size, or a block
    /// larger than the metadata tile size all return [`Error::Malformed`].
    /// The output is sized from the cells actually present, not the declared
    /// point count.
    pub fn grid_points(&self) -> Result<Vec<GridPoint>> {
        use std::collections::BTreeMap;
        let meta = self.scan_meta()?;
        let (rows, cols, tile) = (meta.grid_rows as usize, meta.grid_cols as usize, meta.tile_size as usize);
        let mut tiles: BTreeMap<(u32, u32), [Option<Block>; 3]> = BTreeMap::new();
        for b in self.blocks() {
            let slot = match b.component {
                COMPONENT_RANGE => 0,
                COMPONENT_VERTICAL_ANGLE => 1,
                COMPONENT_HORIZONTAL_ANGLE => 2,
                _ => continue,
            };
            let entry = &mut tiles.entry((b.tile_row, b.tile_col)).or_default()[slot];
            if let Some(prev) = entry {
                return Err(Error::Malformed(format!(
                    "tile ({},{}) has two component {} blocks (0x{:x} and 0x{:x})",
                    b.tile_row, b.tile_col, b.component, prev.descriptor, b.descriptor
                )));
            }
            *entry = Some(b);
        }
        if tiles.is_empty() {
            return Err(Error::Malformed("no range/angle blocks found".into()));
        }
        let mut cells = 0u64;
        let mut checked = Vec::with_capacity(tiles.len());
        for (&(tr, tc), p) in &tiles {
            let [Some(r), Some(v), Some(h)] = *p else {
                return Err(Error::Malformed(format!("tile ({tr},{tc}) is missing a component")));
            };
            for b in [&r, &v, &h] {
                let n = b.width as u64 * b.height as u64;
                if b.raw_size as u64 != n * 4 {
                    return Err(Error::Malformed(format!(
                        "block at 0x{:x}: {}x{} cells but {} bytes, not 4 per cell",
                        b.descriptor, b.width, b.height, b.raw_size
                    )));
                }
                if (b.width, b.height) != (r.width, r.height) {
                    return Err(Error::Malformed(format!(
                        "tile ({tr},{tc}): component {} is {}x{}, range is {}x{}",
                        b.component, b.width, b.height, r.width, r.height
                    )));
                }
                if b.width as usize > tile || b.height as usize > tile {
                    return Err(Error::Malformed(format!(
                        "block at 0x{:x} is {}x{}, larger than the {tile}-cell tile",
                        b.descriptor, b.width, b.height
                    )));
                }
            }
            cells += r.width as u64 * r.height as u64;
            checked.push(((tr, tc), [r, v, h]));
        }
        let cap = cells
            .min(meta.n_points as u64)
            .min(meta.grid_rows as u64 * meta.grid_cols as u64);
        let t = meta.tilt;
        let mut out = Vec::with_capacity(cap as usize);
        let (mut rb, mut vb, mut hb) = (Vec::new(), Vec::new(), Vec::new());
        for ((tr, tc), [r, v, h]) in checked {
            self.decode_into(&r, &mut rb)?;
            self.decode_into(&v, &mut vb)?;
            self.decode_into(&h, &mut hb)?;
            // decode_into yields exactly raw_size = width × height × 4 bytes.
            let (lines, per_line) = (r.width as usize, r.height as usize);
            let cell = |buf: &[u8], i: usize| rd_u32(buf, i * 4);
            for line in 0..lines {
                let col = tc as usize * tile + line;
                if col >= cols {
                    break;
                }
                let (mut va_u, mut ha_u) = (0i64, 0i64);
                for k in 0..per_line {
                    let i = line * per_line + k;
                    // Integrate before the validity checks so the running angle
                    // stays correct across empty cells.
                    va_u += cell(&vb, i) as i32 as i64;
                    ha_u += cell(&hb, i) as i32 as i64;
                    let row = tr as usize * tile + k;
                    let raw = cell(&rb, i);
                    if row >= rows || raw == 0 || raw == u32::MAX {
                        continue;
                    }
                    let range = raw as f64 / r.scale as f64 / 1000.0;
                    let (va, ha) = (va_u as f64 * 1e-6, ha_u as f64 * 1e-6);
                    let hz = range * va.sin();
                    let (x, y, z) = (hz * ha.cos(), hz * ha.sin(), range * va.cos());
                    // p' = tiltᵀ · p
                    out.push(GridPoint {
                        point: Point {
                            x: (t[0][0] * x + t[1][0] * y + t[2][0] * z) as f32,
                            y: (t[0][1] * x + t[1][1] * y + t[2][1] * z) as f32,
                            z: (t[0][2] * x + t[1][2] * y + t[2][2] * z) as f32,
                            intensity: 0.0,
                        },
                        row: row as u32,
                        col: col as u32,
                    });
                }
            }
        }
        out.sort_by_key(|g| (g.col, g.row));
        Ok(out)
    }
}

/// A point plus the grid cell it came from (`col` = horizontal, `row` = vertical).
#[derive(Debug, Clone, Copy)]
pub struct GridPoint {
    pub point: Point,
    pub row: u32,
    pub col: u32,
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
    } else if snappy_declared_len(p) == Some(raw as usize) {
        BlockKind::Snappy
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

/// The uncompressed length a raw Snappy block declares (its leading varint).
fn snappy_declared_len(buf: &[u8]) -> Option<usize> {
    let (mut len, mut shift) = (0usize, 0);
    for &b in buf.iter().take(5) {
        len |= ((b & 0x7f) as usize) << shift;
        if b < 0x80 {
            return Some(len);
        }
        shift += 7;
    }
    None
}

/// Decompress a raw Snappy block (public block format: a varint length, then
/// literal / copy elements) into `out`; succeed only if it declares and yields
/// exactly `expect` bytes.
fn snappy_decompress_into(buf: &[u8], expect: usize, out: &mut Vec<u8>) -> std::result::Result<(), ()> {
    let byte = |p: usize| buf.get(p).copied().ok_or(());
    let le = |p: usize, n: usize| -> std::result::Result<usize, ()> {
        let b = buf.get(p..p + n).ok_or(())?;
        Ok(b.iter().rev().fold(0usize, |a, &x| a << 8 | x as usize))
    };
    let (mut len, mut pos, mut shift) = (0usize, 0usize, 0);
    loop {
        let b = byte(pos)?;
        pos += 1;
        len |= ((b & 0x7f) as usize) << shift;
        if b < 0x80 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return Err(());
        }
    }
    if len != expect {
        return Err(());
    }
    out.clear();
    out.reserve(len);
    while out.len() < len {
        let tag = byte(pos)?;
        pos += 1;
        let (n, off) = match tag & 3 {
            0 => {
                let mut n = (tag >> 2) as usize;
                if n >= 60 {
                    let w = n - 59;
                    n = le(pos, w)?;
                    pos += w;
                }
                let lit = buf.get(pos..pos + n + 1).ok_or(())?;
                if out.len() + lit.len() > len {
                    return Err(());
                }
                out.extend_from_slice(lit);
                pos += n + 1;
                continue;
            }
            1 => {
                let o = ((tag as usize >> 5) << 8) | byte(pos)? as usize;
                pos += 1;
                (((tag >> 2) & 7) as usize + 4, o)
            }
            2 => {
                pos += 2;
                ((tag >> 2) as usize + 1, le(pos - 2, 2)?)
            }
            _ => {
                pos += 4;
                ((tag >> 2) as usize + 1, le(pos - 4, 4)?)
            }
        };
        if off == 0 || off > out.len() || out.len() + n > len {
            return Err(());
        }
        let start = out.len() - off;
        for k in 0..n {
            let v = out[start + k]; // copies may overlap their own output
            out.push(v);
        }
    }
    Ok(())
}

/// Upper bounds on how far a payload can expand, used to reject impossible
/// declared sizes before allocating. Raw DEFLATE tops out near 1032:1; a
/// Snappy copy element of 3 bytes yields at most 64.
const DEFLATE_MAX_RATIO: usize = 1100;
const SNAPPY_MAX_RATIO: usize = 22;

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
    fn snappy_literals_and_overlapping_copy() {
        // len 7; literal "ab"; copy-1 (len 5, offset 2) -> "abababa".
        let buf = [7u8, 0x04, b'a', b'b', 1 << 2 | 1, 2];
        let mut out = Vec::new();
        snappy_decompress_into(&buf, 7, &mut out).unwrap();
        assert_eq!(&out, b"abababa");
        assert!(snappy_decompress_into(&buf, 8, &mut out).is_err());
        assert_eq!(snappy_declared_len(&[0x80, 0x80, 0x40]), Some(1 << 20));
    }

    #[test]
    fn tilt_guard_accepts_rotations_only() {
        let (c, sn) = (0.02f64.cos(), 0.02f64.sin());
        assert!(as_rotation([[c, 0.0, sn], [0.0, 1.0, 0.0], [-sn, 0.0, c]]).is_some());
        assert!(as_rotation([[0.0; 3]; 3]).is_none());
        assert!(as_rotation([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
    }

    /// One plane block for [`plane_file`]: component, tile, width, height,
    /// descriptor scale, decoded bytes.
    type Plane = (u32, (u32, u32), u32, u32, f32, Vec<u8>);

    fn cells(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|c| c.to_le_bytes()).collect()
    }

    /// Build a file from DEFLATE plane blocks plus a metadata block with the
    /// given grid (rows, cols, tile size, point count) and an all-zero tilt.
    /// Returns the file and the metadata offset.
    fn plane_file(planes: &[Plane], grid: (u32, u32, u32, u32)) -> (Vec<u8>, usize) {
        let mut buf = vec![0u8; FIRST_DESCRIPTOR];
        buf[..8].copy_from_slice(&MAGIC);
        for (comp, tile, w, h, scale, raw) in planes {
            let mut p = (raw.len() as u32).to_le_bytes().to_vec();
            p.extend_from_slice(&[0x78, 0x01]);
            p.extend_from_slice(&deflate_raw(raw));
            push_block(&mut buf, *comp, *tile, *w, *h, raw.len() as u32, &p);
            let at = buf.len() - p.len() - DESCRIPTOR_LEN;
            buf[at + 48..at + 52].copy_from_slice(&scale.to_le_bytes());
        }
        let m = buf.len();
        buf[0x10..0x18].copy_from_slice(&(m as u64).to_le_bytes());
        buf.extend_from_slice(&[0u8; 0x44 + 72]);
        let (rows, cols, tile, n) = grid;
        for (off, v) in [(0x30, rows), (0x34, cols), (0x38, tile), (0x3c, n)] {
            buf[m + off..m + off + 4].copy_from_slice(&v.to_le_bytes());
        }
        (buf, m)
    }

    /// One 2x2 tile: two vertical lines at HA 0 and 90°, both rows at VA 90°
    /// (horizontal), range 1 m and 2 m.
    fn two_line_planes() -> Vec<Plane> {
        let q = 1_570_796u32; // π/2 in µrad
        vec![
            (COMPONENT_RANGE, (0, 0), 2, 2, 10.0, cells(&[10_000, 20_000, 10_000, 20_000])),
            (COMPONENT_VERTICAL_ANGLE, (0, 0), 2, 2, 1.0, cells(&[q, 0, q, 0])),
            (COMPONENT_HORIZONTAL_ANGLE, (0, 0), 2, 2, 1.0, cells(&[0, 0, q, 0])),
        ]
    }

    /// Expect the two-line tile's points on +x and +y.
    #[test]
    fn decodes_points_from_planes() {
        let (buf, _) = plane_file(&two_line_planes(), (2, 2, 2, 4));
        let pts = Tzf::from_bytes(buf).unwrap().grid_points().unwrap();
        assert_eq!(pts.len(), 4);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let p00 = pts[0].point; // col 0, row 0: 1 m along +x
        assert!(near(p00.x, 1.0) && near(p00.y, 0.0) && near(p00.z, 0.0));
        let p11 = pts[3].point; // col 1, row 1: 2 m along +y
        assert!(near(p11.x, 0.0) && near(p11.y, 2.0) && near(p11.z, 0.0));
    }

    fn malformed(planes: &[Plane]) -> String {
        let (buf, _) = plane_file(planes, (2, 2, 2, 4));
        match Tzf::from_bytes(buf).unwrap().grid_points() {
            Err(Error::Malformed(m)) => m,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn rejects_plane_not_4_bytes_per_cell() {
        let mut planes = two_line_planes();
        planes[0].5.truncate(8); // 2x2 cells in 8 bytes: 2 per cell
        assert!(malformed(&planes).contains("not 4 per cell"));
        let mut planes = two_line_planes();
        planes[2].5.truncate(8);
        assert!(malformed(&planes).contains("not 4 per cell"));
    }

    #[test]
    fn rejects_planes_of_different_sizes() {
        let mut planes = two_line_planes();
        planes[1].3 = 1; // vertical-angle plane 2x1, 4 bytes per cell
        planes[1].5.truncate(8);
        assert!(malformed(&planes).contains("range is 2x2"));
    }

    #[test]
    fn rejects_duplicate_and_missing_components() {
        let mut planes = two_line_planes();
        planes.push(planes[0].clone());
        assert!(malformed(&planes).contains("two component 1 blocks"));
        let mut planes = two_line_planes();
        planes.remove(1);
        assert!(malformed(&planes).contains("missing a component"));
    }

    #[test]
    fn rejects_block_larger_than_tile() {
        let (buf, m) = plane_file(&two_line_planes(), (2, 2, 2, 4));
        let mut buf = buf;
        buf[m + 0x38..m + 0x3c].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            Tzf::from_bytes(buf).unwrap().grid_points(),
            Err(Error::Malformed(_))
        ));
    }

    /// A declared point count of u32::MAX must not drive the allocation.
    #[test]
    fn ignores_huge_declared_point_count() {
        let (buf, _) = plane_file(&two_line_planes(), (u32::MAX, u32::MAX, 2, u32::MAX));
        let pts = Tzf::from_bytes(buf).unwrap().grid_points().unwrap();
        assert_eq!(pts.len(), 4);
    }

    /// A descriptor claiming more raw bytes than its payload could ever expand
    /// to is rejected before the output buffer is sized.
    #[test]
    fn rejects_impossible_raw_size() {
        let mut buf = vec![0u8; FIRST_DESCRIPTOR];
        buf[..8].copy_from_slice(&MAGIC);
        let raw = 8192 * 8192 * 4u32;
        let mut p = raw.to_le_bytes().to_vec();
        p.extend_from_slice(&[0x78, 0x01, 0x03, 0x00]); // empty deflate stream
        push_block(&mut buf, 1, (0, 0), 8192, 8192, raw, &p);
        let end = buf.len() as u64;
        buf[0x10..0x18].copy_from_slice(&end.to_le_bytes());
        buf.extend_from_slice(&[0u8; 0x40]);
        let t = Tzf::from_bytes(buf).unwrap();
        let b = t.blocks().next().unwrap();
        assert_eq!(b.kind, BlockKind::Deflate);
        let mut out = Vec::new();
        assert!(matches!(t.decode_into(&b, &mut out), Err(Error::Decompress(_))));
        assert_eq!(out.capacity(), 0);
    }

    /// Every truncation and every single-byte corruption of a valid file must
    /// give `Ok` or `Err`, never a panic.
    #[test]
    fn truncated_or_corrupted_files_do_not_panic() {
        let (buf, m) = plane_file(&two_line_planes(), (2, 2, 2, 4));
        let try_all = |b: Vec<u8>| {
            if let Ok(t) = Tzf::from_bytes(b) {
                let _ = t.scan_meta();
                let _ = t.grid_points();
                for blk in t.blocks() {
                    let _ = t.decode(&blk);
                }
            }
        };
        for len in 0..buf.len() {
            try_all(buf[..len].to_vec());
        }
        for i in 0..buf.len() {
            for x in [0xff, 0x80, 0x01] {
                let mut b = buf.clone();
                b[i] ^= x;
                try_all(b);
            }
        }
        // Truncated inside the plane data: the walker stops early and the
        // tiles come up short.
        let mut b = buf[..m - 10].to_vec();
        b[0x10..0x18].copy_from_slice(&((m - 10) as u64).to_le_bytes());
        b.extend_from_slice(&buf[m..]);
        assert!(Tzf::from_bytes(b).unwrap().grid_points().is_err());
    }

    #[test]
    fn reports_whether_levelling_was_applied() {
        let (mut buf, m) = plane_file(&two_line_planes(), (2, 2, 2, 4));
        let meta = |b: &[u8]| Tzf::from_bytes(b.to_vec()).unwrap().scan_meta().unwrap();
        assert_eq!(meta(&buf).levelling, Levelling::NotARotation); // all zeros
        for (k, v) in IDENTITY.iter().flatten().enumerate() {
            let o = m + 0x44 + 8 * k;
            buf[o..o + 8].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(meta(&buf).levelling, Levelling::Applied);
        assert!(meta(&buf).levelling.is_applied());
        assert_eq!(meta(&buf[..m + 0x44]).levelling, Levelling::Missing);
    }

    /// The decoded points' azimuth atan2(y, x) is 2π − HA, where HA is the
    /// instrument horizontal angle of the column (`ScanMeta::ha_of_col`).
    #[test]
    fn point_azimuth_is_two_pi_minus_ha() {
        // Four columns from HA 1.0 rad in 0.5 rad steps. Like the metadata
        // start angle, the plane stores each line's HA negated (µrad, the
        // first cell absolute, the rest deltas).
        let (start, step) = (1.0f64, 0.5f64);
        let q = 1_570_796u32;
        let ha = |c: u32| -((start + c as f64 * step) * 1e6).round() as i32 as u32;
        let planes: Vec<Plane> = vec![
            (COMPONENT_RANGE, (0, 0), 4, 1, 1.0, cells(&[1000; 4])),
            (COMPONENT_VERTICAL_ANGLE, (0, 0), 4, 1, 1.0, cells(&[q; 4])),
            (COMPONENT_HORIZONTAL_ANGLE, (0, 0), 4, 1, 1.0, cells(&[ha(0), ha(1), ha(2), ha(3)])),
        ];
        let (mut buf, m) = plane_file(&planes, (1, 4, 4, 4));
        for (off, v) in [(0x10, step), (0x18, step), (0x28, -start)] {
            buf[m + off..m + off + 8].copy_from_slice(&v.to_le_bytes());
        }
        let t = Tzf::from_bytes(buf).unwrap();
        let meta = t.scan_meta().unwrap();
        let pts = t.grid_points().unwrap();
        assert_eq!(pts.len(), 4);
        for g in pts {
            let az = (g.point.y as f64).atan2(g.point.x as f64).rem_euclid(std::f64::consts::TAU);
            let ha = meta.ha_of_col(g.col as f64);
            assert!((az - (std::f64::consts::TAU - ha)).abs() < 1e-5, "col {}", g.col);
            assert!((az - meta.azimuth_of_col(g.col as f64)).abs() < 1e-5);
            assert!((ha_to_azimuth(az) - ha).abs() < 1e-5);
        }
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
