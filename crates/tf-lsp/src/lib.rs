//! Editor-neutral dataset references. No workspace mutation, producer imports or networking.
mod protocol;
mod syntax;
/// Encode an existing validated local path as a file URI; callers enforce containment.
pub fn file_uri(path: &std::path::Path) -> Option<String> {
    protocol::uri(path)
}
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

/// Application-owned, read-only workspace metadata; never discovers/imports producers.
pub trait Catalogue {
    /// Whether a document belongs to this workspace's validated source roots.
    fn contains(&self, path: &Path) -> bool;
    /// Exact registered dataset/alias resolution.
    fn resolves(&self, reference: &str) -> bool;
    /// Live catalogue names, including retained aliases; values carry display metadata.
    fn names(&self) -> &BTreeMap<String, Arc<Value>>;
    /// Whether a captured producer descriptor snapshot exists.
    fn has_producers(&self) -> bool;
    /// Whether the generated catalogue type-stub entry point is available.
    fn stubs_available(&self) -> bool;
    /// Cheap authoring/branch revision check; immutable descriptors need no per-keystroke rehash.
    fn needs_refresh(&self) -> bool;
    /// Explicitly label retained metadata after a failed refresh.
    fn mark_stale(&mut self);
    /// Whether current authoring metadata failed validation.
    fn stale(&self) -> bool;
    /// Selected opaque data branch, never inferred from another workspace.
    fn branch(&self) -> &str;
    /// Contextual plaintext hover; unknown built/provider metadata stays unknown.
    fn hover(&self, reference: &str) -> Option<String>;
    /// Producer location, validated against this workspace's registered source roots.
    fn location(&self, reference: &str) -> Option<Value>;
    /// Explicit parser interpreter, falling back to workspace configuration.
    fn python(&self, explicit: Option<&PathBuf>) -> PathBuf;
}
/// Application service loader; repository access stays outside the protocol crate.
pub type Loader = fn(&Path, Option<&str>) -> Result<Box<dyn Catalogue>, String>;
/// Startup selection; individual workspace metadata remains isolated.
#[derive(Default)]
pub struct Config {
    /// Optional command-line workspace; otherwise supplied by initialize.
    pub root: Option<PathBuf>,
    /// Explicit Python 3.14 syntax interpreter; no installation is attempted.
    pub python: Option<PathBuf>,
    /// Explicit data branch, otherwise workspace policy selects it.
    pub branch: Option<String>,
}
struct Document {
    text: String,
    version: i64,
    syntax: Option<Value>,
    pending: bool,
}
struct Context {
    root: PathBuf,
    metadata: Option<Box<dyn Catalogue>>,
    error: Option<String>,
}
struct Server {
    roots: Vec<Context>,
    documents: BTreeMap<String, Document>,
    python: Option<PathBuf>,
    branch: Option<String>,
    initialized: bool,
    shutdown: bool,
    backend: Loader,
}
fn error(message: &str) -> Value {
    json!({"code":-32602,"message":message})
}
fn pos(value: &Value) -> Option<(u64, u64)> {
    Some((value["line"].as_u64()?, value["character"].as_u64()?))
}
fn within(range: &Value, position: &Value) -> bool {
    match (pos(&range["start"]), pos(&range["end"]), pos(position)) {
        (Some(a), Some(b), Some(p)) => a <= p && p <= b,
        _ => false,
    }
}
fn diagnostic(message: &str, range: Value, severity: u8, code: &str) -> Value {
    json!({"range":range,"severity":severity,"code":code,"source":"transflow","message":message})
}
fn zero() -> Value {
    json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}})
}
impl Server {
    fn new(
        root: Option<PathBuf>,
        python: Option<PathBuf>,
        branch: Option<String>,
        backend: Loader,
    ) -> Self {
        Self {
            roots: root
                .into_iter()
                .map(|root| Context {
                    root,
                    metadata: None,
                    error: None,
                })
                .collect(),
            documents: BTreeMap::new(),
            python,
            branch,
            initialized: false,
            shutdown: false,
            backend,
        }
    }
    fn add_root(&mut self, uri: &str) -> Result<(), Value> {
        let path = protocol::path(uri)
            .and_then(|p| p.canonicalize().ok())
            .ok_or_else(|| error("Workspace must be an existing local file URI"))?;
        if self.roots.iter().any(|r| r.root == path) {
            return Ok(());
        }
        if self.roots.len() >= 16 {
            return Err(error("At most 16 workspace roots are supported"));
        }
        self.roots.push(Context {
            root: path,
            metadata: None,
            error: None,
        });
        Ok(())
    }
    fn refresh(&mut self) {
        for context in &mut self.roots {
            if context.error.is_none()
                && context
                    .metadata
                    .as_ref()
                    .is_some_and(|m| !m.needs_refresh())
            {
                continue;
            }
            match (self.backend)(&context.root, self.branch.as_deref()) {
                Ok(metadata) => {
                    context.metadata = Some(metadata);
                    context.error = None;
                }
                Err(e) => {
                    context.error = Some(e);
                    if let Some(m) = &mut context.metadata {
                        m.mark_stale();
                    }
                }
            }
        }
    }
    fn context(&self, uri: &str) -> Option<&Context> {
        let path = protocol::path(uri)?;
        self.roots
            .iter()
            .filter(|r| r.metadata.as_ref().is_some_and(|m| m.contains(&path)))
            .max_by_key(|r| r.root.components().count())
    }
    fn analyze(&mut self, uri: &str) -> Vec<Value> {
        self.refresh();
        let Some(doc) = self.documents.get(uri) else {
            return vec![];
        };
        let Some(context) = self.context(uri) else {
            return vec![diagnostic(
                "Document is outside registered workspace source roots, or workspace metadata is unavailable",
                zero(),
                1,
                "workspace",
            )];
        };
        let Some(metadata) = &context.metadata else {
            return vec![];
        };
        let parsed = if let Some(syntax) = doc.syntax.as_ref().filter(|_| !doc.pending) {
            Ok(syntax.clone())
        } else {
            syntax::inspect(&metadata.python(self.python.as_ref()), &doc.text)
        };
        let mut diagnostics = vec![];
        if context.error.is_some() {
            diagnostics.push(diagnostic(
                "Current catalogue/configuration is invalid; using last valid metadata",
                zero(),
                2,
                "stale-catalogue",
            ));
        }
        if !metadata.has_producers() {
            diagnostics.push(diagnostic("No retained producer descriptors. Run catalog sync explicitly; editor requests do not execute producers",zero(),3,"missing-descriptors"));
        }
        if !metadata.stubs_available() {
            diagnostics.push(diagnostic("Generated catalogue stubs are missing; this service uses the working-tree registry. Run catalog sync explicitly to regenerate stubs",zero(),3,"missing-stubs"));
        }
        match &parsed {
            Err(message) => diagnostics.push(diagnostic(message, zero(), 1, "parser")),
            Ok(parsed) => {
                if parsed["syntax_error"].is_object() {
                    diagnostics.push(diagnostic(
                        parsed["syntax_error"]["message"]
                            .as_str()
                            .unwrap_or("Invalid Python draft"),
                        parsed["syntax_error"]["range"].clone(),
                        1,
                        "syntax",
                    ));
                }
                for reference in parsed["references"].as_array().into_iter().flatten() {
                    let name = reference["value"].as_str().unwrap_or("");
                    if !metadata.resolves(name)
                        && !(reference["form"] == "catalog"
                            && metadata
                                .names()
                                .keys()
                                .any(|path| path.starts_with(&format!("{name}/"))))
                    {
                        diagnostics.push(diagnostic(&format!("Unknown dataset reference: {name}. Register it explicitly or correct the spelling"),reference["range"].clone(),1,"unknown-dataset"));
                    }
                }
            }
        }
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.syntax = parsed.ok();
            doc.pending = false;
        }
        diagnostics
    }
    fn publish(&mut self, out: &mut dyn io::Write, uri: &str) -> io::Result<()> {
        let diagnostics = self.analyze(uri);
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.pending = false;
        }
        protocol::write(
            out,
            &json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":self.documents.get(uri).map(|d|d.version),"diagnostics":diagnostics}}),
        )
    }
    fn flush(&mut self, out: &mut dyn io::Write) -> io::Result<()> {
        let uris: Vec<_> = self
            .documents
            .iter()
            .filter(|(_, d)| d.pending)
            .take(1)
            .map(|(u, _)| u.clone())
            .collect();
        for uri in uris {
            self.publish(out, &uri)?;
        }
        Ok(())
    }
    fn request(&mut self, method: &str, p: &Value) -> Result<Value, Value> {
        if method == "initialize" {
            if self.initialized {
                return Err(error("Server already initialized"));
            }
            if let Some(folders) = p["workspaceFolders"].as_array() {
                for folder in folders {
                    self.add_root(
                        folder["uri"]
                            .as_str()
                            .ok_or_else(|| error("Invalid workspace URI"))?,
                    )?;
                }
            } else if let Some(uri) = p["rootUri"].as_str() {
                self.add_root(uri)?;
            }
            if self.roots.is_empty() {
                return Err(error("Select a Transflow workspace"));
            }
            self.refresh();
            self.initialized = true;
            return Ok(
                json!({"capabilities":{"positionEncoding":"utf-16","textDocumentSync":{"openClose":true,"change":1,"save":{"includeText":false}},"completionProvider":{"triggerCharacters":[".","/","\"","'"]},"hoverProvider":true,"definitionProvider":true,"diagnosticProvider":{"interFileDependencies":false,"workspaceDiagnostics":false},"workspace":{"workspaceFolders":{"supported":true,"changeNotifications":true}}},"serverInfo":{"name":"transflow","version":env!("CARGO_PKG_VERSION")}}),
            );
        }
        if !self.initialized {
            return Err(json!({"code":-32002,"message":"Server not initialized"}));
        }
        if method == "shutdown" {
            self.shutdown = true;
            return Ok(Value::Null);
        }
        if self.shutdown {
            return Err(error("Server is shut down"));
        }
        let uri = p["textDocument"]["uri"]
            .as_str()
            .ok_or_else(|| error("Expected textDocument URI"))?;
        if !self.documents.contains_key(uri) {
            return Err(error("Open the document before requesting editor features"));
        }
        let diagnostics = self.analyze(uri);
        if method == "textDocument/diagnostic" {
            return Ok(json!({"kind":"full","items":diagnostics}));
        }
        let Some(metadata) = self.context(uri).and_then(|c| c.metadata.as_ref()) else {
            return Ok(Value::Null);
        };
        let doc = &self.documents[uri];
        let Some(syntax) = &doc.syntax else {
            return Ok(Value::Null);
        };
        if method == "textDocument/completion" {
            return Ok(completion(doc, metadata.as_ref(), &p["position"]));
        }
        let reference = syntax["references"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| within(&r["range"], &p["position"]));
        let Some(reference) = reference else {
            return Ok(Value::Null);
        };
        let name = reference["value"].as_str().unwrap_or("");
        match method {
            "textDocument/hover"=>Ok(metadata.hover(name).map(|text|json!({"contents":{"kind":"plaintext","value":text},"range":reference["range"]})).unwrap_or(Value::Null)),
            "textDocument/definition"=>Ok(metadata.location(name).unwrap_or(Value::Null)),
            _=>Err(json!({"code":-32601,"message":"Method not found"})),
        }
    }
    fn notification(&mut self, method: &str, p: &Value, out: &mut dyn io::Write) -> io::Result<()> {
        if !self.initialized || self.shutdown {
            return Ok(());
        }
        match method {
            "textDocument/didOpen" | "textDocument/didChange" => {
                let d = &p["textDocument"];
                let (Some(uri), Some(version)) = (d["uri"].as_str(), d["version"].as_i64()) else {
                    return Ok(());
                };
                let text = if method.ends_with("didOpen") {
                    d["text"].as_str()
                } else {
                    p["contentChanges"]
                        .as_array()
                        .filter(|a| a.len() == 1)
                        .and_then(|a| {
                            if a[0].get("range").is_none() {
                                a[0]["text"].as_str()
                            } else {
                                None
                            }
                        })
                };
                let Some(text) = text else {
                    return Ok(());
                };
                if self
                    .documents
                    .get(uri)
                    .is_some_and(|d| version <= d.version)
                {
                    return Ok(());
                }
                if text.len() > 524_288
                    || (self.documents.len() >= 128 && !self.documents.contains_key(uri))
                {
                    protocol::write(
                        out,
                        &json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":version,"diagnostics":[diagnostic("Editor limit exceeded: 512 KiB/document, 128 open documents",zero(),1,"limit")]}}),
                    )?;
                    self.documents.remove(uri);
                    return Ok(());
                }
                self.documents.insert(
                    uri.into(),
                    Document {
                        text: text.into(),
                        version,
                        syntax: None,
                        pending: true,
                    },
                );
            }
            "textDocument/didClose" => {
                if let Some(uri) = p["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                    protocol::write(
                        out,
                        &json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":[]}}),
                    )?;
                }
            }
            "textDocument/didSave" | "workspace/didChangeWatchedFiles" => {
                for doc in self.documents.values_mut() {
                    doc.pending = true;
                }
            }
            "workspace/didChangeWorkspaceFolders" => {
                for folder in p["event"]["removed"].as_array().into_iter().flatten() {
                    if let Some(path) = folder["uri"].as_str().and_then(protocol::path) {
                        self.roots.retain(|r| r.root != path);
                    }
                }
                for folder in p["event"]["added"].as_array().into_iter().flatten() {
                    if let Some(uri) = folder["uri"].as_str() {
                        let _ = self.add_root(uri);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn prefix<'a>(text: &'a str, position: &Value) -> Option<&'a str> {
    let (line, units) = pos(position)?;
    let line = text.split('\n').nth(line as usize)?;
    let mut used = 0;
    for (byte, c) in line.char_indices() {
        if used == units {
            return Some(&line[..byte]);
        }
        used += c.len_utf16() as u64;
        if used > units {
            return None;
        }
    }
    (used == units).then_some(line)
}
fn completion(doc: &Document, metadata: &dyn Catalogue, position: &Value) -> Value {
    let Some(before) = prefix(&doc.text, position) else {
        return Value::Null;
    };
    let Some(syntax) = &doc.syntax else {
        return Value::Null;
    };
    let aliases = &syntax["aliases"];
    let mut found = None;
    for (alias, kind) in aliases.as_object().into_iter().flatten() {
        if kind == "catalog" {
            let start = before
                .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
                .map_or(0, |i| i + 1);
            let token = &before[start..];
            if let Some(value) = token.strip_prefix(&format!("{alias}.")) {
                found = Some((value.replace('.', "/"), true));
                break;
            }
        } else if kind == "input"
            && let Some((_, tail)) = before.rsplit_once(&format!("{alias}("))
        {
            let tail = tail.trim_start();
            if let Some(tail) = tail.strip_prefix(['\"', '\''])
                && !tail.contains(['\"', '\''])
            {
                found = Some((tail.to_string(), false));
                break;
            }
        }
    }
    let Some((typed, catalog)) = found else {
        return json!({"isIncomplete":false,"items":[]});
    };
    if syntax["noncode"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|region| within(&region["range"], position) && (catalog || region["comment"] == true))
    {
        return json!({"isIncomplete":false,"items":[]});
    }
    // Comments and unrelated strings cannot become C references/completion sites.
    if catalog && (before.contains('#') || before.matches(['\"', '\'']).count() % 2 != 0) {
        return json!({"isIncomplete":false,"items":[]});
    }
    let (parent, part) = typed
        .rsplit_once('/')
        .map_or(("", ""), |(parent, part)| (parent, part));
    let part = if typed.contains('/') {
        part
    } else {
        typed.as_str()
    };
    let mut matches = BTreeMap::new();
    for path in metadata.names().keys() {
        let rest = if parent.is_empty() {
            path.as_str()
        } else {
            let Some(rest) = path.strip_prefix(&format!("{parent}/")) else {
                continue;
            };
            rest
        };
        let label = if catalog {
            rest.split('/').next().unwrap_or(rest)
        } else {
            path.as_str()
        };
        if (catalog && !label.starts_with(part)) || (!catalog && !path.starts_with(&typed)) {
            continue;
        }
        if catalog
            && !label
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_alphabetic() || (i > 0 && c.is_numeric()))
        {
            continue;
        }
        let units = if catalog { part } else { typed.as_str() }
            .encode_utf16()
            .count() as u64;
        let mut start = position.clone();
        start["character"] = (position["character"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(units))
        .into();
        let mut end = position.clone();
        if let Some(line) = doc
            .text
            .split('\n')
            .nth(position["line"].as_u64().unwrap_or(0) as usize)
        {
            let suffix = &line[before.len()..];
            let extra = suffix
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || (!catalog && *c == '/'))
                .count() as u64;
            end["character"] = (position["character"].as_u64().unwrap_or(0) + extra).into();
        }
        let semantic_path = if catalog && !parent.is_empty() {
            format!("{parent}/{label}")
        } else {
            label.to_string()
        };
        let identity = metadata
            .names()
            .get(&semantic_path)
            .and_then(|e| e["dataset_id"].as_str())
            .unwrap_or("Catalogue namespace");
        matches.insert(label.to_string(),json!({"label":label,"kind":if catalog&&rest.contains('/'){9}else{6},"detail":format!("{} · {} · working-tree catalogue{}",identity,metadata.branch(),if metadata.stale(){" (last valid)"}else{""}),"textEdit":{"range":{"start":start,"end":end},"newText":label}}));
    }
    let incomplete = matches.len() > 200;
    json!({"isIncomplete":incomplete,"items":matches.into_values().take(200).collect::<Vec<_>>()})
}
/// Run the bounded standard-input LSP event loop; stdout contains protocol frames only.
pub fn serve(
    config: Config,
    backend: Loader,
    out: &mut dyn io::Write,
    err: &mut dyn io::Write,
) -> io::Result<bool> {
    let root = config.root.map(|p| p.canonicalize()).transpose()?;
    let mut server = Server::new(root, config.python, config.branch, backend);
    let (tx, rx) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        loop {
            let frame = protocol::read(&mut input);
            let done = !matches!(frame, Ok(Some(_)));
            if tx.send(frame).is_err() || done {
                break;
            }
        }
    });
    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(Ok(Some(frame))) => {
                let method = frame["method"].as_str().unwrap_or("");
                if method == "exit" {
                    return Ok(server.shutdown);
                }
                if frame["jsonrpc"] != "2.0" || method.is_empty() {
                    if let Some(id) = frame.get("id") {
                        protocol::write(
                            out,
                            &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid request"}}),
                        )?;
                    }
                    continue;
                }
                if let Some(id) = frame.get("id") {
                    let result = server.request(method, &frame["params"]);
                    let response = match result {
                        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
                    };
                    if protocol::write(out, &response).is_err() {
                        protocol::write(
                            out,
                            &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"Response exceeds the protocol limit"}}),
                        )?;
                    }
                } else {
                    server.notification(method, &frame["params"], out)?;
                }
            }
            Ok(Ok(None)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(server.shutdown),
            Ok(Err(_)) => {
                writeln!(err, "Invalid or excessive LSP input frame")?;
                return Ok(false);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => server.flush(out)?,
        }
    }
}
