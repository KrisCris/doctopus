//! Paths written into Markdown links and shown in reports.

use std::path::{Component, Path, PathBuf};

use doctopus_core::Address;

/// The directory of the file an address names: where its converted or
/// extracted files go unless `-o` says otherwise.
pub(crate) fn beside(address: &Address) -> PathBuf {
    address
        .file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// `path` as reports show it: absolute, so it says where the file is
/// whatever directory the reader is in, and with symbolic links resolved
/// once it exists, so `link/..` names the directory it leads to.
pub(crate) fn shown(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(real) => without_verbatim_prefix(real),
        Err(_) => absolute(path).unwrap_or_else(|| path.to_path_buf()),
    }
}

/// `path` as a key for the names taken in one run: absolute and without
/// case, as names compare on Windows and macOS, and the same whether or not
/// the file is there yet.
pub(crate) fn key(path: &Path) -> String {
    absolute(path)
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

/// Whether `a` and `b` are the same file on disk; false when either is not
/// there.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A canonical Windows path without its `\\?\` prefix, which many programs
/// do not accept.
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        let text = path.to_string_lossy();
        if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{share}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// `file` relative to the directory `dir` that holds it, for listing the
/// files written there.
pub(crate) fn within(dir: &Path, file: &Path) -> PathBuf {
    relative(dir, file).unwrap_or_else(|| file.to_path_buf())
}

/// `file` as a link from a Markdown file in the directory `from`: relative
/// to it when both are on the same root, and with `/` between components
/// on every platform.
pub(crate) fn link(from: &Path, file: &Path) -> String {
    let relative = relative(from, file).unwrap_or_else(|| file.to_path_buf());
    slashes(&relative)
}

/// `path` with `/` between components, as links write it, also on Windows.
pub(crate) fn slashes(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.into_owned()
    }
}

/// The path from directory `from` to `file`, or `None` when they share no
/// root (different drives on Windows).
fn relative(from: &Path, file: &Path) -> Option<PathBuf> {
    let from = absolute(from)?;
    let file = absolute(file)?;
    let from: Vec<Component<'_>> = from.components().collect();
    let file: Vec<Component<'_>> = file.components().collect();
    let common = from.iter().zip(&file).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return None;
    }
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for component in &file[common..] {
        out.push(component);
    }
    Some(out)
}

/// `path` made absolute against the current directory, with `.` and `..`
/// resolved by name. Both sides of a link are built the same way, so
/// resolving `..` without looking at symbolic links is good enough.
fn absolute(path: &Path) -> Option<PathBuf> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let mut out = PathBuf::new();
    for component in std::path::absolute(path).ok()?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_relative_to_the_markdown_file() {
        let out = Path::new("out");
        let image = out.join("report.parts").join("image 1.png");
        assert_eq!(link(out, &image), "report.parts/image 1.png");

        let nested = out.join("report.parts").join("emb1");
        assert_eq!(link(&nested, &image), "../image 1.png");
        assert_eq!(link(Path::new(""), &image), "out/report.parts/image 1.png");
        assert_eq!(
            link(&out.join("..").join("out"), &image),
            "report.parts/image 1.png"
        );
    }

    #[test]
    fn reports_show_absolute_paths() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(shown(Path::new("")), here);
        assert_eq!(shown(Path::new("a/../b.md")), here.join("b.md"));
        let dir = shown(Path::new("out"));
        assert_eq!(
            within(&dir, &dir.join("report.parts").join("a.png")),
            Path::new("report.parts").join("a.png")
        );
        let address = Address::parse("docs/report.docx");
        assert_eq!(beside(&address), Path::new("docs"));
        assert_eq!(beside(&Address::parse("report.docx")), Path::new(""));
    }
}
