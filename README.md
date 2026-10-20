# trimble-tzf

A Rust reader for **Trimble `.tzf`** laser‑scan point‑cloud files, validated on the X9.
The format is documented from the **observable byte structure** of `.tzf` files
in `docs/format.md`. A scan's data region is a sequence of blocks, each a
60‑byte descriptor plus a payload, tiling a 512×512 grid with several
components per tile.

Independent interoperability project, documented from the files themselves. It
contains no vendor source, firmware, or binaries, and is **not affiliated with
or endorsed by Trimble Inc.**; "Trimble", "X7", "X9" are trademarks of their
respective owners.

## Status

| Piece | State |
|---|---|
| File header + section table | ✅ `Header::parse` |
| Scan metadata (grid, angle steps, tile size, point count) | ✅ `Tzf::scan_meta` |
| Deterministic block walk (lazy, allocation‑free) | ✅ `Tzf::blocks` |
| Tile placement (row, col per block) | ✅ `Block::tile_row` / `tile_col` |
| Block decode (raw DEFLATE or raw Snappy 512×512×4; JPEG payloads) | ✅ `Tzf::decode` / `decode_into` / `payload` |
| Components 1–3: range, vertical angle, horizontal angle | ✅ `docs/format.md` |
| Spherical → XYZ with levelling | ✅ `Tzf::points` / `grid_points` |
| PLY / LAS export | ✅ `tzf2ply` |
| Intensity | 🟡 located (component 4 JPEG, see `docs/format.md`); not decoded yet, so `tzf2ply --intensity-pgm` takes the scanner preview |

Validated on area and full-dome X9 scans: every declared point decodes, walls are flat to 1–2 mm RMS and plumb to 0.04° after levelling. How the format was worked out: `docs/PROVENANCE.md`.

**Memory:** files are memory‑mapped and blocks decode on demand into a reusable
buffer, so working memory is about one decoded block (1 MiB) plus the mapped
pages actually read. This keeps a Raspberry Pi in budget.

## Use

```bash
cargo run --bin tzfdump -- scan.tzf            # header, sections, scan metadata
cargo run --bin tzfdump -- scan.tzf --blocks   # block summary (decodes nothing)
cargo run --bin tzfdump -- scan.tzf --verify   # decode every block once
tzf2ply scan.tzf out --intensity-pgm preview.pgm  # out.ply + out.las
cargo test
```

```rust
use trimble_tzf::{BlockKind, Tzf};

let tzf = Tzf::open(std::path::Path::new("scan.tzf"))?;
let meta = tzf.scan_meta()?;
println!("{}x{} grid, {} points", meta.grid_rows, meta.grid_cols, meta.n_points);

let mut buf = Vec::new();
for b in tzf.blocks().filter(|b| b.kind == BlockKind::Deflate) {
    tzf.decode_into(&b, &mut buf)?; // buf.len() == 512*512*4
}
```

## License

MIT (see `LICENSE`).
