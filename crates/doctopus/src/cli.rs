//! Command-line arguments.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use doctopus_core::{Address, CellRange};

/// Characters `read` prints per call unless told otherwise: a page fits in
/// the tool-output budget of common coding agents.
pub(crate) const DEFAULT_MAX_CHARS: usize = 25_000;
/// Embedded document levels parsed by default: deeper than real documents
/// nest, so everything is read; the engine's document budget bounds the
/// work on hostile files.
pub(crate) const DEFAULT_MAX_DEPTH: u32 = 32;
/// Embedded document levels shown inline by default.
pub(crate) const DEFAULT_EMBED_DEPTH: u32 = 8;

const AFTER_HELP: &str = "\
Addresses name a file or something inside it:
  report.docx             the document
  deck.pptx#emb1          embedded object emb1 (ids are listed by `doctopus info`)
  deck.pptx#emb1/img2     image img2 inside that object

Start with `doctopus info FILE`, then `doctopus read FILE`, or write
Markdown files to read later with `doctopus convert FILE`.
`doctopus guide` explains the whole workflow.";

const AFTER_LONG_HELP: &str = "\
Addresses name a file or something inside it:
  report.docx             the document
  deck.pptx#emb1          embedded object emb1 (ids are listed by `doctopus info`)
  deck.pptx#emb1/img2     image img2 inside that object
Quote addresses that contain `#`.

Exit codes:
  0  success
  1  the file could not be read (missing, damaged)
  2  invalid arguments (unknown option, no such part, sheet or cell range)
  3  unsupported file type
  4  encrypted file

Start with `doctopus info FILE`, then `doctopus read FILE`, or write
Markdown files to read later with `doctopus convert FILE`.
`doctopus guide` explains the whole workflow.";

/// Read Word, Excel, PowerPoint, Visio and PDF files, and everything
/// embedded in them, as Markdown, plain text or JSON.
#[derive(Debug, Parser)]
#[command(
    name = "doctopus",
    version,
    after_help = AFTER_HELP,
    after_long_help = AFTER_LONG_HELP,
    arg_required_else_help = true,
    max_term_width = 100
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Levels of embedded documents to parse (0 leaves them as opaque files).
    #[arg(long, global = true, value_name = "N", default_value_t = DEFAULT_MAX_DEPTH)]
    pub(crate) max_depth: u32,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Print a document, or a document embedded in it, as Markdown, text or JSON.
    ///
    /// Output is paged: at most --max-chars characters per call, with the
    /// command for the next page printed on stderr. Select sheets, slides,
    /// pages or cells to read less, or write everything to a file with -o.
    Read(ReadArgs),
    /// Summarize documents: format, properties, sheets, slides or pages,
    /// embedded parts and the size of the full output.
    Info(InfoArgs),
    /// Save the images, embedded documents and attached files of a document
    /// to a directory.
    ///
    /// Parts nested in an embedded document go to a subdirectory named after
    /// that document's id. Each saved file is listed with its address.
    Extract(ExtractArgs),
    /// Convert documents to Markdown files that are read without doctopus.
    ///
    /// Each document becomes NAME.md, next to the file unless -o is given,
    /// with its pictures and attached files in NAME.parts/. A NAME.md that
    /// is not an earlier conversion of the same file is left alone, and the
    /// next free name is used. Embedded documents are converted too, to any
    /// depth: each gets a directory named after its id inside its parent's
    /// parts, with its own Markdown file and parts. Links between the files
    /// are relative, so the output can be moved as a whole.
    Convert(ConvertArgs),
    /// List the file formats doctopus reads.
    Formats(FormatsArgs),
    /// Print a usage guide written for coding agents.
    Guide,
}

#[derive(Debug, Args)]
pub(crate) struct ReadArgs {
    /// File to read; `FILE#id` reads an embedded document on its own.
    #[arg(value_name = "ADDRESS")]
    pub(crate) address: String,

    /// Only these sheets, slides or pages: numbers, ranges or names
    /// (`2`, `1,3-5`, `Summary`, `Q*`).
    #[arg(
        long = "section",
        visible_aliases = ["sheet", "slide", "page"],
        value_name = "LIST",
        help_heading = "Selection"
    )]
    pub(crate) sections: Option<String>,

    /// Only these spreadsheet cells (`B7`, `A1:F50`, columns `B:D`, rows `5:40`).
    #[arg(long, value_name = "RANGE", help_heading = "Selection")]
    pub(crate) range: Option<CellRange>,

    /// Show spreadsheet formulas next to their values.
    #[arg(long, help_heading = "Selection")]
    pub(crate) formulas: bool,

    /// Output format.
    #[arg(
        long,
        short = 'f',
        value_enum,
        default_value_t = Format::Markdown,
        help_heading = "Output"
    )]
    pub(crate) format: Format,

    /// Characters per call; 0 prints everything.
    #[arg(long, value_name = "N", env = "DOCTOPUS_MAX_CHARS", help_heading = "Output",
          default_value_t = DEFAULT_MAX_CHARS)]
    pub(crate) max_chars: usize,

    /// Start at this character of the output (the next page's offset is
    /// printed at the end of each page).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        conflicts_with = "output",
        help_heading = "Output"
    )]
    pub(crate) offset: usize,

    /// Write the whole output to FILE and print where each sheet, slide or
    /// page starts in it.
    #[arg(long, short = 'o', value_name = "FILE", help_heading = "Output")]
    pub(crate) output: Option<PathBuf>,

    /// Also save images and embedded files to DIR; references in the output
    /// then point at the saved files.
    #[arg(long, value_name = "DIR", help_heading = "Output")]
    pub(crate) extract: Option<PathBuf>,

    /// Levels of embedded documents shown inline; deeper ones are referenced.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_EMBED_DEPTH, help_heading = "Output")]
    pub(crate) embed_depth: u32,

    /// Leave out the YAML header of Markdown output.
    #[arg(long, help_heading = "Output")]
    pub(crate) no_front_matter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// Markdown with a YAML header.
    #[value(alias = "md")]
    Markdown,
    /// The same content without Markdown syntax.
    #[value(alias = "txt")]
    Text,
    /// The full document model (never paged).
    Json,
}

impl Format {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Text => "txt",
            Self::Json => "json",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct InfoArgs {
    /// Files to summarize; `FILE#id` summarizes an embedded document.
    #[arg(value_name = "ADDRESS", required = true)]
    pub(crate) addresses: Vec<String>,

    /// Print JSON (an array when several addresses are given).
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ExtractArgs {
    /// File whose parts to save; `FILE#id` saves one part (and what it contains).
    #[arg(value_name = "ADDRESS")]
    pub(crate) address: String,

    #[arg(
        long,
        short = 'o',
        value_name = "DIR",
        help = "Directory to save to [default: <file name>.parts, next to the file]"
    )]
    pub(crate) output: Option<PathBuf>,

    /// Print the list of saved files as JSON.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConvertArgs {
    /// Files to convert; `FILE#id` converts an embedded document.
    #[arg(value_name = "ADDRESS", required = true)]
    pub(crate) inputs: Vec<String>,

    #[arg(
        long,
        short = 'o',
        value_name = "DIR",
        help = "Directory to write to [default: the directory of each file]"
    )]
    pub(crate) output: Option<PathBuf>,

    /// Show spreadsheet formulas next to their values.
    #[arg(long)]
    pub(crate) formulas: bool,

    /// Print the list of written files as JSON (an array when several files
    /// are given).
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FormatsArgs {
    /// Print JSON.
    #[arg(long)]
    pub(crate) json: bool,
}

/// The process arguments, with `read` inserted when the first argument is
/// a file rather than a command: `doctopus report.docx` reads the file.
pub(crate) fn args() -> Vec<OsString> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if let Some(first) = args.get(1).and_then(|a| a.to_str())
        && is_implicit_read(first)
    {
        args.insert(1, "read".into());
    }
    args
}

fn is_implicit_read(first: &str) -> bool {
    const COMMANDS: [&str; 7] = [
        "read", "info", "extract", "convert", "formats", "guide", "help",
    ];
    !first.starts_with('-')
        && !COMMANDS.contains(&first)
        && (Path::new(first).exists() || Address::parse(first).file.is_file())
}

/// A name for files derived from an address: `deck` for `deck.pptx`,
/// `deck-emb1` for `deck.pptx#emb1`.
pub(crate) fn stem(address: &Address) -> String {
    let mut stem = address
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into());
    for part in &address.parts {
        stem.push('-');
        stem.push_str(part);
    }
    stem
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn arguments_are_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn section_flags_are_aliases() {
        for flag in ["--sheet", "--slide", "--page", "--section"] {
            let cli = Cli::try_parse_from(["doctopus", "read", "a.xlsx", flag, "2-3"]).unwrap();
            let Command::Read(args) = cli.command else {
                panic!("not a read")
            };
            assert_eq!(args.sections.as_deref(), Some("2-3"));
        }
    }

    #[test]
    fn offset_and_output_conflict() {
        let parsed =
            Cli::try_parse_from(["doctopus", "read", "a.docx", "--offset", "9", "-o", "a.md"]);
        assert!(parsed.is_err());
    }

    #[test]
    fn stems_include_part_paths() {
        let address = Address::parse_with("dir/deck.pptx#emb1/img2", |p| p == "dir/deck.pptx");
        assert_eq!(stem(&address), "deck-emb1-img2");
    }
}
