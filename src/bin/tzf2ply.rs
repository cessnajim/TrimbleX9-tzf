//! tzf2ply — convert a Trimble TZF scan to PLY (binary) and LAS 1.2.
//!
//! Usage: `tzf2ply <in.tzf> <out-base> [--intensity-pgm <grid.pgm>]`
//!
//! Writes `<out-base>.ply` (x y z float + gray rgb) and `<out-base>.las`
//! (point format 0, 0.1 mm scale). TZF files carry no intensity plane we know
//! of yet, so intensity can come from a grayscale image with one pixel per
//! grid cell (`cols` wide × `rows` tall), e.g. the scanner's preview JPEG
//! converted to binary PGM. Without it, points are written mid-gray.

use std::io::{BufWriter, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: tzf2ply <in.tzf> <out-base> [--intensity-pgm <grid.pgm>]");
        return ExitCode::from(2);
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let tzf = tzf::Tzf::open(std::path::Path::new(&args[1]))?;
    let meta = tzf.scan_meta()?;
    let pts = tzf.grid_points()?;
    let gray = match args.iter().position(|a| a == "--intensity-pgm") {
        Some(i) => {
            let path = args.get(i + 1).ok_or("--intensity-pgm needs a path")?;
            let (w, h, px) = read_pgm(path)?;
            if (w, h) != (meta.grid_cols as usize, meta.grid_rows as usize) {
                return Err(format!(
                    "intensity image is {w}x{h}, grid is {}x{}",
                    meta.grid_cols, meta.grid_rows
                )
                .into());
            }
            Some((w, px))
        }
        None => None,
    };
    let g = |p: &tzf::GridPoint| match &gray {
        Some((w, px)) => px[p.row as usize * w + p.col as usize],
        None => 128u8,
    };

    let base = &args[2];
    write_ply(&format!("{base}.ply"), &pts, &g)?;
    write_las(&format!("{base}.las"), &pts, &g)?;
    eprintln!(
        "{} points ({} expected) -> {base}.ply, {base}.las",
        pts.len(),
        meta.n_points
    );
    Ok(())
}

fn write_ply(
    path: &str,
    pts: &[tzf::GridPoint],
    gray: &dyn Fn(&tzf::GridPoint) -> u8,
) -> std::io::Result<()> {
    let mut w = BufWriter::new(std::fs::File::create(path)?);
    write!(
        w,
        "ply\nformat binary_little_endian 1.0\ncomment tzf2ply\nelement vertex {}\n\
         property float x\nproperty float y\nproperty float z\n\
         property uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n",
        pts.len()
    )?;
    for p in pts {
        let q = p.point;
        let v = gray(p);
        for f in [q.x, q.y, q.z] {
            w.write_all(&f.to_le_bytes())?;
        }
        w.write_all(&[v, v, v])?;
    }
    w.flush()
}

/// LAS 1.2, point data format 0 (20-byte records), no VLRs.
fn write_las(
    path: &str,
    pts: &[tzf::GridPoint],
    gray: &dyn Fn(&tzf::GridPoint) -> u8,
) -> std::io::Result<()> {
    const SCALE: f64 = 0.0001;
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for p in pts {
        let q = [p.point.x as f64, p.point.y as f64, p.point.z as f64];
        for k in 0..3 {
            min[k] = min[k].min(q[k]);
            max[k] = max[k].max(q[k]);
        }
    }
    if pts.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    let mut h = Vec::with_capacity(227);
    h.extend_from_slice(b"LASF");
    h.extend_from_slice(&[0u8; 4]); // file source id, global encoding
    h.extend_from_slice(&[0u8; 16]); // project GUID
    h.extend_from_slice(&[1, 2]); // version 1.2
    h.extend_from_slice(&pad32(b"tzf"));
    h.extend_from_slice(&pad32(b"tzf2ply"));
    h.extend_from_slice(&[0u8; 4]); // creation day / year
    h.extend_from_slice(&227u16.to_le_bytes()); // header size
    h.extend_from_slice(&227u32.to_le_bytes()); // offset to point data
    h.extend_from_slice(&0u32.to_le_bytes()); // number of VLRs
    h.push(0); // point data format 0
    h.extend_from_slice(&20u16.to_le_bytes()); // record length
    h.extend_from_slice(&(pts.len() as u32).to_le_bytes());
    h.extend_from_slice(&(pts.len() as u32).to_le_bytes()); // all first returns
    h.extend_from_slice(&[0u8; 16]);
    for _ in 0..3 {
        h.extend_from_slice(&SCALE.to_le_bytes());
    }
    for m in min {
        h.extend_from_slice(&m.to_le_bytes()); // offsets = min corner
    }
    for k in 0..3 {
        h.extend_from_slice(&max[k].to_le_bytes());
        h.extend_from_slice(&min[k].to_le_bytes());
    }
    debug_assert_eq!(h.len(), 227);

    let mut w = BufWriter::new(std::fs::File::create(path)?);
    w.write_all(&h)?;
    for p in pts {
        let q = [p.point.x as f64, p.point.y as f64, p.point.z as f64];
        for k in 0..3 {
            let v = ((q[k] - min[k]) / SCALE).round() as i32;
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&(gray(p) as u16 * 257).to_le_bytes())?; // intensity
        w.write_all(&[0b0000_1001, 0, 0, 0])?; // return 1 of 1, class, angle, user
        w.write_all(&0u16.to_le_bytes())?; // point source id
    }
    w.flush()
}

fn pad32(s: &[u8]) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[..s.len()].copy_from_slice(s);
    b
}

/// Minimal binary PGM (P5, maxval ≤ 255) reader.
fn read_pgm(path: &str) -> Result<(usize, usize, Vec<u8>), Box<dyn std::error::Error>> {
    let d = std::fs::read(path)?;
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while i < d.len() && d[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < d.len() && d[i] == b'#' {
            while i < d.len() && d[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let s = i;
        while i < d.len() && !d[i].is_ascii_whitespace() {
            i += 1;
        }
        if s == i {
            return Err("truncated PGM header".into());
        }
        fields.push(std::str::from_utf8(&d[s..i])?.to_string());
    }
    if fields[0] != "P5" || fields[3].parse::<u32>()? > 255 {
        return Err("expected 8-bit binary PGM (P5)".into());
    }
    let (w, h): (usize, usize) = (fields[1].parse()?, fields[2].parse()?);
    let px = d.get(i + 1..i + 1 + w * h).ok_or("truncated PGM pixels")?.to_vec();
    Ok((w, h, px))
}
