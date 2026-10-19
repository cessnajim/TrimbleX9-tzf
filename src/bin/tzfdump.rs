//! tzfdump — inspect a Trimble TZF file (header + sections; more as the decoder
//! lands). Usage: `tzfdump <file.tzf>`

use std::process::ExitCode;

fn main() -> ExitCode {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: tzfdump <file.tzf>");
            return ExitCode::from(2);
        }
    };
    let tzf = match tzf::Tzf::open(std::path::Path::new(&path)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let h = &tzf.header;
    println!("file         : {path}");
    println!("size         : {} bytes", tzf.data.len());
    println!("magic        : {}", hex(&h.magic));
    println!("word_08/0c   : {} / {}", h.word_08, h.word_0c);
    println!("sections (u64 offsets):");
    for (pos, off) in tzf::SECTION_POS.iter().zip(&h.sections) {
        let note = if (*off as usize) < tzf.data.len() { "" } else { "  (out of range)" };
        println!("  @0x{pos:02x} -> {off} (0x{off:x}){note}");
    }
    // Best-effort hex peek at the metadata block to aid ongoing RE.
    let mo = h.meta_offset() as usize;
    if mo + 64 <= tzf.data.len() {
        println!("meta block @ {mo} (0x{mo:x}), first 64 bytes:");
        println!("  {}", hex(&tzf.data[mo..mo + 64]));
    }
    match tzf.scan_meta() {
        Ok(m) => println!("scan meta    : {m:?}"),
        Err(e) => println!("scan meta    : <{e}>"),
    }
    if std::env::args().any(|a| a == "--patches") {
        let streams = tzf.patch_streams();
        println!("patch streams: {} (each inflates to {} bytes = 512x512x4)",
                 streams.len(), tzf::PATCH_BYTES);
        for s in streams.iter().take(8) {
            let nz = s.raw.iter().filter(|&&b| b != 0).count();
            println!("  @0x{:x}  comp={}  nonzero={}% ", s.offset, s.comp_size,
                     100 * nz / s.raw.len());
        }
        if streams.len() > 8 {
            println!("  ... and {} more", streams.len() - 8);
        }
    }
    ExitCode::SUCCESS
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}
