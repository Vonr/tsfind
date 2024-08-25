use bstr::ByteSlice;
use clap::Parser;
use color_eyre::{
    eyre::{eyre, Result},
    Section,
};
use ignore::{types::TypesBuilder, WalkBuilder};
use memmap2::Mmap;
use parking_lot::Mutex;
use std::{
    collections::HashSet,
    fmt::Write,
    fs::File,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use tree_sitter::{Language as TSLanguage, Parser as TSParser, Query, QueryCursor};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(value_parser = Language::get)]
    language: Language,
    paths: Vec<PathBuf>,

    #[arg(short = 'q', long, help = "The query to find matches for")]
    query: Option<Box<str>>,

    #[arg(
        short = 'Q',
        long,
        help = "The file containing the query to find matches for"
    )]
    query_file: Option<PathBuf>,

    #[arg(short = 'H', long, help = "Recurse into hidden files and directories")]
    hidden: bool,

    #[arg(short = 'C', long, help = "Show captures starting with '_'")]
    hidden_captures: bool,

    #[arg(short = 't', long, help = "Only report captured text")]
    only_text: bool,

    #[arg(
        short = 's',
        long,
        help = "Separator for matches, only useful with --only-text/-t",
        value_parser = unescaper::unescape,
    )]
    #[cfg_attr(windows, arg(default_value_t = String::from("\r\n")))]
    #[cfg_attr(not(windows), arg(default_value_t = String::from("\n")))]
    separator: String,
}

#[derive(Clone, Debug)]
struct Language {
    name: &'static str,
    ts_lang: TSLanguage,
}

impl Language {
    pub fn new(name: &'static str, ts_lang: TSLanguage) -> Self {
        Self { ts_lang, name }
    }

    pub fn get(language_name: &str) -> Result<Self> {
        Ok(match language_name.to_lowercase().as_str() {
            #[cfg(feature = "rust")]
            "rust" => Language::new("rust", tree_sitter_rust::language()),
            #[cfg(feature = "go")]
            "go" => Language::new("go", tree_sitter_go::language()),
            #[cfg(feature = "javascript")]
            "js" | "javascript" => Language::new("js", tree_sitter_javascript::language()),
            #[cfg(feature = "typescript")]
            "ts" | "typescript" => {
                Language::new("ts", tree_sitter_typescript::language_typescript())
            }
            #[cfg(feature = "typescript")]
            "tsx" => Language::new("ts", tree_sitter_typescript::language_tsx()),
            #[cfg(feature = "php")]
            "php" => Language::new("php", tree_sitter_php::language_php()),
            #[cfg(feature = "php")]
            "phponly" => Language::new("php", tree_sitter_php::language_php_only()),
            _ => return Err(eyre!("unsupported language")),
        })
    }
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
        separator,
    } = Args::parse();

    let query_src = match (query, query_file) {
        (None, None) => return Err(eyre!("specify either a query or query file with -q/-Q")),
        (Some(..), Some(..)) => {
            return Err(eyre!("only specify one query or query file with -q/-Q"))
        }
        (Some(query), None) => Box::leak(query),
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

    let query = match Query::new(&language.ts_lang, &query_src) {
        Ok(q) => q,
        Err(e) => return Err(eyre!("error parsing query").error(e)),
    };

    let query_captures: &'static [&'static mut str] = Box::leak(
        query
            .capture_names()
            .iter()
            .map(|n| n.to_string().leak())
            .collect::<Box<[_]>>(),
    );

    let out = Arc::new(Mutex::new(String::new()));
    let done = Arc::new(Mutex::new(HashSet::new()));

    if paths.is_empty() {
        paths.push("./".into());
    }

    let separator = separator.leak() as &'static str;

    for path in paths {
        WalkBuilder::new(path)
            .hidden(hidden)
            .types(
                TypesBuilder::new()
                    .add_defaults()
                    .select(language.name)
                    .build()?,
            )
            .threads(
                std::thread::available_parallelism()
                    .map(NonZeroUsize::get)
                    .unwrap_or(1),
            )
            .build_parallel()
            .run(|| {
                let out = out.clone();
                let done = done.clone();
                let ts_lang = language.ts_lang.clone();
                let query = Query::new(&ts_lang, &query_src).unwrap();
                Box::new(move |file| {
                    use ignore::WalkState::*;

                    let Ok(file) = file else {
                        return Skip;
                    };

                    if !file.file_type().map(|f| f.is_file()).unwrap_or(false)
                        || done.lock().contains(file.path())
                    {
                        return Continue;
                    }

                    let mut parser = TSParser::new();
                    if let Err(e) = parser.set_language(&ts_lang) {
                        unreachable!("could not create parser: {e:?}");
                    }

                    let path: Arc<Path> = file.path().into();
                    done.lock().insert(path.clone());
                    if let Err(e) = parse(
                        path,
                        &mut parser,
                        &query,
                        query_captures,
                        out.clone(),
                        hidden_captures,
                        only_text,
                        &separator,
                    ) {
                        eprintln!("{e:?}");
                    }

                    Continue
                })
            })
    }

    let out = Arc::into_inner(out).unwrap().into_inner();
    if only_text {
        println!("{out}");
    } else {
        println!("[{out}]");
    }

    Ok(())
}

fn parse(
    path: Arc<Path>,
    parser: &mut TSParser,
    query: &Query,
    query_captures: &'static [&'static mut str],
    out: Arc<Mutex<String>>,
    hidden_captures: bool,
    only_text: bool,
    separator: &'static str,
) -> Result<()> {
    let Ok(file) = std::fs::File::open(path.clone()) else {
        return Err(eyre!("{path:?} failed to read file"));
    };

    let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
        return Err(eyre!("{path:?}: could not read file, skipping"));
    };
    let Ok(buf) = mmap[..].to_str() else {
        // skip this file
        return Ok(());
    };

    let tree = parser.parse(buf, None);
    let Some(tree) = tree else {
        return Err(eyre!("{path:?}: failed to parse file"));
    };

    let mut cursor = QueryCursor::new();

    for (captures, idx) in cursor.captures(query, tree.root_node(), buf.as_bytes()) {
        if !hidden_captures && query_captures[idx].starts_with("_") {
            continue;
        }

        let nodes = captures.nodes_for_capture_index(idx as u32);

        for node in nodes {
            let Ok(text) = node.utf8_text(buf.as_bytes()) else {
                return Err(eyre!("{path:?}: file contents of are not valid UTF-8"));
            };

            let mut out = out.lock();
            if only_text {
                write!(*out, "{text}{separator}")?;
            } else {
                if !out.is_empty() {
                    out.push(',');
                }

                let start = node.start_position();
                let end = node.end_position();

                write!(
                    *out,
                    r#"{{"file":{file:?},"start":{{"row":{srow},"column":{scol}}},"end":{{"row":{erow},"column":{ecol}}},"capture":{capture:?},"text":{text:?}}}"#,
                    file = path.to_string_lossy().trim_start_matches("./"),
                    srow = start.row,
                    scol = start.column,
                    erow = end.row,
                    ecol = end.column,
                    capture = query_captures[idx],
                )?;
            }
        }
    }

    parser.reset();
    Ok(())
}
