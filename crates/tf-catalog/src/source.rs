//! Deterministic source allowlist. Enumeration reads metadata, never imports user code.
use crate::workspace::Workspace;
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// A bounded source enumeration failure; paths are available structurally, never interpolated.
#[derive(Debug, thiserror::Error)]
#[error("Source enumeration failed: {reason}; correct the affected source paths and retry")]
pub struct SourceError {
    /// Fixed actionable reason without source contents.
    pub reason: &'static str,
    /// Workspace-relative affected paths (sanitize before human rendering).
    pub paths: Vec<PathBuf>,
}
fn fail(reason: &'static str, paths: Vec<PathBuf>) -> SourceError {
    SourceError { reason, paths }
}
/// One allowed file, stored under its workspace-relative authoring path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFile {
    path: PathBuf,
    bytes: u64,
    identity: (u64, u64, i64, i64, i64, i64),
}
impl SourceFile {
    /// Workspace-relative destination in a source capture.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Observed size; capture must independently guard and hash copied bytes.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}
/// Immutable complete allowlist plus excluded paths; no content or mtime is execution authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIndex {
    files: Vec<SourceFile>,
    excluded: Vec<PathBuf>,
    roots: Vec<PathBuf>,
}
impl SourceIndex {
    /// Enumerate only configured roots, respecting hard exclusions, nested workspaces and globs.
    pub fn enumerate(workspace: &Workspace) -> Result<Self, SourceError> {
        let mut scan = Scanner {
            workspace,
            files: Vec::new(),
            excluded: Vec::new(),
            directories: BTreeSet::new(),
            count: 0,
        };
        for pattern in workspace.config().source_exclude() {
            validate_glob(pattern)?;
        }
        for relative in workspace.config().source_roots() {
            scan.visit(&workspace.root().join(relative), relative, 0)?;
        }
        scan.files.sort_by(|a, b| a.path.cmp(&b.path));
        scan.excluded.sort();
        Ok(Self {
            files: scan.files,
            excluded: scan.excluded,
            roots: workspace.config().source_roots().to_vec(),
        })
    }
    /// Sorted capture candidates, including undecorated helpers and non-Python resources.
    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }
    /// Explicit excluded paths (an excluded directory represents its entire subtree).
    pub fn excluded(&self) -> &[PathBuf] {
        &self.excluded
    }
    /// Ordered relative import roots, preserved for the capture and module index.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
    /// An explicit computation resource cannot silently disappear through exclusion rules.
    pub fn require_file(&self, path: &Path) -> Result<&SourceFile, SourceError> {
        self.files
            .binary_search_by(|file| file.path.as_path().cmp(path))
            .map(|i| &self.files[i])
            .map_err(|_| {
                fail(
                    "explicit computation file is missing or excluded",
                    vec![path.to_owned()],
                )
            })
    }
}
struct Scanner<'a> {
    workspace: &'a Workspace,
    files: Vec<SourceFile>,
    excluded: Vec<PathBuf>,
    directories: BTreeSet<PathBuf>,
    count: usize,
}
impl Scanner<'_> {
    fn visit(&mut self, absolute: &Path, relative: &Path, depth: usize) -> Result<(), SourceError> {
        self.count += 1;
        if self.count > 100_000 || depth > 64 {
            return Err(fail(
                "source tree exceeds 100000 entries or 64 directory levels",
                vec![relative.to_owned()],
            ));
        }
        let display = relative
            .to_str()
            .ok_or_else(|| fail("source paths must be UTF-8", vec![relative.to_owned()]))?;
        if hard_excluded(relative)
            || self
                .workspace
                .config()
                .source_exclude()
                .iter()
                .any(|p| glob_matches(p, display))
        {
            self.excluded.push(relative.to_owned());
            return Ok(());
        }
        let link = fs::symlink_metadata(absolute).map_err(|_| {
            fail(
                "source entry disappeared or cannot be inspected",
                vec![relative.to_owned()],
            )
        })?;
        let resolved = absolute
            .canonicalize()
            .map_err(|_| fail("source link is broken or cyclic", vec![relative.to_owned()]))?;
        let target = resolved.strip_prefix(self.workspace.root()).map_err(|_| {
            fail(
                "source symlink escapes the workspace",
                vec![relative.to_owned()],
            )
        })?;
        if hard_excluded(target) {
            return Err(fail(
                "source link targets protected state or credentials",
                vec![relative.to_owned()],
            ));
        }
        // Links must stay inside the configured source tree, not reach arbitrary authoring files.
        if link.file_type().is_symlink()
            && !self
                .workspace
                .roots()
                .iter()
                .any(|root| resolved.starts_with(root))
        {
            return Err(fail(
                "source link targets a file outside configured source roots",
                vec![relative.to_owned()],
            ));
        }
        if self
            .workspace
            .config()
            .source_exclude()
            .iter()
            .any(|pattern| {
                target
                    .ancestors()
                    .filter_map(Path::to_str)
                    .any(|name| glob_matches(pattern, name))
            })
        {
            self.excluded.push(relative.to_owned());
            return Ok(());
        }
        for ancestor in resolved
            .parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .take_while(|p| *p != self.workspace.root())
        {
            if ancestor.join("workspace.toml").exists() || ancestor.join("pyvenv.cfg").exists() {
                return Err(fail(
                    "source link or path enters a nested workspace or environment",
                    vec![relative.to_owned()],
                ));
            }
        }
        let metadata = fs::metadata(&resolved).map_err(|_| {
            fail(
                "source entry changed during enumeration",
                vec![relative.to_owned()],
            )
        })?;
        if metadata.is_dir() {
            if resolved.join("workspace.toml").exists() || resolved.join("pyvenv.cfg").exists() {
                self.excluded.push(relative.to_owned());
                return Ok(());
            }
            if !self.directories.insert(resolved) {
                return Err(fail(
                    "directory aliases or symlink cycles would enumerate source twice",
                    vec![relative.to_owned()],
                ));
            }
            let mut children = Vec::new();
            for entry in fs::read_dir(absolute)
                .map_err(|_| fail("source directory is unreadable", vec![relative.to_owned()]))?
            {
                let entry = entry
                    .map_err(|_| fail("source enumeration failed", vec![relative.to_owned()]))?;
                children.push(entry.file_name());
                if children.len() > 100_000 {
                    return Err(fail(
                        "source directory exceeds 100000 entries",
                        vec![relative.to_owned()],
                    ));
                }
            }
            children.sort();
            for child in children {
                self.visit(&absolute.join(&child), &relative.join(&child), depth + 1)?;
            }
        } else if metadata.is_file() {
            self.files.push(SourceFile {
                path: relative.to_owned(),
                bytes: metadata.len(),
                identity: (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                ),
            });
        } else {
            return Err(fail(
                "source contains a special file; only regular files are captured",
                vec![relative.to_owned()],
            ));
        }
        Ok(())
    }
}
/// Hard safety exclusions are independent of user globs and apply to link targets too.
pub fn hard_excluded(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str().to_str().is_none_or(|part| {
            matches!(
                part,
                ".git"
                    | ".transflow"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | "node_modules"
                    | "transflow.local.toml"
                    | ".aws"
                    | ".ssh"
                    | ".netrc"
                    | ".pypirc"
                    | "id_rsa"
                    | "id_ed25519"
            ) || part == ".env"
                || part.starts_with(".env.")
                || part.ends_with(".pem")
                || part.ends_with(".key")
                || part.ends_with(".pyc")
        })
    })
}
#[derive(Clone)]
enum Token {
    Star,
    Any,
    Literal(char),
    Class {
        negate: bool,
        ranges: Vec<(char, char)>,
    },
}
fn tokens(pattern: &str) -> Result<Vec<Token>, SourceError> {
    let mut input = pattern.chars().peekable();
    let mut result = Vec::new();
    while let Some(c) = input.next() {
        result.push(match c {
            '*' => Token::Star,
            '?' => Token::Any,
            '[' => {
                let negate = input.peek() == Some(&'!');
                if negate {
                    input.next();
                }
                let mut ranges = Vec::new();
                let mut closed = false;
                while let Some(start) = input.next() {
                    if start == ']' {
                        closed = true;
                        break;
                    }
                    let end = if input.peek() == Some(&'-') {
                        input.next();
                        input.next().ok_or_else(|| {
                            fail("unterminated source exclusion character range", vec![])
                        })?
                    } else {
                        start
                    };
                    if end == ']' || end < start {
                        return Err(fail("invalid source exclusion character range", vec![]));
                    }
                    ranges.push((start, end));
                }
                if !closed || ranges.is_empty() {
                    return Err(fail(
                        "empty or unterminated source exclusion character class",
                        vec![],
                    ));
                }
                Token::Class { negate, ranges }
            }
            '{' | '}' | '\\' => {
                return Err(fail(
                    "source globs do not support brace expansion or backslash escapes",
                    vec![],
                ));
            }
            _ => Token::Literal(c),
        });
    }
    Ok(result)
}
fn validate_glob(pattern: &str) -> Result<(), SourceError> {
    for part in pattern.trim_end_matches('/').split('/') {
        tokens(part)?;
    }
    Ok(())
}
fn segment_matches(pattern: &str, text: &str) -> bool {
    let Ok(p) = tokens(pattern) else {
        return false;
    };
    let t: Vec<_> = text.chars().collect();
    let mut previous = vec![false; t.len() + 1];
    previous[0] = true;
    for token in p {
        let mut current = vec![false; t.len() + 1];
        if matches!(token, Token::Star) {
            current[0] = previous[0];
        }
        for (i, ch) in t.iter().enumerate() {
            current[i + 1] = match &token {
                Token::Star => previous[i + 1] || current[i],
                Token::Any => previous[i],
                Token::Literal(c) => previous[i] && c == ch,
                Token::Class { negate, ranges } => {
                    previous[i] && (ranges.iter().any(|(a, b)| a <= ch && ch <= b) != *negate)
                }
            };
        }
        previous = current;
    }
    previous[t.len()]
}
fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    let p: Vec<_> = pattern.split('/').collect();
    let t: Vec<_> = path.split('/').collect();
    // Patterns without a slash match a basename at any depth; directory matches prune its subtree.
    if p.len() == 1 {
        return t.last().is_some_and(|name| segment_matches(pattern, name));
    }
    let mut previous = vec![false; t.len() + 1];
    previous[0] = true;
    for segment in p {
        let mut current = vec![false; t.len() + 1];
        if segment == "**" {
            current[0] = previous[0];
        }
        for (i, name) in t.iter().enumerate() {
            current[i + 1] = if segment == "**" {
                previous[i + 1] || current[i]
            } else {
                previous[i] && segment_matches(segment, name)
            };
        }
        previous = current;
    }
    previous[t.len()]
}

/// Apply the same validated exclusion patterns to a prospective Git path before reading its blob.
pub(crate) fn excluded_by_patterns(patterns: &[String], path: &Path) -> Result<bool, SourceError> {
    for pattern in patterns {
        validate_glob(pattern)?;
    }
    Ok(path
        .ancestors()
        .filter_map(Path::to_str)
        .any(|name| patterns.iter().any(|p| glob_matches(p, name))))
}
