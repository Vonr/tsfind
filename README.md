# tsfind

![Demo](https://github.com/user-attachments/assets/26255220-c94a-419d-ab8b-a0b20f6ab7c2)

Extract code using [tree-sitter](https://tree-sitter.github.io/tree-sitter/) queries.

Inspired by [helixbass/tree-sitter-grep](https://github.com/helixbass/tree-sitter-grep) which reports entire lines instead of just the captures.

Quite WIP, command line interface should be considered unstable - use at your own risk.

# Usage

```
Extract code using tree-sitter queries

Usage: tsfind [OPTIONS] <LANGUAGE> [PATHS]...

Arguments:
  <LANGUAGE>  [possible values: rust, go, js, ts, tsx, php, php-only]
  [PATHS]...  

Options:
  -q, --query <QUERY>            The query to find matches for
  -Q, --query-file <QUERY_FILE>  The file containing the query to find matches for
  -H, --hidden                   Recurse into hidden files and directories
  -C, --hidden-captures          Show captures starting with '_'
  -t, --only-text                Only report captured text
  -l, --list                     Only report files with matches
  -s, --separator <SEPARATOR>    Separator for matches, only useful with --only-text/-t [default: "\n"]
  -h, --help                     Print help
  -V, --version                  Print version
```


