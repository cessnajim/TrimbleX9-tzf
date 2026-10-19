# TrimbleX9-tzf

A Rust reader for **Trimble X‑series `.tzf`** terrestrial‑laser‑scan files (X7/X9).
The format is documented from the **observable byte structure** of `.tzf` files
(`docs/format.md`) — a spherical range‑image on a grid, tiled into 512×512
patches, each independently compressed, convertible to XYZ via the grid's
angular step plus a tilt correction.

Independent interoperability project, documented from the files themselves. It
contains no vendor source, firmware, or binaries, and is **not affiliated with
or endorsed by Trimble Inc.**; "Trimble", "X7", "X9" are trademarks of their
respective owners.

## Status — work in progress

| Piece | State |
|---|---|
| File header + `u64` section table | ✅ parsed (`Header::parse`, `tzfdump`) |
| Patch decode (`[0x78 0x01]` → raw‑deflate → 512×512×4) | ✅ `decode_patch` / `patch_streams` |
| Scan calibration (grid, angular step, point count) | 🟡 located; field offsets being confirmed |
| Per‑patch pixel/plane layout + transforms (plain / shuffle+delta / snappy) | ⬜ TODO |
| Patch placement + range scale | ⬜ TODO |
| Spherical → XYZ + tilt, LAS/PLY export | ⬜ TODO |

Until the decode path lands, `Tzf::points()` returns `Error::Unimplemented`.

## Use

```bash
cargo run --bin tzfdump -- scan.tzf            # header + sections
cargo run --bin tzfdump -- scan.tzf --patches  # locate + decode patch streams
cargo test
```

```rust
let tzf = tzf::Tzf::open(std::path::Path::new("scan.tzf"))?;
println!("{} patch streams", tzf.patch_streams().len());
```

## License

MIT (see `LICENSE`).
