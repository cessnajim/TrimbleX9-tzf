# Trimble X‑series `.tzf` — observed file structure

These notes describe the `.tzf` scan‑file layout **as observed from the bytes of
sample files** (an X9 scan: 83,570,701 B, 13,718,279 points). Items marked
*unknown* have not been worked out yet. All integers little‑endian.

## File header
- `0x00`: 8‑byte magic `5e de 73 9b db 7e 79 ab`.
- `0x08`: u32, `0x0c`: u32 (roles unknown).
- `u64` **section offsets** at `0x10, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50`.
  `sections[0]` is the scan metadata block and also the **end of the block
  data region**. Other sections lie after it (see below).
- The first block descriptor starts at `0x5c`.

## Blocks (the scan payload)
The data region `[0x5c, sections[0])` is an unbroken sequence of blocks:

```
[60-byte descriptor][payload]   [60-byte descriptor][payload]   ...
```

**Descriptor** (offsets relative to the descriptor start):

| off | type | field | sample value |
|---|---|---|---|
| 0  | u64 | id | constant within a file |
| 8  | u32 | tag | 200 |
| 12 | u32 | tag | 16 |
| 16 | u32 | **length L** | payload length = L − 28 |
| 20 | u32 | **component** | 1..4 |
| 24 | u32 | **tile row** | 0..6 |
| 28 | u32 | **tile col** | 0..16 |
| 32 | u32 | tag | 100 |
| 36 | u32 | **width** | 512 |
| 40 | u32 | **height** | 512 |
| 44 | u32 | **raw size** | decoded bytes (width × height × bytes/px) |
| 48 | f32 | scale | varies (role unknown) |
| 52 | u64 | hash | varies |

The next descriptor begins immediately after the payload, so the region can be
walked deterministically. On the sample the walk visits **476 blocks** and ends
exactly at `sections[0]`.

**Payload kinds** (by first bytes):
- **Deflate** — `[u32 raw size][0x78 0x01][raw DEFLATE …]`. Inflating the raw
  DEFLATE stream yields exactly `raw size` bytes (a standard zlib read fails the
  checksum; treat the `0x78 0x01` as a fixed prefix). Components 1, 2, 3:
  512×512×4 = 1,048,576 bytes each.
- **JPEG** — a baseline JPEG (`FF D8 FF E0 … JFIF … FF D9`). Component 4:
  512×512, 8‑bit (raw size 262,144).

**Tiling.** The grid is 3262 × 8330 cells, so ⌈3262/512⌉ × ⌈8330/512⌉ =
7 × 17 = 119 tiles. Each descriptor carries its tile's (row, col). On the sample,
all 119 (row, col) pairs occur, each with exactly one block of every component
1–4 (476 blocks). Tiles appear in column‑major order (row varies fastest); the
order of components within a tile varies. Tile (r, c) covers grid rows
`512·r … 512·r+511` and columns `512·c … 512·c+511`, clipped at the grid edge.

**Cells** in components 1–3 are 4 bytes. `0` and `0xFFFFFFFF` appear as
no‑data values. Components 1–3 are range, vertical angle and horizontal angle; see below.

## Metadata block (at `sections[0]`)

| off | type | field | sample value |
|---|---|---|---|
| 0x10 | f64 | angle step A | 7.5431e‑4 rad |
| 0x18 | f64 | angle step B | 7.5431e‑4 rad |
| 0x20 | f64 | VA of the first row, radians from the zenith | 0.0 (full dome); 1.47 (area scan) |
| 0x28 | f64 | **negated** HA of the first column, radians | −3.5788 |
| 0x30 | u32 | grid rows | 3262 |
| 0x34 | u32 | grid cols | 8330 |
| 0x38 | u32 | tile size | 512 |
| 0x3c | u32 | point count | 13,718,279 |

Consistency check: step × cols = 6.2834 rad = **360.01°**, i.e. the columns span
one full horizontal revolution. step × rows ≈ 141°.

Grid cell `(row, col)` therefore lies at VA = `m[0x20] + row × stepB` and
HA = `(−m[0x28] + col × stepA) mod 2π`. Verified on a live instrument: an
area scan commanded at HA 5.3588 / VA 1.1741 rad produced a file with
`m[0x28] = −5.3588`, `m[0x20] = 1.1741`, and its content matched the
full‑dome scan at the cell those formulas predict (within 3 cells). Area
scans have the same layout with a smaller grid.

## Other sections (after the data region)
- One section begins with `FF D8 FF E0 … JFIF`: an embedded preview JPEG.
- Others hold small directories and an `f32` array; their roles are *unknown*.

## Components 1–3: range and measured angles

Worked out on scans recorded for this project (see `PROVENANCE.md`), by
commanding scans with known parameters and checking the decoded values against
them and against flat surfaces.

| Component | Cell type | Meaning |
|---|---|---|
| 1 | u32 | **Range.** Metres = value / scale / 1000, where scale is the descriptor's f32 at offset 48 (10.0 on samples, so 0.1 mm units). `0` = no return. |
| 2 | u32 / i32 | **Vertical angle**, µrad from the zenith. |
| 3 | u32 / i32 | **Horizontal angle**, µrad, stored negated (same sign as metadata `0x28`). |

**Cell order inside a tile is column-major.** Cell `line × height + k` is grid
column `512·tile_col + line` and row `512·tile_row + k`: each run of `height`
cells is one vertical scan line. (This replaces the earlier row-major
assumption.)

**Angle planes are delta-coded per line.** The first cell of each line holds
the absolute angle; every following cell holds a signed `i32` difference from
the previous cell. Integrate along the line, including across empty cells.
Values that look like huge `u32`s are negative deltas. The angles are the
instrument's measured angles per point, not an ideal grid.

Evidence (area scan commanded at start VA 1.261 rad, start HA 5.163 rad,
STD_L2, 905 × 874 cells):
- Component 2's first cells read 1,261,058 µrad; steps are ~503 µrad, matching
  the metadata angle step (5.029e‑4 rad).
- Component 3's first cells read −5,162,788 µrad, matching the negated start HA.
- Component 1 equals 10 × the full-grid `f32` range plane (below) in every cell
  (correlation 1.0000).

**To XYZ** (right-handed, metres), with `va`, `ha` the integrated angles in
radians and `r` the range:

```
x = r·sin(va)·cos(ha)    y = r·sin(va)·sin(ha)    z = r·cos(va)
```

## Levelling (tilt)

The metadata block holds a 3×3 rotation at offset `0x44` (nine `f64`,
row-major). Its off-diagonal terms match the two leveling-compensator readings
the scanner reports over its HTTP status endpoint while the scan is taken
(e.g. 0.01545 and 0.02297 rad). Level each point with **p′ = Rᵀ · p**.

Evidence: applying Rᵀ brings a wall from 0.33° off plumb to 0.04° (area scan)
and a floor to 0.04° from level (full-dome scan). R, and the mirrored variants
S·R·S and S·Rᵀ·S (S = diag(1, −1, 1)), all make the result worse.

## Snappy payloads (full-dome scans, current firmware)

On full-dome scans recorded in 2026, components 1–3 are not Deflate. The
payload begins directly with a raw **Snappy** block (public block format, no
framing): its leading varint is the raw size (`80 80 40` = 1,048,576), and
decoding it yields exactly that many bytes. The descriptor's length rule is
unchanged (payload = L − 28). Planes, cell order and angle coding are the same
as above.

## Full-grid planes after the metadata

Area scans also carry three `rows × cols × 4` raw-DEFLATE blocks after the
metadata block (scale 1.0): range as `f32` millimetres (exactly `nbPoints`
finite cells), and the two angles as `f32` bit patterns with integer deltas.
They duplicate components 1–3.

## Intensity (component 4)

Component 4 is the **laser intensity image** for its tile: a baseline
512 × 512 8-bit grayscale JPEG. Unlike components 1–3 it is stored **row-major,
upright**: JPEG pixel (x, y) is grid column `512·tile_col + x`, row
`512·tile_row + y`.

Evidence: in the full-dome scan (36 tiles, one JPEG each), tile (1, 4)
downsampled 2× correlates 0.87 with the matching region of the scanner's
preview image; transposed or flipped, it correlates 0.12 or less.

Not every scan carries it: the color-off area scan has no component 4, while
the color-on full-dome scan does. Whether color mode is what controls it is
not yet confirmed. When it is absent, the scanner's preview image is
grid-aligned and can supply intensity (`tzf2ply --intensity-pgm`).

## Validation summary

| Scan | Points decoded / declared | Check |
|---|---|---|
| Area, STD_L2, color off, 905 × 874, Deflate | 777,580 / 777,580 | wall plane fit 1.2 mm RMS; plumb to 0.04° after levelling |
| Full dome, STD_L0, color on, 4165 × 1631, Snappy + JPEG intensity | 6,281,859 / 6,281,859 | floor plane fit 2.0 mm RMS; level to 0.04° after levelling |
