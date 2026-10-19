//! tzfdump — inspect a Trimble X-series `.tzf` file.
//!
//!   tzfdump <file.tzf>             header, sections, scan metadata
//!   tzfdump <file.tzf> --blocks    + block summary (lazy; decodes nothing)
//!   tzfdump <file.tzf> --verify    + decode every block once (bounded memory)

use std::collections::BTreeMap;
use std::process::ExitCode;
use tzf::{BlockKind, Tzf};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.iter().skip(1).find(|a| !a.starts_with("--")) else {
        eprintln!("usage: tzfdump <file.tzf> [--blocks] [--verify]");
        return ExitCode::from(2);
    };
    let flag = |f: &str| args.iter().any(|a| a == f);
    let tzf = match Tzf::open(std::path::Path::new(path)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let h = tzf.header();
    println!("file      : {path}");
    println!("size      : {} bytes", tzf.bytes().len());
    println!("sections  :");
    for (pos, off) in tzf::SECTION_POS.iter().zip(&h.sections) {
        println!("  @0x{pos:02x} -> {off} (0x{off:x})");
    }
    match tzf.scan_meta() {
        Ok(m) => {
            println!(
                "scan meta : grid {}x{} (rows x cols), tile {}, points {}",
                m.grid_rows, m.grid_cols, m.tile_size, m.n_points
            );
            let s = m.angle_step_rad;
            println!(
                "            angle steps {:.6e} / {:.6e} rad; step x cols = {:.2} deg",
                s[0],
                s[1],
                (s[0] * m.grid_cols as f64).to_degrees()
            );
            println!(
                "            start HA {:.4} rad ({:.2} deg), start VA {:.4} rad ({:.2} deg from zenith)",
                m.start_ha_rad,
                m.start_ha_rad.to_degrees(),
                m.start_va_rad,
                m.start_va_rad.to_degrees()
            );
        }
        Err(e) => println!("scan meta : <{e}>"),
    }

    if flag("--blocks") || flag("--patches") || flag("--verify") {
        let mut summary: BTreeMap<(u32, String, u32, u32, u32), usize> = BTreeMap::new();
        let mut total = 0usize;
        let mut tiles = std::collections::BTreeSet::new();
        for (i, b) in tzf.blocks().enumerate() {
            tiles.insert((b.tile_row, b.tile_col));
            if i < 6 {
                println!(
                    "  block @0x{:x}: tile ({},{}) component {} {:?} {}x{} raw {} payload {} B",
                    b.descriptor,
                    b.tile_row,
                    b.tile_col,
                    b.component,
                    b.kind,
                    b.width,
                    b.height,
                    b.raw_size,
                    b.payload_end - b.payload_start
                );
            }
            *summary
                .entry((
                    b.component,
                    format!("{:?}", b.kind),
                    b.width,
                    b.height,
                    b.raw_size,
                ))
                .or_default() += 1;
            total += 1;
        }
        let (max_r, max_c) = tiles
            .iter()
            .fold((0, 0), |(r, c), &(tr, tc)| (r.max(tr), c.max(tc)));
        println!(
            "blocks    : {total} in {} tiles ({} rows x {} cols)",
            tiles.len(),
            max_r + 1,
            max_c + 1
        );
        for ((c, k, w, hh, raw), n) in &summary {
            println!("  component {c} {k:<7} {w}x{hh} raw {raw:>8}  x {n}");
        }
    }

    if flag("--verify") {
        let mut buf = Vec::new();
        let (mut ok, mut bad, mut jpeg_ok, mut jpeg_bad) = (0, 0, 0, 0);
        for b in tzf.blocks() {
            match b.kind {
                BlockKind::Deflate => match tzf.decode_into(&b, &mut buf) {
                    Ok(()) => ok += 1,
                    Err(e) => {
                        bad += 1;
                        eprintln!("  decode failed: {e}");
                    }
                },
                BlockKind::Jpeg => {
                    let p = tzf.payload(&b);
                    if p.starts_with(&[0xff, 0xd8]) && p.ends_with(&[0xff, 0xd9]) {
                        jpeg_ok += 1
                    } else {
                        jpeg_bad += 1
                    }
                }
                BlockKind::Unknown => bad += 1,
            }
        }
        println!("verify    : deflate {ok} ok / {bad} failed; jpeg {jpeg_ok} ok / {jpeg_bad} bad");
        if bad > 0 || jpeg_bad > 0 {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
