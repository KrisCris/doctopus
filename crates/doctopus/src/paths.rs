//! Paths written into Markdown links.

use std::path::{Component, Path, PathBuf};

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
}
