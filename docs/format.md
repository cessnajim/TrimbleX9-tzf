# Trimble X‑series `.tzf` — observed file structure

These notes describe the `.tzf` scan‑file layout **as observed from the bytes of
sample files** (an X9 scan: 83,570,701 B, 13,718,279 points). No vendor code,
headers, or documentation were used; everything below is re‑derivable with a hex
viewer and a DEFLATE library. Still incomplete — enough to decode patch payloads,
not yet to emit XYZ.

All integers little‑endian.

## File header
- `0x00`: 8‑byte magic `5e de 73 9b db 7e 79 ab`.
- `0x08`: u32, `0x0c`: u32 (roles unconfirmed).
- `u64` **section offsets** at `0x10, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50`. On the
  sample these point at a metadata block and several directories near EOF; the
  compressed patch data occupies `[0x9c, sections[0])`.
- `0x58…0x9c`: the first patch's descriptor (see below), then the first patch.

## Patches (the scan payload)
The scan is a spherical range‑image on a grid, tiled into **512×512 patches**.
Each patch block is:

```
[descriptor][0x78 0x01][compressed body]
```

- The body is **raw DEFLATE**. Inflating from `marker+2` yields exactly
  **1,048,576 bytes = 512 × 512 × 4**, clean stream‑end. (A standard zlib read
  fails the Adler check — the `0x78 0x01` is a header‑shaped prefix only.)
- **Descriptor** (u32s immediately preceding the marker):
  `[id 8B][0xC8][0x10][compressedSize][patchIndex][0][0][0x64][512][512]
  [0x100000 = uncompressed size][scale f32][hash 8B][0x100000]`.
  `patchIndex` is sequential; the first patch is all‑zero (a no‑return region).
- **Cells**: 4 bytes each; `0` and `0xFFFFFFFF` act as no‑data/invalid sentinels.
- **Per‑patch interpretation varies.** Some patches read directly as small,
  bounded range values (one sample patch: all cells 0..1952). Others read as
  full‑range noise unless first un‑transformed — those use a **byte‑plane
  shuffle + delta** before deflate (a naive un‑shuffle/un‑delta corrupts a plain
  patch, confirming the transform is per‑patch, not global). A third form is
  Snappy‑compressed. Total patch streams ≈ 3× the patch‑position count, so each
  grid position carries multiple planes (range + intensity + an overview level).

## Metadata block (at `sections[0]`)
Holds the calibration. Observed fields: grid **≈ 8330 cols × 3262 rows**, two
near‑equal `f64` angular steps **≈ 7.56e‑4 rad** (one per axis), the point count
(`13,718,279`), and the patch size (`512`).

## Other sections (near EOF)
- One section begins with `FF D8 FF E0 … JFIF` — an embedded **preview JPEG**.
- One is a **patch directory** whose header carries a patch‑position count.
- One indexes the trailing meta chunks; another is an `f32` per‑scanline array.

## Still to work out (for XYZ)
1. Which patch streams are the full‑res **range** planes vs intensity/overview,
   and the per‑patch transform (plain / shuffle+delta / snappy) — selected by a
   descriptor field. Validation: valid range cells should total the point count.
2. The range **scale** (cell value → metres).
3. Each patch's (row, col) **placement** in the grid.
4. Spherical `(range, col·step_h, row·step_v)` → XYZ, then the leveling/tilt
   correction, and export to LAS/PLY.
