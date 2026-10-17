"""Bounded stdlib-only syntax inspection, embedded in the native language service.

Reads a JSON string from stdin. Never imports, compiles or executes the document.
All semantic dataset resolution remains in the Rust service.
"""

import ast
import io
import json
import sys
import tokenize

if sys.version_info[:2] != (3, 14):
    raise RuntimeError("Python 3.14 is required")
text = json.load(sys.stdin)
if not isinstance(text, str) or len(text.encode("utf-8")) > 524288:
    raise ValueError("Document limit")
lines = text.splitlines()


def position(line, byte):
    prefix = lines[line - 1].encode("utf-8")[:byte].decode("utf-8")
    return {"line": line - 1, "character": len(prefix.encode("utf-16-le")) // 2}


def span(node):
    return {
        "start": position(node.lineno, node.col_offset),
        "end": position(node.end_lineno, node.end_col_offset),
    }


aliases = {}
references = []
error = None
try:
    tree = ast.parse(text)
except (SyntaxError, ValueError, RecursionError) as exc:
    tree = None
    if isinstance(exc, SyntaxError):
        line = max(0, (exc.lineno or 1) - 1)
        column = max(0, (exc.offset or 1) - 1)
        prefix = lines[line][:column] if line < len(lines) else ""
        start = {"line": line, "character": len(prefix.encode("utf-16-le")) // 2}
        error = {"range": {"start": start, "end": start}, "message": exc.msg}
    else:
        error = {
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
            "message": "Document syntax exceeds the supported parser limits",
        }

# Recover import context from complete import lines when the draft is unfinished.
imports = list(ast.walk(tree)) if tree is not None else []
if tree is None:
    for line in lines:
        if line.startswith(("from transflow", "import transflow")):
            try:
                imports.extend(ast.walk(ast.parse(line)))
            except SyntaxError:
                pass
for node in imports:
    if isinstance(node, ast.ImportFrom):
        for alias in node.names:
            if node.module == "transflow.catalog" and alias.name == "C":
                aliases[alias.asname or "C"] = "catalog"
            if node.module == "transflow" and alias.name == "Input":
                aliases[alias.asname or "Input"] = "input"
    elif isinstance(node, ast.Import):
        for alias in node.names:
            if alias.name == "transflow.catalog":
                aliases[(alias.asname or "transflow.catalog") + ".C"] = "catalog"
            elif alias.name == "transflow":
                aliases[(alias.asname or "transflow") + ".Input"] = "input"


def dotted(node):
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = dotted(node.value)
        return base + "." + node.attr if base else ""
    return ""


if tree is not None:
    # Conservatively disable ambiguous aliases, including local rebinding.
    assigned = {
        node.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)
    }
    assigned.update(node.arg for node in ast.walk(tree) if isinstance(node, ast.arg))
    aliases = {name: kind for name, kind in aliases.items() if name.split(".")[0] not in assigned}
    parents = {child: node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and aliases.get(dotted(node.func)) == "input":
            ref = node.args[0] if node.args else None
            if isinstance(ref, ast.Constant) and isinstance(ref.value, str):
                references.append({"form": "string", "value": ref.value, "range": span(ref)})
        if isinstance(node, ast.Attribute):
            name = dotted(node)
            for alias, kind in aliases.items():
                if kind != "catalog" or not name.startswith(alias + "."):
                    continue
                # Report the longest expression, never duplicate its prefixes.
                parent = parents.get(node)
                if isinstance(parent, ast.Attribute) and parent.value is node:
                    continue
                if isinstance(node.ctx, ast.Load):
                    references.append(
                        {
                            "form": "catalog",
                            "value": name[len(alias) + 1 :].replace(".", "/"),
                            "range": span(node),
                        }
                    )
                break
if len(references) > 4096:
    raise ValueError("Reference limit exceeded; split this document")
noncode = []
try:
    for token in tokenize.generate_tokens(io.StringIO(text).readline):
        if token.type in (tokenize.COMMENT, tokenize.STRING):

            def token_position(point):
                line, column = point
                prefix = lines[line - 1][:column] if line <= len(lines) else ""
                return {"line": line - 1, "character": len(prefix.encode("utf-16-le")) // 2}

            noncode.append(
                {
                    "comment": token.type == tokenize.COMMENT,
                    "range": {
                        "start": token_position(token.start),
                        "end": token_position(token.end),
                    },
                }
            )
            if len(noncode) > 8192:
                raise ValueError("Lexical context limit exceeded")
except tokenize.TokenError, IndentationError, SyntaxError:
    pass  # Keep complete lexical regions from an unfinished draft.
json.dump(
    {"aliases": aliases, "references": references, "syntax_error": error, "noncode": noncode},
    sys.stdout,
)
