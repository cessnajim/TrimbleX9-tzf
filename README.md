# trimble-tzf

A Rust library and command-line tools for reading **Trimble `.tzf`**
laser-scan files and turning them into point clouds. Validated on scans from
a Trimble X9.

- `tzfdump` inspects a file: header, sections, scan metadata, blocks, and an
  optional decode-everything check.
- `tzf2ply` converts a scan to levelled XYZ, written as both binary PLY and
  LAS 1.2.
- The `trimble_tzf` crate gives the same access from Rust.

The format is documented from the **observable byte structure** of `.tzf`
files in [`docs/format.md`](docs/format.md); how it was worked out is in
[`docs/PROVENANCE.md`](docs/PROVENANCE.md).

Independent interoperability project, documented from the files themselves. It
contains no vendor source, firmware, or binaries, and is **not affiliated with
or endorsed by Trimble Inc.**; "Trimble", "X7", "X9" are trademarks of their
respective owners.

Contents: [What is supported](#what-is-supported) · [Build and install](#build-and-install) ·
[tzfdump](#tzfdump) · [tzf2ply](#tzf2ply) · [Library](#library) ·
[Coordinates](#coordinates) · [Accuracy](#accuracy) ·
[Limitations](#limitations) · [Tests](#tests)

## What is supported

A `.tzf` file holds one scan from one station. Its scan data is a sequence of
**blocks**, each a 60-byte descriptor plus a compressed payload. The blocks
tile the scan grid (rows = vertical angle, columns = horizontal angle) in
512 × 512 tiles, with one block per **component** per tile. A metadata block
after the data gives the grid size, angle steps, start angles, point count and
a levelling rotation. Full details: [`docs/format.md`](docs/format.md).

| Feature | Support |
|---|---|
| Full-dome scans | Yes. Validated on an X9 full dome (6,281,859 points, 36 tiles). |
| Area (window) scans | Yes. Same layout with a smaller grid and non-zero start angles. Validated on an X9 area scan (777,580 points, 4 tiles); single-tile area scans decode too. |
| Component 1: range | Decoded. Metres = value / descriptor scale / 1000. `0` and `0xFFFFFFFF` are no-return cells and are skipped. |
| Component 2: vertical angle | Decoded. Per-point µrad from the zenith, delta-coded along each vertical scan line. |
| Component 3: horizontal angle | Decoded. Per-point µrad, stored negated, delta-coded along each line. |
| Component 4: intensity | Located but **not decoded**. It is a 512 × 512 8-bit JPEG per tile, available raw through the API. Not present in every scan. |
| Raw DEFLATE payloads | Yes (via `flate2`). Seen on area scans and older full domes. |
| Raw Snappy payloads | Yes (built-in decoder of the public Snappy block format). Seen on full-dome scans from current firmware. |
| JPEG payloads | Returned as raw bytes (not decoded). |
| Levelling (tilt) | Applied. The file's 3 × 3 rotation is applied to every point as p′ = Rᵀ · p, so +z is up. |
| Measured angles | Points use the instrument's measured angles per point, not an idealised grid. |
| Output formats | PLY (binary little-endian) and LAS 1.2 (point format 0), from `tzf2ply`. |

Scanners other than the X9 (for example the X7) write `.tzf` files too, but
only X9 files have been tested.

## Build and install

Rust 1.77 or later. Dependencies: `flate2`, `memmap2`, `thiserror`.

```bash
git clone https://github.com/northidaholabs/trimble-tzf
cd trimble-tzf
cargo build --release          # target/release/tzfdump, target/release/tzf2ply
cargo test
```

Install both binaries into `~/.cargo/bin`:

```bash
cargo install --git https://github.com/northidaholabs/trimble-tzf --tag v0.2.0
```

Use the library from another crate:

```toml
[dependencies]
trimble-tzf = { git = "https://github.com/northidaholabs/trimble-tzf", tag = "v0.2.0" }
```

The crate is imported as `trimble_tzf`. Use `v0.2.0` or later: the `v0.1.0`
tag predates point decoding, Snappy support and `tzf2ply` (and the package
was still named `tzf` then). Changes are listed in
[`CHANGELOG.md`](CHANGELOG.md).

Tagged releases attach prebuilt binaries for `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu` (64-bit Raspberry Pi OS) as
`trimble-tzf-<target>.tar.gz`, each holding `tzfdump`, `tzf2ply`, `LICENSE`
and `README.md`.

## tzfdump

Inspect a file.

```
tzfdump <file.tzf> [--blocks] [--verify]
```

The first argument that does not start with `--` is the file; flags can come
before or after it.

| Flag | Effect |
|---|---|
| (none) | Print the file size, the seven section offsets, and the scan metadata: grid rows × cols, tile size, point count, both angle steps, `step × cols` in degrees (≈ 360 for a full dome), start HA and start VA in radians and degrees. |
| `--blocks` | Also walk the blocks without decoding anything: print the first 6 blocks (offset, tile, component, payload kind, size, raw size, payload bytes), the total block and tile counts, and one summary line per (component, kind, size, raw size). |
| `--patches` | Same as `--blocks` (older name). |
| `--verify` | Everything `--blocks` prints, then decode every DEFLATE and Snappy block once into one reused buffer and check every JPEG block starts with `FF D8` and ends with `FF D9`. Decode failures are printed to stderr. Blocks of unknown kind count as failures. |

Exit status: `0` on success; `1` if the file cannot be opened or its header
is invalid (a metadata block that cannot be read is printed as
`scan meta : <error>` and does not fail), or if `--verify` found a failed or unknown block or a bad JPEG; `2` for a usage
error (no file given).

Example (a 905 × 874 area scan):

```
$ tzfdump window.tzf --verify
file      : window.tzf
size      : 6421968 bytes
sections  :
  @0x10 -> 2929292 (0x2cb28c)
  ...
scan meta : grid 874x905 (rows x cols), tile 512, points 777580
            angle steps 5.028697e-4 / 5.028718e-4 rad; step x cols = 26.08 deg
            start HA 5.1630 rad (295.82 deg), start VA 1.2610 rad (72.25 deg from zenith)
  block @0x5c: tile (0,0) component 3 Deflate 512x512 raw 1048576 payload 145426 B
  ...
blocks    : 16 in 4 tiles (2 rows x 2 cols)
  component 1 Deflate 512x512 raw  1048576  x 4
  component 2 Deflate 512x512 raw  1048576  x 4
  component 3 Deflate 512x512 raw  1048576  x 4
  component 4 Jpeg    512x512 raw   262144  x 4
verify    : deflate/snappy 12 ok / 0 failed; jpeg 4 ok / 0 bad
```

`tzfdump` does not print the levelling matrix or the two unidentified header
words.

## tzf2ply

Convert a scan to a point cloud.

```
tzf2ply <in.tzf> <out-base> [--intensity-pgm <grid.pgm>]
```

The input and output base must be the first two arguments, in that order.
It writes **two** files, `<out-base>.ply` and `<out-base>.las`, overwriting
them if they exist, and prints `<n> points (<declared> expected) -> …` to
stderr. If the file's levelling rotation is missing or is not a proper
rotation, it also prints a `warning: … points are NOT levelled` line to
stderr and writes the points unlevelled (exit status is still `0`).

| Argument / flag | Effect |
|---|---|
| `<in.tzf>` | The scan file. |
| `<out-base>` | Output path without extension; `.ply` and `.las` are appended. |
| `--intensity-pgm <grid.pgm>` | Gray value per point from an 8-bit binary PGM (`P5`, maxval ≤ 255) with exactly one pixel per grid cell: `cols` wide × `rows` tall, row 0 at the top (smallest VA). Any other size is an error that names both sizes. Without this flag every point gets gray 128. |

Points are written in grid order (by column, then row) in metres, levelled,
with the scanner at the origin (see [Coordinates](#coordinates)).

| Output | Format |
|---|---|
| `.ply` | `binary_little_endian 1.0`, one `vertex` element per point: `float x, y, z` and `uchar red, green, blue` (all three set to the gray value). 15 bytes per point. |
| `.las` | LAS 1.2, point data format 0, 20-byte records, 227-byte header, no VLRs. Scale 0.0001 (0.1 mm) on all axes; offsets are the minimum corner; bounds are set. Intensity = gray × 257 (0–65535). Every point is return 1 of 1, class 0, point source 0. No coordinate reference system, GPS time, file creation date or project GUID. |

**Intensity from the scanner's preview.** The per-tile intensity JPEGs
(component 4) are located but not decoded yet. The scanner's panoramic preview image (served by the
scanner's HTTP API next to each scan file, e.g. via
[TrimbleX9-mcp](https://github.com/northidaholabs/TrimbleX9-mcp)'s
`scanner_download_preview`) is aligned with the
grid and can stand in. On full-dome scans it is about half the grid size
(e.g. 2082 × 815 for a 4165 × 1631 grid), so scale it to exactly
`cols × rows` first; area-scan previews seen so far already match the grid.
With ImageMagick, for a 1631-row × 4165-column scan:

```bash
magick preview.jpg -colorspace Gray -resize '4165x1631!' -depth 8 preview.pgm
tzf2ply scan.tzf scan --intensity-pgm preview.pgm
```

`tzfdump` prints the grid as `rows x cols`.

## Library

```rust
use std::path::Path;
use trimble_tzf::{BlockKind, Tzf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tzf = Tzf::open(Path::new("scan.tzf"))?;

    // Grid size, angle steps, start angles, point count, levelling.
    let meta = tzf.scan_meta()?;
    println!("{} x {} cells, {} points", meta.grid_rows, meta.grid_cols, meta.n_points);
    if !meta.levelling.is_applied() {
        eprintln!("not levelled: {:?}", meta.levelling);
    }

    // Levelled XYZ in metres, scanner at the origin, +z up.
    for p in tzf.points()?.iter().take(5) {
        println!("{:.3} {:.3} {:.3}", p.x, p.y, p.z);
    }

    // The same points with the grid cell each came from.
    let cells = tzf.grid_points()?;
    println!("first point is cell row {} col {}", cells[0].row, cells[0].col);

    // Walk the blocks lazily; decode or copy out only what you need.
    let mut buf = Vec::new();
    for b in tzf.blocks() {
        match b.kind {
            BlockKind::Deflate | BlockKind::Snappy => {
                tzf.decode_into(&b, &mut buf)?; // buf.len() == b.raw_size
            }
            BlockKind::Jpeg => {
                let jpeg: &[u8] = tzf.payload(&b); // component 4, a complete JPEG
                let _ = (b.tile_row, b.tile_col, jpeg);
            }
            BlockKind::Unknown => {}
        }
    }
    Ok(())
}
```

### `Tzf`, an open file

| Item | Description |
|---|---|
| `Tzf::open(&Path) -> Result<Tzf>` | Memory-map a file read-only and parse its header. Nothing is decompressed. Do not modify the file while it is open. |
| `Tzf::from_bytes(Vec<u8>) -> Result<Tzf>` | Same, from a buffer in memory. |
| `bytes(&self) -> &[u8]` | The whole file. |
| `header(&self) -> &Header` | The parsed header. |
| `scan_meta(&self) -> Result<ScanMeta>` | Parse the metadata block. |
| `blocks(&self) -> Blocks` | Lazy iterator over all blocks in the data region. Allocates nothing. If a descriptor fails the structural checks it scans forward byte by byte for the next valid one. |
| `payload(&self, &Block) -> &[u8]` | A block's raw payload bytes, e.g. the JPEG of a `Jpeg` block. |
| `decode_into(&self, &Block, &mut Vec<u8>) -> Result<()>` | Decompress a `Deflate` or `Snappy` block into a reused buffer. On success the buffer is exactly `raw_size` bytes; anything shorter or longer is an error. |
| `decode(&self, &Block) -> Result<Vec<u8>>` | Same, into a new buffer. |
| `points(&self) -> Result<Vec<Point>>` | All valid points, levelled, in metres. `intensity` is always `0.0` for now. |
| `grid_points(&self) -> Result<Vec<GridPoint>>` | Like `points`, with each point's grid `row` and `col`. Sorted by column, then row. |

`points` and `grid_points` need exactly one block each of components 1, 2 and
3 for every tile that has any of them, all with the same width and height, 4
bytes per cell, and no larger than the metadata tile size. Anything else
(a missing or duplicated component, a plane of the wrong size) is an
`Error::Malformed`, not a panic. Component 4 and unknown components are
ignored.

### Data types

| Type | Fields / methods |
|---|---|
| `Header` | `magic: [u8; 8]`; `word_08`, `word_0c: u32` (roles unknown); `sections: Vec<u64>` (the seven section offsets); `parse(&[u8]) -> Result<Header>`; `meta_offset() -> u64` (= `sections[0]`, the metadata block and end of the block data). |
| `ScanMeta` | `grid_rows`, `grid_cols`, `tile_size`, `n_points: u32`; `angle_step_rad: [f64; 2]` (horizontal, vertical); `start_ha_rad: f64` (HA of column 0, in [0, 2π), un-negated); `start_va_rad: f64` (VA of row 0, from the zenith); `tilt: [[f64; 3]; 3]` (levelling rotation, row-major); `levelling: Levelling` (whether `tilt` is the stored rotation); `ha_of_col(f64) -> f64` (instrument HA of a column); `azimuth_of_col(f64) -> f64` (`atan2(y, x)` of that column's points, = 2π − HA); `va_of_row(f64) -> f64`. |
| `Levelling` | `Applied` (stored rotation used), `Missing` (metadata too short; identity used), `NotARotation` (stored matrix rejected; identity used); `is_applied() -> bool`. |
| `Block` | `descriptor` (offset); `payload_start`, `payload_end` (byte range); `component` (1–4 on known files); `tile_row`, `tile_col`; `width`, `height` (512 × 512 on known files); `raw_size` (declared decoded bytes); `scale: f32` (descriptor value; the range divisor for component 1); `kind: BlockKind`. `Copy`, holds no payload. |
| `BlockKind` | `Deflate`, `Snappy`, `Jpeg`, `Unknown`, detected from the payload's first bytes. |
| `Blocks<'a>` | The iterator returned by `blocks()`, yielding `Block`. |
| `Point` | `x`, `y`, `z`, `intensity: f32`. |
| `GridPoint` | `point: Point`; `row`, `col: u32`. |
| `Error` | `BadMagic`; `Truncated { need, have }`; `Io(std::io::Error)`; `Decompress(String)` (a payload that does not decode to its declared size, or declares more than it could expand to); `WrongKind { offset, kind }` (decoding a block that is not DEFLATE or Snappy); `Malformed(String)` (blocks that do not fit together: missing or duplicated components, planes of the wrong or differing sizes, no range/angle blocks). |
| `Result<T>` | `std::result::Result<T, Error>`. |

Functions: `ha_to_azimuth(f64) -> f64` converts an instrument HA to the
x/y azimuth of decoded points and back (`(2π − a) mod 2π`).

Constants: `MAGIC` (the 8-byte file magic), `SECTION_POS` (header positions of
the section offsets), `FIRST_DESCRIPTOR` (`0x5c`), `DESCRIPTOR_LEN` (`60`),
`COMPONENT_RANGE` (1), `COMPONENT_VERTICAL_ANGLE` (2),
`COMPONENT_HORIZONTAL_ANGLE` (3).

### Memory

The file is memory-mapped and `blocks()`, `decode_into` and
`tzfdump --verify` work one block at a time, so they need about one decoded
block (1 MiB) plus the mapped pages actually read. That fits a Raspberry Pi.

`points()` and `grid_points()` return the whole cloud as a `Vec`: 24 bytes
per `GridPoint`, plus three 1 MiB decode buffers. `points()` builds the
`GridPoint` vector first and then converts it, so it briefly needs about 40
bytes per point. A 6.3 million point full dome takes about 150 MB through
`grid_points()`, which is what `tzf2ply` uses.

## Coordinates

Points are in a right-handed frame in metres, with the scanner at the origin:

```
x = r · sin(va) · cos(ha)    y = r · sin(va) · sin(ha)    z = r · cos(va)
```

then levelled with p′ = Rᵀ · p, so +z points up. `va` and `ha` are the
per-point angles from components 2 and 3, used as stored. Because the file
stores the horizontal angle negated, a point's azimuth `atan2(y, x)` is
2π − HA, where HA is the instrument's horizontal angle as in
`ScanMeta::ha_of_col` and in area-scan commands. So x/y azimuth increases
the opposite way to HA. `ScanMeta::azimuth_of_col` gives the azimuth of a
column directly and `ha_to_azimuth` converts either way; the unit test
`point_azimuth_is_two_pi_minus_ha` pins the relation down. It is exact before
levelling; levelling tilts points by the small tilt angle, so levelled
points match to within that. The output coordinates are unchanged from
earlier versions.

The horizontal zero is the instrument's own; points are not georeferenced and
scans from different stations are not registered to each other.

If the stored levelling matrix is missing or is not a proper rotation
(orthonormal with determinant +1, to 1e-6), `scan_meta()` uses the identity
and points are **not levelled**. `ScanMeta::levelling` says which case
applies, and `tzf2ply` prints a warning. All scans tested so far are
levelled.

## Accuracy

Checked on X9 scans recorded for this project (details in
[`docs/format.md`](docs/format.md) and
[`docs/PROVENANCE.md`](docs/PROVENANCE.md)):

| Scan | Points decoded / declared | Check |
|---|---|---|
| Area, STD_L2, color off, 905 × 874 cells, DEFLATE | 777,580 / 777,580 | Wall plane fit 1.2 mm RMS; plumb to 0.04° after levelling |
| Full dome, STD_L0, color on, 4165 × 1631 cells, Snappy + JPEG | 6,281,859 / 6,281,859 | Floor plane fit 2.0 mm RMS; level to 0.04° after levelling |

Range resolution is 0.1 mm (scale 10 on known files). Coordinates are stored
as `f32`, which keeps sub-millimetre resolution out to several hundred
metres. Accuracy beyond these checks (range bias, angular calibration against
a reference) has not been measured.

## Limitations

- **Intensity is not decoded.** `Point::intensity` is always 0, and
  `tzf2ply` uses a PGM you supply or a constant gray. The per-tile JPEGs
  (component 4) are reachable through `Tzf::payload`.
- **No colour.** Images taken with color mode are not read or applied.
- **Other sections are not interpreted.** The embedded preview JPEG, the
  full-grid planes that area scans carry after the metadata block, and the
  remaining sections are not exposed beyond their offsets in
  `Header::sections`. The header words at `0x08` / `0x0c` and several
  descriptor fields have unknown roles.
- **One station at a time.** No registration, georeferencing or coordinate
  reference system.
- **Validated on the X9 only**, on scans recorded in October 2026. Other
  models or firmware may use payload kinds or layouts not seen yet; `tzfdump
  --verify` reports blocks it cannot decode.
- **Not hardened against hostile input.** Malformed range/angle planes,
  duplicate blocks and impossible declared sizes are reported as errors, and
  allocations are sized from the data present rather than the declared point
  count; unit tests truncate and corrupt every byte of a small synthetic file
  without a panic. The decoder has not been fuzzed, and a corrupt but
  structurally valid file can still decode to wrong points.
- **Whole cloud in memory** for `points()`, `grid_points()` and `tzf2ply`
  (see [Memory](#memory)).
- **LAS point counts** use the 32-bit legacy fields of LAS 1.2, so `tzf2ply`
  handles up to about 4.29 billion points per file.

## Tests

```bash
cargo test
```

| Test | What it checks |
|---|---|
| Unit tests in `src/lib.rs` (synthetic files) | Non-`.tzf` input is rejected; block walk, tile placement and DEFLATE decode; JPEG blocks are not decodable; metadata angles and HA wrap-around; Snappy literals and overlapping copies; the levelling guard accepts only proper rotations and `ScanMeta::levelling` reports each case; range/angle planes turn into the expected XYZ; point azimuth is 2π − HA; the walker resyncs past a corrupt descriptor; planes not 4 bytes per cell, planes of differing sizes, duplicate or missing components and blocks larger than a tile are errors; a huge declared point count or raw size does not drive allocation; every truncation and single-byte corruption of a synthetic file returns without panicking |
| `tests/fixture.rs` | With `TZF_FIXTURE=<path to a .tzf>`: decodes the file, checks levelling was applied, the point count equals the declared count and every range is between 0.1 m and 400 m. Without the variable it passes without doing anything. |

Scan files are not committed (size, and they show private places), so the
fixture test is opt-in:

```bash
TZF_FIXTURE=/path/to/scan.tzf cargo test --release
```

CI runs `cargo test --locked` and builds release archives (`tzfdump` and
`tzf2ply`) for x86_64 and aarch64 Linux.

## License

MIT (see `LICENSE`).
