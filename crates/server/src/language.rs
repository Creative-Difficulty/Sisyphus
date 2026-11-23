//! The programming language of an edited file, from its name. Only the
//! file's path relative to the project is uploaded; this looks at its name
//! and extension.

/// The language's display name, or "Other".
pub fn of(path: &str) -> &'static str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name {
        "Dockerfile" | "Containerfile" => return "Dockerfile",
        "Makefile" | "GNUmakefile" | "makefile" => return "Makefile",
        "CMakeLists.txt" => return "CMake",
        "Cargo.lock" | "uv.lock" | "poetry.lock" | "Pipfile.lock" => return "TOML",
        "Gemfile" | "Rakefile" => return "Ruby",
        "Justfile" | "justfile" => return "Just",
        _ => {}
    }
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return "Other";
    };
    if stem.is_empty() {
        // Dotfiles like `.gitignore` or `.env`.
        return "Other";
    }
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "Rust",
        "py" | "pyi" | "pyw" => "Python",
        "ipynb" => "Jupyter",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "Sass",
        "less" => "Less",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "astro" => "Astro",
        "c" => "C",
        "h" => "C",
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "ino" => "C++",
        "cs" => "C#",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "scala" | "sc" => "Scala",
        "groovy" | "gradle" => "Groovy",
        "go" => "Go",
        "swift" => "Swift",
        "m" | "mm" => "Objective-C",
        "rb" | "erb" => "Ruby",
        "php" => "PHP",
        "lua" => "Lua",
        "luau" => "Luau",
        "dart" => "Dart",
        "zig" => "Zig",
        "nim" => "Nim",
        "hs" => "Haskell",
        "ml" | "mli" => "OCaml",
        "fs" | "fsx" => "F#",
        "ex" | "exs" | "heex" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "clj" | "cljs" | "cljc" | "edn" => "Clojure",
        "el" => "Emacs Lisp",
        "lisp" | "lsp" | "scm" | "rkt" => "Lisp",
        "jl" => "Julia",
        "r" => "R",
        "pl" | "pm" => "Perl",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "bat" | "cmd" => "Batch",
        "sql" => "SQL",
        "gd" => "GDScript",
        "glsl" | "vert" | "frag" | "wgsl" | "hlsl" | "shader" => "Shaders",
        "asm" | "s" => "Assembly",
        "v" | "sv" | "vhd" | "vhdl" => "HDL",
        "nix" => "Nix",
        "tf" => "Terraform",
        "proto" => "Protobuf",
        "graphql" | "gql" => "GraphQL",
        "md" | "mdx" | "markdown" => "Markdown",
        "rst" => "reStructuredText",
        "tex" | "sty" | "bib" => "TeX",
        "typ" => "Typst",
        "txt" => "Text",
        "json" | "jsonc" | "json5" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "xml" | "xsd" | "xsl" | "plist" => "XML",
        "ini" | "cfg" | "conf" => "Config",
        "csv" | "tsv" => "CSV",
        "svg" => "SVG",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::of;

    #[test]
    fn names_and_extensions() {
        assert_eq!(of("crates/server/src/main.rs"), "Rust");
        assert_eq!(of("web/App.TSX"), "TSX");
        assert_eq!(of("Dockerfile"), "Dockerfile");
        assert_eq!(of("deploy/Makefile"), "Makefile");
        assert_eq!(of("src\\lib.py"), "Python");
        assert_eq!(of(".gitignore"), "Other");
        assert_eq!(of("LICENSE"), "Other");
        assert_eq!(of("notes.unknown"), "Other");
    }
}
