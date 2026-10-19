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
no‑data values. The meaning of each of components 1–3 is *unknown* (one sample
tile of component 2 holds small bounded values 0..1952; others span the full
u32 range).

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

## Still to work out (for XYZ)
1. What each of components 1–3 encodes, and the cell → range scale.
2. Grid cell → (horizontal angle, vertical angle) origin and direction, then
   spherical → XYZ and any leveling correction, and export to LAS/PLY.
3. Cell order inside a 512×512 tile (row‑major is assumed; unconfirmed).
