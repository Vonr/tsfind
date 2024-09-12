use clap::ValueEnum;
use tree_sitter::Language as TSLanguage;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Language {
    #[cfg(feature = "rust")]
    Rust,
    #[cfg(feature = "go")]
    Go,
    #[cfg(feature = "javascript")]
    JS,
    #[cfg(feature = "typescript")]
    TS,
    #[cfg(feature = "typescript")]
    TSX,
    #[cfg(feature = "php")]
    PHP,
    #[cfg(feature = "php")]
    PHPOnly,
}

impl Language {
    pub fn name(self) -> &'static str {
        use Language::*;

        match self {
            #[cfg(feature = "rust")]
            Rust => "rust",
            #[cfg(feature = "go")]
            Go => "go",
            #[cfg(feature = "javascript")]
            JS => "js",
            #[cfg(feature = "typescript")]
            TS | TSX => "ts",
            #[cfg(feature = "php")]
            PHP => "php",
            #[cfg(feature = "php")]
            PHPOnly => "php",
        }
    }

    pub fn ts_lang(self) -> TSLanguage {
        use Language::*;
        match self {
            #[cfg(feature = "rust")]
            Rust => tree_sitter_rust::LANGUAGE,
            #[cfg(feature = "go")]
            Go => tree_sitter_go::LANGUAGE,
            #[cfg(feature = "javascript")]
            JS => tree_sitter_javascript::LANGUAGE,
            #[cfg(feature = "typescript")]
            TS => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            #[cfg(feature = "typescript")]
            TSX => tree_sitter_typescript::LANGUAGE_TSX,
            #[cfg(feature = "php")]
            PHP => tree_sitter_php::LANGUAGE_PHP,
            #[cfg(feature = "php")]
            PHPOnly => tree_sitter_php::LANGUAGE_PHP_ONLY,
        }
        .into()
    }
}
