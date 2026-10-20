//! Decodes a real scan when `TZF_FIXTURE` points at one (scan files are not
//! committed). Checks the point count against the file metadata and that the
//! points form a plausible scene.
#[test]
fn decodes_fixture_when_present() {
    let Ok(path) = std::env::var("TZF_FIXTURE") else { return };
    let tzf = trimble_tzf::Tzf::open(std::path::Path::new(&path)).unwrap();
    let meta = tzf.scan_meta().unwrap();
    assert!(meta.levelling.is_applied(), "levelling: {:?}", meta.levelling);
    let pts = tzf.points().unwrap();
    assert_eq!(pts.len() as u64, meta.n_points as u64);
    for p in &pts {
        let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt();
        assert!(r > 0.1 && r < 400.0, "range {r} out of bounds");
    }
}
