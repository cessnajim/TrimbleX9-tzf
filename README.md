# TrimbleX9-tzf

A Rust reader for **Trimble X‑series `.tzf`** terrestrial‑laser‑scan files (X7/X9).
The format is documented from the **observable byte structure** of `.tzf` files
in `docs/format.md`. A scan's data region is a sequence of blocks, each a
60‑byte descriptor plus a payload, tiling a 512×512 grid with several
components per tile.

Independent interoperability project, documented from the files themselves. It
contains no vendor source, firmware, or binaries, and is **not affiliated with
or endorsed by Trimble Inc.**; "Trimble", "X7", "X9" are trademarks of their
respective owners.

## Status: work in progress

| Piece | State |
|---|---|
| File header + section table | ✅ `Header::parse` |
| Scan metadata (grid, angle steps, tile size, point count) | ✅ `Tzf::scan_meta` |
| Deterministic block walk (lazy, allocation‑free) | ✅ `Tzf::blocks` |
| Tile placement (row, col per block) | ✅ `Block::tile_row` / `tile_col` |
| Block decode (raw‑deflate 512×512×4; JPEG payloads) | ✅ `Tzf::decode` / `decode_into` / `payload` |
| Meaning of components 1–3, range scale | ⬜ TODO |
| Spherical → XYZ, LAS/PLY export | ⬜ TODO |

Until the decode path lands, `Tzf::points()` returns `Error::Unimplemented`.

**Memory:** files are memory‑mapped and blocks decode on demand into a reusable
buffer, so working memory is about one decoded block (1 MiB) plus the mapped
pages actually read. This keeps a Raspberry Pi in budget.

## Use

```bash
cargo run --bin tzfdump -- scan.tzf            # header, sections, scan metadata
cargo run --bin tzfdump -- scan.tzf --blocks   # block summary (decodes nothing)
cargo run --bin tzfdump -- scan.tzf --verify   # decode every block once
cargo test
```

```rust
use tzf::{BlockKind, Tzf};

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
