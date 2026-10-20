# Changelog

## 0.2.0 — 2026-10-20

First release that decodes points. Use this rather than `v0.1.0`.

### Added
- Point decoding: `Tzf::points` and `Tzf::grid_points` turn the range,
  vertical-angle and horizontal-angle planes (components 1–3) into levelled
  XYZ in metres.
- Raw Snappy payloads (full-dome scans from current firmware), with a
  built-in decoder; no new dependency.
- Levelling: the stored 3 × 3 rotation is applied as p′ = Rᵀ · p.
  `ScanMeta::levelling` (`Levelling::{Applied, Missing, NotARotation}`)
  reports whether it was applied.
- `tzf2ply`: writes binary PLY and LAS 1.2, optional intensity from a PGM;
  warns on stderr when points are not levelled.
- `ScanMeta::azimuth_of_col` and `ha_to_azimuth`, making explicit that a
  decoded point's `atan2(y, x)` is 2π − HA (coordinates are unchanged).
- `Error::Malformed` for blocks that do not fit together.
- Release archives `trimble-tzf-<target>.tar.gz` now ship `tzf2ply` as well as
  `tzfdump`, plus `LICENSE` and `README.md`.

### Changed
- Package renamed from `tzf` to `trimble-tzf` (crate `trimble_tzf`).
- `grid_points` returns errors instead of panicking when a range or angle
  plane is not 4 bytes per cell, the three planes of a tile differ in size, or
  a block is larger than the tile; a tile with two blocks for one component is
  an error instead of silently keeping the last one.
- Allocations are sized from the cells actually present, not the declared
  point count, and a block whose declared raw size its payload could not
  expand to is rejected before its buffer is allocated.
- Missing components and files without range/angle blocks are reported as
  `Error::Malformed` (was `Error::Decompress`).
- `Error::WrongKind` now reads "not a DEFLATE or Snappy block".

### Removed
- `Error::Unimplemented` (it was never returned).

## 0.1.0 — 2026-10-19

- Header and scan-metadata parsing (grid size, angle steps, start angles,
  point count).
- Lazy memory-mapped block walk with resync; raw-DEFLATE payload decoding.
- `tzfdump` (`--blocks`, `--verify`), prebuilt for x86_64 and aarch64 Linux.
