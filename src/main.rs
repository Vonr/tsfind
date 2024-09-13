use bstr::ByteSlice;
use clap::Parser;
use color_eyre::{
    eyre::{eyre, Result},
    Section,
};
use ignore::{types::TypesBuilder, WalkBuilder};
use language::Language;
use memmap2::Mmap;
use parking_lot::Mutex;
use std::{borrow::Cow, fmt::Write};
use std::{
    convert::Infallible, fs::File, num::NonZeroUsize, os::unix::ffi::OsStrExt, path::Path,
    sync::Arc,
};

use tree_sitter::{Language as TSLanguage, Parser as TSParser, Query, QueryCursor};

mod language;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    language: Language,
    paths: Vec<Box<Path>>,

    #[arg(short = 'q', long, help = "The query to find matches for", value_parser = leak_str)]
    query: Option<&'static str>,

    #[arg(
        short = 'Q',
        long,
        help = "The file containing the query to find matches for"
    )]
    query_file: Option<Box<Path>>,

    #[arg(short = 'H', long, help = "Recurse into hidden files and directories")]
    hidden: bool,

    #[arg(short = 'C', long, help = "Show captures starting with '_'")]
    hidden_captures: bool,

    #[arg(short = 't', long, help = "Only report captured text")]
    only_text: bool,

    #[arg(short = 'l', long, help = "Only report files with matches")]
    list: bool,

    #[arg(
        short = 's',
        long,
        help = "Separator for matches, only useful with --only-text/-t",
        value_parser = unescape_and_leak_str,
    )]
    #[cfg_attr(windows, arg(default_value_t = "\r\n"))]
    #[cfg_attr(not(windows), arg(default_value_t = "\n"))]
    separator: &'static str,
}

fn leak_str(s: &str) -> Result<&'static str, Infallible> {
    Ok(Box::<str>::leak(<Box<str>>::from(s)))
}

fn unescape_and_leak_str(s: &str) -> Result<&'static str, unescaper::Error> {
    unescaper::unescape(s).map(|s| s.leak() as _)
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let Args {
        language,
        query,
        query_file,
        mut paths,
        hidden,
        hidden_captures,
        only_text,
        list,
        separator,
    } = Args::parse();

    let query_src = match (query, query_file) {
        (None, None) => return Err(eyre!("specify either a query or query file with -q/-Q")),
        (Some(..), Some(..)) => {
            return Err(eyre!("only specify one query or query file with -q/-Q"))
        }
        (Some(query), None) => query,
        (None, Some(query_file)) => match File::open(query_file) {
            Ok(f) => match unsafe { Mmap::map(&f) } {
                Ok(s) => match s[..].to_str() {
                    Ok(s) => Box::leak(<Box<str>>::from(s)),
                    Err(e) => return Err(eyre!("could not read query file").error(e)),
                },
                Err(e) => {
                    return Err(eyre!("could not read query file").error(e));
                }
            },
            Err(e) => {
                return Err(eyre!("could not open query file").error(e));
            }
        },
    };

    if query_src.is_empty() {
        if !list && !only_text {
            println!("[]");
        }
        return Ok(());
    }

    let query = match Query::new(&language.ts_lang(), query_src) {
        Ok(q) => q,
        Err(e) => return Err(eyre!("error parsing query").error(e)),
    };

    let query_captures: &'static [&'static str] = Box::leak(
        query
            .capture_names()
            .iter()
            .map(|&n| Box::leak(<Box<str>>::from(n)) as _)
            .collect::<Box<[_]>>(),
    );

    let out = Arc::new(Mutex::new(String::new()));

    if paths.is_empty() {
        paths.push(Path::new("./").into());
    }

    let query = Box::leak(Box::new(query)) as &'static _;

    let types = TypesBuilder::new()
        .add_defaults()
        .select(language.name())
        .build()?;

    let threads = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1);

    for path in paths {
        WalkBuilder::new(path)
            .parents(!hidden)
            .hidden(!hidden)
            .ignore(!hidden)
            .git_global(!hidden)
            .git_ignore(!hidden)
            .git_exclude(!hidden)
            .types(types.clone())
            .threads(threads)
            .build_parallel()
            .run(|| {
                let out = out.clone();
                Box::new(move |file| {
                    use ignore::WalkState::*;

                    let Ok(file) = file else {
                        return Skip;
                    };

                    if !file.file_type().map(|f| f.is_file()).unwrap_or(false) {
                        return Continue;
                    }

                    if let Err(e) = parse(
                        file.path(),
                        &language.ts_lang(),
                        query,
                        query_captures,
                        out.clone(),
                        hidden_captures,
                        only_text,
                        list,
                        separator,
                    ) {
                        eprintln!("{e:?}");
                    }

                    Continue
                })
            })
    }

    let out = Arc::into_inner(out).unwrap().into_inner();
    if list || only_text {
        if out.is_empty() {
            return Ok(());
        }

        println!("{}", out.strip_suffix(separator).unwrap_or(&out));
    } else {
        println!("[{out}]");
    }

    Ok(())
}

fn parse(
    path: &Path,
    language: &TSLanguage,
    query: &Query,
    query_captures: &[&str],
    out: Arc<Mutex<String>>,
    hidden_captures: bool,
    only_text: bool,
    list: bool,
    separator: &str,
) -> Result<()> {
    let Ok(file) = std::fs::File::open(path) else {
        return Err(eyre!("{path:?}: failed to read file"));
    };

    let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
        return Err(eyre!("{path:?}: could not memmap file"));
    };

    let src = &mmap[..];

    let mut parser = TSParser::new();
    parser.set_language(language)?;

    let Some(tree) = parser.parse(src, None) else {
        return Err(eyre!("{path:?}: failed to parse file"));
    };

    let mut path_buf: Option<Cow<'_, str>> = None;
    let mut cursor = QueryCursor::new();

    for (captures, idx) in cursor.captures(query, tree.root_node(), src) {
        if !hidden_captures && query_captures[idx].starts_with("_") {
            continue;
        }

        let mut nodes = captures.nodes_for_capture_index(idx as u32);
        if list && nodes.next().is_some() {
            let path_bytes = path.as_os_str().as_bytes();
            write!(
                out.lock(),
                "{}{separator}",
                path_bytes
                    .strip_prefix(b"./")
                    .unwrap_or(path_bytes)
                    .to_str_lossy(),
            )?;
            return Ok(());
        }

        for node in nodes {
            let Ok(text) = node.utf8_text(src) else {
                eprintln!("{path:?}: found match that is not valid UTF-8");
                continue;
            };

            if only_text {
                write!(out.lock(), "{text}{separator}")?;
                continue;
            }

            let start = node.start_position();
            let end = node.end_position();

            let path_buf = path_buf.get_or_insert_with(|| {
                let path_bytes = path.as_os_str().as_bytes();
                path_bytes
                    .strip_prefix(b"./")
                    .unwrap_or(path_bytes)
                    .to_str_lossy()
            });

            let mut out = out.lock();
            if !out.is_empty() {
                out.push(',');
            }

            write!(
                out,
                r#"{{"file":{file:?},"start":{{"row":{srow},"column":{scol}}},"end":{{"row":{erow},"column":{ecol}}},"capture":{capture:?},"text":{text:?}}}"#,
                file = path_buf,
                srow = start.row,
                scol = start.column,
                erow = end.row,
                ecol = end.column,
                capture = query_captures[idx],
            )?;
        }
    }

    Ok(())
}
