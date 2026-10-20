# Provenance

This crate reads `.tzf` files for interoperability: getting point clouds out of
scans made on a scanner its author owns, on platforms the vendor does not
support. This file records how the format knowledge in `docs/format.md` was
obtained, so anyone can check it.

## Method

Every fact the decoder relies on was established from **files produced by our
own instrument** and checked against independent measurements:

1. **Controlled scans.** Scans were commanded over the scanner's HTTP
   interface with chosen parameters (scan mode, area-scan angles, color
   on/off). The values we sent were then found in the file
   (start angles, grid size, angle step, point count).
2. **Physical checks.** Decoded geometry was tested against things that must be
   true: flat walls fit a plane at 1–2 mm RMS, walls are plumb and floors level
   after levelling, and the point count equals the count the file declares.
3. **Public formats only.** Compression is standard raw DEFLATE (via `flate2`)
   and raw Snappy, decoded here from the public Snappy block-format description.

## What this repository does not contain

- No vendor source code, binaries, SDK headers or documentation.
- No names of vendor-internal functions, classes, namespaces or files.
- No content derived from disassembling or decompiling vendor software.

## Evidence files

The validation scans were recorded on 2026-10-19 with a Trimble X9:

| Scan | Mode | Grid | Payload | Used to establish |
|---|---|---|---|---|
| Area scan of a window | STD_L2, color off | 905 × 874 | Deflate | metadata offsets, components 1–3, cell order, angle coding, range scale, levelling |
| Full dome of a room | STD_L0, color on | 4165 × 1631 | Snappy + JPEG | Snappy payloads, levelling on a floor, intensity component |

The scan files are not committed (size, and they show a private home). Tests
that need them run when `TZF_FIXTURE` points at a file.

## Limits of the process

This is interoperability reverse engineering from data, not a two-team
clean-room process. The same people who wrote this crate have, separately,
looked at the vendor's installed software while investigating how the scanner
connects. To keep that apart:

- Format facts are stated here only with evidence from our own files.
- Where an idea about the format first came from that separate investigation,
  it was treated as a hypothesis and kept only once our own data confirmed it.
  This applies to one item: trying Snappy for the full-dome payloads. It was
  confirmed by the data (every block declares exactly 1 MiB in Snappy's own
  header and decodes to it, and all 6,281,859 points come out).

This is a description of the method, not legal advice.
