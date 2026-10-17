use std::io::{BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use djvu2pdf::convert::{LayerCoding, Options, convert_with_progress};
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
  -q, --quiet            no summary lines and no progress bar (the bar is shown
                         only when stderr is a terminal)
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

/// A one-line page progress bar on stderr, redrawn at most every 100 ms.
struct Progress {
    label: String,
    start: Instant,
    last: Option<Instant>,
}

impl Progress {
    fn new(src: &Path) -> Self {
        let name = src.file_name().unwrap_or_default().to_string_lossy();
        let label: String = if name.chars().count() > 30 {
            name.chars().take(29).chain(['…']).collect()
        } else {
            name.into_owned()
        };
        Self {
            label,
            start: Instant::now(),
            last: None,
        }
    }

    fn update(&mut self, done: usize, total: usize) {
        let now = Instant::now();
        if done < total
            && self
                .last
                .is_some_and(|t| now.duration_since(t).as_millis() < 100)
        {
            return;
        }
        self.last = Some(now);
        const WIDTH: usize = 30;
        let filled = (done * WIDTH).checked_div(total).unwrap_or(WIDTH);
        let secs = self.start.elapsed().as_secs_f64();
        let rate = if secs > 0.0 { done as f64 / secs } else { 0.0 };
        let eta = if rate > 0.0 && done < total {
            format!(", {:.0}s left", (total - done) as f64 / rate)
        } else {
            String::new()
        };
        let mut err = std::io::stderr().lock();
        let _ = write!(
            err,
            "\r\x1b[2K{} [{}{}] {done}/{total} pages, {rate:.0}/s{eta}",
            self.label,
            "#".repeat(filled),
            "-".repeat(WIDTH - filled)
        );
        let _ = err.flush();
    }

    fn clear(&self) {
        if self.last.is_some() {
            let _ = write!(std::io::stderr(), "\r\x1b[2K");
        }
    }
}

fn convert_file(
    src: &Path,
    dst: &Path,
    options: &Options,
    show_progress: bool,
) -> Result<String, String> {
    let t0 = Instant::now();
    let data = std::fs::read(src).map_err(|e| e.to_string())?;
    let mut tmp = dst.as_os_str().to_os_string();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut progress = Progress::new(src);
    let result = convert_with_progress(
        &data,
        BufWriter::with_capacity(1 << 20, file),
        options,
        |done, total| {
            if show_progress {
                progress.update(done, total)
            }
        },
    );
    progress.clear();
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
    let show_progress = !args.quiet && std::io::stderr().is_terminal();
    let mut code = ExitCode::SUCCESS;
    for src in &args.inputs {
        let dst = if multi {
            // append rather than `with_extension`: stems may contain dots ("D. Pedoe")
            let mut name = src.file_stem().unwrap_or_default().to_os_string();
            name.push(".pdf");
            args.output.join(name)
        } else {
            args.output.clone()
        };
        match convert_file(src, &dst, &args.options, show_progress) {
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
