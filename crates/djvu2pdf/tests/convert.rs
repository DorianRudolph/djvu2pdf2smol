//! End-to-end conversion of the djvu-rs test fixtures (vendor/djvu-rs/tests/fixtures).

use djvu2pdf::convert::{Options, convert};

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/djvu-rs/tests/fixtures/"
    )
    .to_string()
        + name;
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e} (git submodule update --init?)"))
}

fn run(name: &str, jobs: usize) -> (Vec<u8>, djvu2pdf::convert::Stats) {
    let options = Options {
        jobs,
        ..Default::default()
    };
    convert(&fixture(name), Vec::new(), &options).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn fixtures_convert() {
    // (file, pages, bookmarks, links)
    for (name, pages, bookmarks, links) in [
        ("boy_jb2.djvu", 1, 0, 0),
        ("boy_jb2_rotate90.djvu", 1, 0, 0),
        ("boy.djvu", 1, 0, 0),
        ("chicken_crcbhalf.djvu", 1, 0, 0),
        ("irish.djvu", 1, 0, 0),
        ("legacy_bm44.djvu", 1, 0, 0),
        ("navm_fgbz.djvu", 6, 6, 20),
        ("DjVu3Spec_bundled.djvu", 71, 100, 0),
        ("czech.djvu", 85, 0, 49),
    ] {
        let (pdf, stats) = run(name, 4);
        assert!(
            pdf.starts_with(b"%PDF-1.7") && pdf.ends_with(b"%%EOF\n"),
            "{name}"
        );
        assert_eq!(
            (stats.pages, stats.bookmarks, stats.links),
            (pages, bookmarks, links),
            "{name}"
        );
    }
}

#[test]
fn output_does_not_depend_on_threads() {
    for name in ["navm_fgbz.djvu", "DjVu3Spec_bundled.djvu"] {
        assert!(run(name, 1).0 == run(name, 4).0, "{name}");
    }
}
