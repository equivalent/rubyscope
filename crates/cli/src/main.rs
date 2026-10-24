//! rubyscope-cli: builds the index and answers questions about it, so
//! resolution can be tested without the window.

mod check;
mod golden;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rubyscope_index::{BuildOptions, FileId, Index, Loc};

#[derive(Parser)]
#[command(name = "rubyscope-cli", about = "Test tool for the Rubyscope index")]
struct Cli {
    /// The project folder.
    #[arg(long, short, default_value = ".")]
    root: PathBuf,
    /// Do not index gems.
    #[arg(long)]
    no_gems: bool,
    /// Do not index the Ruby standard library.
    #[arg(long)]
    no_stdlib: bool,
    /// Do not read or write the saved index.
    #[arg(long)]
    no_cache: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the index and print counts and times.
    Index,
    /// Print where the symbol at FILE:LINE:COL is defined (1-based).
    Def { position: String },
    /// Print every usage of the symbol at FILE:LINE:COL.
    Refs { position: String },
    /// Print what a hover at FILE:LINE:COL shows.
    Hover { position: String },
    /// Print the outline of a file.
    Outline { file: PathBuf },
    /// Search the text of the project.
    Search {
        pattern: String,
        #[arg(long)]
        regex: bool,
    },
    /// Print the view of the controller action at FILE:LINE:COL, or the
    /// controller action of a view.
    Related { position: String },
    /// Print the ancestors of a class or module.
    Ancestors { name: String },
    /// Run golden click tests: lines of `FILE:LINE:COL => TARGET:LINE`.
    Golden { file: PathBuf },
    /// Compare the index with the JSON from probe/rails_probe.rb.
    Check {
        probe: PathBuf,
        /// How many misses to list per group.
        #[arg(long, default_value_t = 15)]
        show: usize,
    },
}

// jemalloc builds and loads the index faster than the system allocator.
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opts = BuildOptions { gems: !cli.no_gems, stdlib: !cli.no_stdlib };
    let cache_dir = rubyscope_index::cache::default_dir().filter(|_| !cli.no_cache);
    let index = match &cache_dir {
        Some(dir) => Index::load(&cli.root, &opts, dir)?,
        None => Index::build(&cli.root, &opts)?,
    };
    if let Some(dir) = &cache_dir
        && !index.stats.same_as_saved
    {
        index.save(dir)?;
    }
    match cli.command {
        Command::Index => print_stats(&index),
        Command::Def { position } => def(&index, &position)?,
        Command::Check { probe, show } => check::check(&index, &probe, show)?,
        Command::Golden { file } => {
            if !golden::run(&index, &file)? {
                std::process::exit(1);
            }
        }
        Command::Refs { position } => {
            let (file, offset) = parse_position(&index, &position)?;
            let Some(sym) = index.symbol_at(file, offset) else {
                println!("no symbol here");
                return Ok(());
            };
            let usages = index.usages(&sym);
            println!("{}: {} usages", sym.label(), usages.len());
            for u in usages.iter().take(40) {
                let line = index.files[u.file as usize].raw.line_of(u.span.start) + 1;
                println!("  {}:{line}  in {}  ({})", index.display_path(u.file), u.context, u.confidence.label());
            }
        }
        Command::Hover { position } => {
            let (file, offset) = parse_position(&index, &position)?;
            match index.hover(file, offset) {
                Some(h) => {
                    println!("{}  ({})", h.title, h.confidence.label());
                    for l in [h.signature, h.receiver, Some(h.location), h.doc].into_iter().flatten() {
                        println!("  {l}");
                    }
                }
                None => println!("nothing here"),
            }
        }
        Command::Outline { file } => {
            let id = index.file_id(&file).context("file is not in the index")?;
            for item in index.outline(id) {
                println!("{}{} [{:?}]", "  ".repeat(item.depth as usize), item.label, item.kind);
            }
        }
        Command::Search { pattern, regex } => {
            let hits = index.search(&pattern, regex, false, 50).map_err(anyhow::Error::msg)?;
            for h in &hits {
                println!("{}:{}  {}", h.rel, h.line + 1, h.text.trim());
            }
            println!("{} hits", hits.len());
        }
        Command::Related { position } => {
            let (file, offset) = parse_position(&index, &position)?;
            match index.related_at(file, offset) {
                Some((label, targets)) => {
                    println!("{label}");
                    for t in &targets {
                        println!("  {}  {}", format_loc(&index, &t.loc), t.label);
                    }
                }
                None => println!("nothing related"),
            }
        }
        Command::Ancestors { name } => {
            for a in index.ancestors(&name).iter() {
                println!("{a}");
            }
        }
    }
    // Freeing the index takes a while and the process ends anyway.
    std::mem::forget(index);
    Ok(())
}

fn print_stats(index: &Index) {
    let s = &index.stats;
    println!("sources       {}", s.sources);
    println!("files         {} ({} in the project)", s.files, s.project_files);
    println!("bytes         {:.1} MB", s.bytes as f64 / 1e6);
    println!("parse errors  {} files", s.parse_errors);
    println!("constants     {}", index.consts.len());
    println!("namespaces    {}", index.namespaces.len());
    println!("methods       {}", index.methods.len());
    println!("refs          {}", index.files.iter().map(|f| f.refs().len()).sum::<usize>());
    println!("reused        {} files from the saved index", s.reused);
    println!("time          discover {:?}, parse {:?}, link {:?}", s.discover, s.parse, s.link);
}

/// Parses `path:line:col` (1-based line and column, column in characters).
pub fn parse_position(index: &Index, position: &str) -> Result<(FileId, u32)> {
    let mut parts = position.rsplitn(3, ':');
    let col: usize = parts.next().context("missing column")?.parse()?;
    let line: usize = parts.next().context("missing line")?.parse()?;
    let path = parts.next().context("missing path")?;
    let file = index.file_id(Path::new(path)).with_context(|| format!("{path} is not in the index"))?;
    let source = std::fs::read_to_string(&index.files[file as usize].path)?;
    let Some(line_text) = source.split_inclusive('\n').nth(line - 1) else { bail!("no line {line}") };
    let line_start = index.files[file as usize].raw.line_starts[line - 1];
    let byte_col: usize = line_text.chars().take(col - 1).map(char::len_utf8).sum();
    Ok((file, line_start + byte_col as u32))
}

pub fn format_loc(index: &Index, loc: &Loc) -> String {
    let raw = &index.files[loc.file as usize].raw;
    let (line, col) = raw.line_col(loc.name_span.start);
    format!("{}:{}:{}", index.display_path(loc.file), line + 1, col + 1)
}

fn def(index: &Index, position: &str) -> Result<()> {
    let (file, offset) = parse_position(index, position)?;
    let Some(res) = index.definition_at(file, offset) else {
        println!("nothing to resolve here");
        return Ok(());
    };
    let source = std::fs::read(&index.files[file as usize].path)?;
    let clicked = String::from_utf8_lossy(&source[res.span.start as usize..res.span.end as usize]);
    println!("clicked: {clicked}");
    if let Some((ty, conf)) = &res.receiver_type {
        println!("receiver: {ty} ({})", conf.label());
    }
    if res.targets.is_empty() {
        println!("no definition found");
    }
    for t in &res.targets {
        let detail = t.detail.as_deref().map(|d| format!(" [{d}]")).unwrap_or_default();
        println!("{}  {}{detail}  ({})", format_loc(index, &t.loc), t.label, t.confidence.label());
        println!(
            "    {}:{}",
            index.stable_path(t.loc.file),
            index.files[t.loc.file as usize].raw.line_col(t.loc.span.start).0 + 1
        );
    }
    Ok(())
}
