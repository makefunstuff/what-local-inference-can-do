//! Context tree: a document-ordered tree of text and file nodes.
//!
//! Piped stdin becomes a single text node; `-c` loads a file that is either a
//! JSON tree or plain text (plain text = one text node).
//!
//! Node shape (JSON object):
//!   { "text": "..." }        raw text
//!   { "file": "path" }       file contents, read at render time (path relative to cwd)
//!   { "children": [ ... ] }  nested subtree
//! An array of nodes is also accepted. Rendering walks the tree in document
//! order; file leaves are emitted as `─── path ───` followed by their contents.

use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Clone)]
pub struct Node {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<Node>>,
}

pub enum Leaf {
    Text(String),
    File { path: String, content: String },
}

pub struct Tree {
    pub nodes: Vec<Node>,
}

/// Parse context source text: a JSON tree, or plain text (one text node).
pub fn parse(source: &str) -> Result<Tree, crate::Fail> {
    let v = match serde_json::from_str::<serde_json::Value>(source) {
        Ok(v) if v.is_object() || v.is_array() => v,
        _ => {
            return Ok(Tree {
                nodes: vec![Node {
                    text: Some(source.to_string()),
                    file: None,
                    children: None,
                }],
            })
        }
    };
    let nodes = if v.is_array() {
        let mut nodes = Vec::new();
        for n in v.as_array().unwrap() {
            nodes.push(parse_node(n)?);
        }
        nodes
    } else {
        vec![parse_node(&v)?]
    };
    Ok(Tree { nodes })
}

fn parse_node(v: &serde_json::Value) -> Result<Node, crate::Fail> {
    let n: Node = serde_json::from_value(v.clone())
        .map_err(|e| crate::Fail::Usage(format!("bad context node: {e}")))?;
    if n.text.is_none() && n.file.is_none() && n.children.is_none() {
        return Err(crate::Fail::Usage(
            "context node needs a 'text', 'file', or 'children' key".into(),
        ));
    }
    Ok(n)
}

/// Load a context file (JSON tree or plain text).
pub fn load_file(path: &Path) -> Result<Tree, crate::Fail> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| crate::Fail::IO(format!("cannot read context file {}: {e}", path.display())))?;
    parse(&src)
}

impl Tree {
    /// Flatten to leaves in document order; file nodes are read eagerly.
    pub fn leaves(&self) -> Result<Vec<Leaf>, crate::Fail> {
        let mut out = Vec::new();
        for n in &self.nodes {
            collect(n, &mut out)?;
        }
        Ok(out)
    }

    /// Rendered context block handed to the model.
    pub fn render(&self) -> String {
        match self.leaves() {
            Ok(leaves) => leaves
                .iter()
                .map(|l| match l {
                    Leaf::Text(t) => t.clone(),
                    Leaf::File { path, content } => format!("─── {path} ───\n{content}"),
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            Err(e) => format!("[context error: {}]", e.msg()),
        }
    }
}

fn collect(n: &Node, out: &mut Vec<Leaf>) -> Result<(), crate::Fail> {
    if let Some(path) = &n.file {
        // file takes precedence over text/children if both are present.
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::Fail::IO(format!("cannot read context file {}: {e}", path)))?;
        out.push(Leaf::File { path: path.clone(), content });
    } else {
        if let Some(t) = &n.text {
            out.push(Leaf::Text(t.clone()));
        }
        if let Some(children) = &n.children {
            for c in children {
                collect(c, out)?;
            }
        }
    }
    Ok(())
}
