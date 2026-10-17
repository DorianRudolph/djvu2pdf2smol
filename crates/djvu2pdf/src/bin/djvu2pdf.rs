use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use djvu2pdf::convert::{LayerCoding, Options, convert};
use djvu2pdf::mask::MergeDicts;

const USAGE: &str = "\
usage: djvu2pdf [options] IN.djvu [IN2.djvu ...] -o OUT(.pdf|DIR)
       djvu2pdf [options] DIR -o OUTDIR          (every *.djvu in DIR)

Layered DjVu -> PDF: bit-exact JBIG2 masks, JPEG 2000 colour layers, invisible text,
outline and links.

options:
  -o, --output PATH      output file, or directory for several inputs
  -j, --jobs N           worker threads (default: all cores)
  --bg CODING            background coding: rate:F = JPEG 2000 at F x the DjVu layer's
                         bytes (default rate:1), lossless
  --fg CODING            FG44 foreground coding (default rate:2)
  --merge-dicts MODE     auto (default), always or never: one JBIG2Globals for all
                         DjVu shape dictionaries
  --no-refine            no JBIG2 refinement coding (larger, simpler streams)
  --search               per-page JBIG2 text-region parameter search (~0.1% smaller)
  --no-text, --no-outline, --no-links
  -q, --quiet
  -h, --help";

fn coding(s: &str) -> Result<LayerCoding, String> {
    match s.split_once(':') {
        _ if s == "lossless" => Ok(LayerCoding::Lossless),
        Some(("rate", f)) => f
            .parse()
            .map(LayerCoding::Rate)
            .map_err(|_| format!("bad rate factor {f:?}")),
        _ => Err(format!(
            "unknown layer coding {s:?} (use rate:F or lossless)"
        )),
    }
}

struct Args {
    inputs: Vec<PathBuf>,
    output: PathBuf,
    options: Options,
    quiet: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut options = Options {
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
        ..Default::default()
    };
    let (mut inputs, mut output, mut quiet) = (Vec::new(), None, false);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or(format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-o" | "--output" => output = Some(PathBuf::from(value()?)),
            "-j" | "--jobs" => options.jobs = value()?.parse().map_err(|_| "bad --jobs")?,
            "--bg" => options.background = coding(&value()?)?,
            "--fg" => options.foreground = coding(&value()?)?,
            "--merge-dicts" => {
                options.merge_dicts = match value()?.as_str() {
                    "auto" => MergeDicts::Auto,
                    "always" => MergeDicts::Always,
                    "never" => MergeDicts::Never,
                    v => return Err(format!("bad --merge-dicts {v:?}")),
                }
            }
            "--no-refine" => options.jbig2.refine = false,
            "--search" => options.jbig2.search = true,
            "--no-text" => options.text = false,
            "--no-outline" => options.outline = false,
            "--no-links" => options.links = false,
            "-q" | "--quiet" => quiet = true,
            f if f.starts_with('-') && f.len() > 1 => return Err(format!("unknown option {f}")),
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    let output = output.ok_or("missing -o OUTPUT")?;
    let mut files = Vec::new();
    for i in inputs {
        if i.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&i)
                .map_err(|e| format!("{}: {e}", i.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().is_some_and(|x| {
                        x.eq_ignore_ascii_case("djvu") || x.eq_ignore_ascii_case("djv")
                    })
                })
                .collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(i);
        }
    }
    if files.is_empty() {
        return Err("no input files".into());
    }
    Ok(Args {
        inputs: files,
        output,
        options,
        quiet,
    })
}

fn convert_file(src: &Path, dst: &Path, options: &Options) -> Result<String, String> {
    let t0 = Instant::now();
    let data = std::fs::read(src).map_err(|e| e.to_string())?;
    let tmp = dst.with_extension("pdf.part");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let result = convert(&data, BufWriter::with_capacity(1 << 20, file), options);
    let (_, stats) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.to_string());
        }
    };
    std::fs::rename(&tmp, dst).map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(dst).map_err(|e| e.to_string())?.len();
    Ok(format!(
        "{} pages ({} with colour layers, {} with text), {} -> {} bytes ({:.3}x), {} JBIG2 dictionar{}{}, {} bookmarks, {} links, {:.1}s",
        stats.pages,
        stats.compound_pages,
        stats.text_pages,
        data.len(),
        bytes,
        bytes as f64 / data.len() as f64,
        stats.jbig2_dictionaries,
        if stats.jbig2_dictionaries == 1 {
            "y"
        } else {
            "ies"
        },
        if stats.jbig2_merged { " (merged)" } else { "" },
        stats.bookmarks,
        stats.links,
        t0.elapsed().as_secs_f64()
    ))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("djvu2pdf: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let multi = args.inputs.len() > 1
        || args.output.is_dir()
        || args.output.to_string_lossy().ends_with('/');
    if multi && let Err(e) = std::fs::create_dir_all(&args.output) {
        eprintln!("djvu2pdf: {}: {e}", args.output.display());
        return ExitCode::FAILURE;
    }
    let mut code = ExitCode::SUCCESS;
    for src in &args.inputs {
        let dst = if multi {
            args.output
                .join(src.file_stem().unwrap_or_default())
                .with_extension("pdf")
        } else {
            args.output.clone()
        };
        match convert_file(src, &dst, &args.options) {
            Ok(msg) if !args.quiet => println!("{}: {msg}", src.display()),
            Ok(_) => {}
            Err(e) => {
                eprintln!("djvu2pdf: {}: {e}", src.display());
                code = ExitCode::FAILURE;
            }
        }
    }
    code
}
