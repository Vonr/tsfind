use bstr::ByteSlice;
use clap::Parser;
use color_eyre::{
    eyre::{eyre, Result},
    Section,
};
use ignore::{types::TypesBuilder, WalkBuilder};
use memmap2::Mmap;
use parking_lot::Mutex;
use serde::ser::SerializeStruct;
use std::{
    collections::HashSet,
    fs::File,
    io::{BufWriter, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Serialize, Serializer};
use tree_sitter::{Language as TSLanguage, Parser as TSParser, Point, Query, QueryCursor};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(value_parser = supported_language)]
    language: Language,
    query_file: PathBuf,
    paths: Vec<PathBuf>,

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
}

fn supported_language(language_name: &str) -> Result<Language> {
    Ok(match language_name.to_lowercase().as_str() {
        "rust" => Language::new("rust", tree_sitter_rust::language()),
        "go" => Language::new("go", tree_sitter_go::language()),
        "js" | "javascript" => Language::new("js", tree_sitter_javascript::language()),
        "ts" | "typescript" => Language::new("ts", tree_sitter_typescript::language_typescript()),
        "tsx" => Language::new("ts", tree_sitter_typescript::language_tsx()),
        "php" => Language::new("php", tree_sitter_php::language_php()),
        "phponly" => Language::new("php", tree_sitter_php::language_php_only()),
        _ => return Err(eyre!("unsupported language")),
    })
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let Args {
        language,
        query_file,
        mut paths,
        hidden,
        hidden_captures,
        only_text,
        separator,
    } = Args::parse();

    let query = match File::open(query_file) {
        Ok(f) => f,
        Err(e) => {
            return Err(eyre!("could not open query file").error(e));
        }
    };

    let query = match unsafe { Mmap::map(&query) } {
        Ok(q) => q,
        Err(e) => {
            return Err(eyre!("could not read query file").error(e));
        }
    };

    let Ok(query_src) = &query[..].to_str() else {
        return Err(eyre!("query is not UTF-8"));
    };

    let query = match Query::new(&language.ts_lang, query_src) {
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

    let out = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(Mutex::new(HashSet::new()));

    if paths.is_empty() {
        paths.push("./".into());
    }
    let num_paths = paths.len();

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
                num_paths.min(
                    std::thread::available_parallelism()
                        .map(NonZeroUsize::get)
                        .unwrap_or(0),
                ),
            )
            .build_parallel()
            .run(|| {
                let out = out.clone();
                let done = done.clone();
                let ts_lang = language.ts_lang.clone();
                let query = Query::new(&ts_lang, query_src).unwrap();
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
                    if let Err(e) = parse(
                        path.clone(),
                        &mut parser,
                        &query,
                        query_captures,
                        out.clone(),
                        hidden_captures,
                    ) {
                        eprintln!("{e:?}");
                    }

                    done.lock().insert(path);
                    Continue
                })
            })
    }

    let mut out = Arc::into_inner(out).unwrap().into_inner();
    out.sort_unstable_by_key(|c| c.file.clone());

    let stdout = std::io::stdout().lock();
    let mut writer = BufWriter::new(stdout);
    if only_text {
        for capture in out {
            _ = writer.write_all(capture.text.as_bytes());
            _ = writer.write_all(separator.as_bytes());
        }
    } else {
        serde_json::to_writer_pretty(&mut writer, &out).expect("could not write to stdout");
    }
    let _ = writer.write_all(b"\n");

    Ok(())
}

fn parse(
    path: Arc<Path>,
    parser: &mut TSParser,
    query: &Query,
    query_captures: &'static [&'static mut str],
    out: Arc<Mutex<Vec<Capture>>>,
    hidden_captures: bool,
) -> Result<()> {
    let Ok(file) = std::fs::File::open(path.clone()) else {
        return Err(eyre!("{path:?} failed to read file"));
    };

    let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
        return Err(eyre!("{path:?}: could not read file, skipping"));
    };
    let buf = &mmap[..];
    let Ok(buf) = buf.to_str() else {
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

            out.lock().push(Capture {
                file: path.clone(),
                start: node.start_position(),
                end: node.end_position(),
                capture: query_captures[idx],
                text: text.to_string(),
            });
        }
    }

    parser.reset();
    Ok(())
}

struct Capture {
    file: Arc<Path>,
    start: Point,
    end: Point,
    capture: &'static str,
    text: String,
}

impl Serialize for Capture {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("Capture", 4)?;
        s.serialize_field("file", &self.file.to_string_lossy())?;
        s.serialize_field("start", &SerializablePoint(self.start))?;
        s.serialize_field("end", &SerializablePoint(self.end))?;
        s.serialize_field("capture", self.capture)?;
        s.serialize_field("text", self.text.as_str())?;
        s.end()
    }
}

struct SerializablePoint(Point);

impl Serialize for SerializablePoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("Point", 2)?;
        s.serialize_field("line", &self.0.row)?;
        s.serialize_field("column", &self.0.column)?;
        s.end()
    }
}
