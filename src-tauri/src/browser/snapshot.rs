//! `browser_snapshot`: the page's accessibility tree
//! (`Accessibility.getFullAXTree`) as compact indented lines
//! `[ref] role "name" (state, ...)`, and the bounded page of it one call
//! returns.
//!
//! Every emitted element gets a ref (`e1`, `e2`, ...) naming its
//! `backendDOMNodeId`, so a later call can address it until the tab
//! navigates. Text runs are `text "..."` lines without a ref. Ignored nodes
//! and unnamed layout wrappers are skipped; their children keep the depth.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

/// The double-escaped size one `browser_snapshot` observation stays under:
/// below the 40 KiB a native CLI echoes back, with room for the result
/// envelope the tool gate adds.
pub const SNAPSHOT_BUDGET: usize = 36 * 1024;
/// Characters of one element name or value before it is cut with `…`.
const MAX_NAME_CHARS: usize = 160;
/// Characters of one text run: page text is what a reader came for, so it is
/// cut far later than a name (long pages continue by `offset`).
const MAX_TEXT_CHARS: usize = 2000;
/// Indentation stops growing past this depth.
const MAX_INDENT_DEPTH: usize = 24;
/// Nesting levels of the tree walked; deeper content is left out with one
/// marker line, so a page of thousands of nested elements costs bounded
/// work and never a deep call stack.
const MAX_TREE_DEPTH: usize = 256;

/// Unnamed nodes of these roles are layout only: their children take their
/// place at the same depth.
const TRANSPARENT_ROLES: &[&str] = &[
    "none",
    "generic",
    "presentation",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
    "LineBreak",
    "paragraph",
    "group",
    "Section",
    "LabelText",
    "Abbr",
    "ruby",
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub lines: Vec<String>,
    /// `e<n>` → backendDOMNodeId.
    pub refs: BTreeMap<String, i64>,
}

/// Format an `Accessibility.getFullAXTree` result.
pub fn format_tree(result: &Value) -> Snapshot {
    let nodes = result
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|node| Some((node.get("nodeId")?.as_str()?, node)))
        .collect();
    let mut snapshot = Snapshot::default();
    let mut omitted = false;
    // Roots: nodes whose parent is not in the tree (normally one RootWebArea).
    for node in nodes {
        let parent = node.get("parentId").and_then(Value::as_str);
        if !parent.is_some_and(|parent| by_id.contains_key(parent)) {
            emit(node, &by_id, &mut snapshot, &mut omitted);
        }
    }
    snapshot
}

/// Emit the subtree of `root` in document order, walking an explicit stack
/// (never the call stack) at most [`MAX_TREE_DEPTH`] levels deep.
fn emit(root: &Value, by_id: &HashMap<&str, &Value>, out: &mut Snapshot, omitted: &mut bool) {
    // (node, indent depth, tree level, the name its text runs repeat)
    let mut stack: Vec<(&Value, usize, usize, String)> = vec![(root, 0, 0, String::new())];
    let mut visited = 0_usize;
    while let Some((node, depth, level, parent_name)) = stack.pop() {
        // A malformed tree with a cycle could never end otherwise.
        visited += 1;
        if visited > by_id.len().saturating_mul(2) + 1 {
            return;
        }
        if level >= MAX_TREE_DEPTH {
            if !*omitted {
                *omitted = true;
                out.lines.push(format!(
                    "{}… (content nested deeper than {MAX_TREE_DEPTH} levels omitted)",
                    indent(depth)
                ));
            }
            continue;
        }
        emit_node(node, depth, level, &parent_name, by_id, out, &mut stack);
    }
}

/// Emit one node's line and push its children (last first, so they pop in
/// order) onto `stack`.
fn emit_node<'a>(
    node: &'a Value,
    depth: usize,
    level: usize,
    parent_name: &str,
    by_id: &HashMap<&str, &'a Value>,
    out: &mut Snapshot,
    stack: &mut Vec<(&'a Value, usize, usize, String)>,
) {
    let role = text_field(node, "role");
    let name = text_field(node, "name");
    let mut push_children = |depth: usize, name: &str| {
        let children = node
            .get("childIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|id| by_id.get(id).copied())
            .collect::<Vec<_>>();
        for child in children.into_iter().rev() {
            stack.push((child, depth, level + 1, name.to_string()));
        }
    };
    let ignored = node.get("ignored").and_then(Value::as_bool) == Some(true);
    if role == "InlineTextBox" {
        return;
    }
    if role == "StaticText" {
        let text = clean(&name, MAX_TEXT_CHARS);
        if !text.is_empty() && text != parent_name {
            out.lines
                .push(format!("{}text {}", indent(depth), quote(&text)));
        }
        return;
    }
    let name = clean(&name, MAX_NAME_CHARS);
    let states = states(node);
    if ignored
        || (name.is_empty() && states.is_empty() && TRANSPARENT_ROLES.contains(&role.as_str()))
    {
        push_children(depth, parent_name);
        return;
    }
    let mut line = indent(depth);
    if let Some(backend) = node.get("backendDOMNodeId").and_then(Value::as_i64) {
        let reference = format!("e{}", out.refs.len() + 1);
        line.push_str(&format!("[{reference}] "));
        out.refs.insert(reference, backend);
    }
    line.push_str(if role == "RootWebArea" {
        "document"
    } else if role.is_empty() {
        "node"
    } else {
        &role
    });
    if !name.is_empty() {
        line.push(' ');
        line.push_str(&quote(&name));
    }
    if !states.is_empty() {
        line.push_str(&format!(" ({})", states.join(", ")));
    }
    out.lines.push(line);
    push_children(depth + 1, &name);
}

/// `role`/`name`/`value` are `{type, value}` objects.
fn text_field(node: &Value, field: &str) -> String {
    match node.get(field).and_then(|field| field.get("value")) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn states(node: &Value) -> Vec<String> {
    let mut states = Vec::new();
    let value = clean(&text_field(node, "value"), MAX_NAME_CHARS);
    if !value.is_empty() {
        states.push(format!("value {}", quote(&value)));
    }
    let properties = node
        .get("properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for property in properties {
        let name = property.get("name").and_then(Value::as_str).unwrap_or("");
        let value = property.get("value").and_then(|value| value.get("value"));
        let truthy = matches!(value, Some(Value::Bool(true)))
            || matches!(value, Some(Value::String(text)) if text == "true");
        let state = match name {
            "focused" if truthy => Some("focused".to_string()),
            "disabled" if truthy => Some("disabled".to_string()),
            "required" if truthy => Some("required".to_string()),
            "readonly" if truthy => Some("readonly".to_string()),
            "selected" if truthy => Some("selected".to_string()),
            "modal" if truthy => Some("modal".to_string()),
            "expanded" => Some(if truthy { "expanded" } else { "collapsed" }.to_string()),
            "checked" | "pressed" => match value {
                Some(Value::String(text)) if text == "mixed" => Some("mixed".to_string()),
                _ if truthy => Some(name.to_string()),
                _ => Some(format!("not {name}")),
            },
            "invalid" => match value {
                Some(Value::String(text)) if text != "false" => Some("invalid".to_string()),
                _ => None,
            },
            "level" => value
                .and_then(Value::as_i64)
                .map(|level| format!("level {level}")),
            _ => None,
        };
        states.extend(state);
    }
    states
}

/// One line of text: whitespace collapsed, cut at `limit` characters.
fn clean(text: &str, limit: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > limit {
        let mut cut = collapsed.chars().take(limit).collect::<String>();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth.min(MAX_INDENT_DEPTH))
}

/// Bytes of `value` rendered as compact JSON and then embedded as a JSON
/// string — the measure a native CLI applies to a tool result it echoes.
pub fn double_escaped_len(value: &Value) -> usize {
    serde_json::to_string(&Value::String(value.to_string()))
        .map(|text| text.len())
        .unwrap_or(usize::MAX)
}

/// What one line adds to the double-escaped size of the text holding it.
fn line_cost(line: &str) -> usize {
    let once = serde_json::to_string(&format!("{line}\n")).unwrap_or_default();
    serde_json::to_string(&once)
        .map(|twice| twice.len().saturating_sub(6))
        .unwrap_or(usize::MAX)
}

/// The page of `lines` starting at `offset` that keeps `build(text,
/// nextOffset)` within `budget` double-escaped bytes. `nextOffset` is the
/// first line left out, or `None` when the page reaches the end. At least one
/// line is returned while any remains.
pub fn page(
    lines: &[String],
    offset: usize,
    budget: usize,
    build: impl Fn(&str, Option<usize>) -> Value,
) -> Value {
    let offset = offset.min(lines.len());
    let base = double_escaped_len(&build("", Some(lines.len())));
    let mut used = base;
    let mut end = offset;
    while end < lines.len() {
        let cost = line_cost(&lines[end]);
        if used + cost > budget && end > offset {
            break;
        }
        used += cost;
        end += 1;
    }
    loop {
        let next = (end < lines.len()).then_some(end);
        let text = lines[offset..end].join("\n");
        let value = build(&text, next);
        if double_escaped_len(&value) <= budget || end <= offset + 1 {
            return value;
        }
        end -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({"nodes": [
            {"nodeId": "1", "ignored": false, "role": {"type": "role", "value": "RootWebArea"},
             "name": {"type": "computedString", "value": "Example Domain"}, "childIds": ["2"],
             "backendDOMNodeId": 1,
             "properties": [{"name": "focusable", "value": {"type": "booleanOrUndefined", "value": true}}]},
            {"nodeId": "2", "parentId": "1", "ignored": true, "role": {"type": "role", "value": "none"},
             "childIds": ["3"], "backendDOMNodeId": 2},
            {"nodeId": "3", "parentId": "2", "ignored": false, "role": {"type": "role", "value": "generic"},
             "name": {"type": "computedString", "value": ""}, "childIds": ["4", "6", "8", "11", "13"],
             "backendDOMNodeId": 3},
            {"nodeId": "4", "parentId": "3", "ignored": false, "role": {"type": "role", "value": "heading"},
             "name": {"type": "computedString", "value": "Example Domain"}, "childIds": ["5"],
             "backendDOMNodeId": 4,
             "properties": [{"name": "level", "value": {"type": "integer", "value": 1}}]},
            {"nodeId": "5", "parentId": "4", "ignored": false, "role": {"type": "role", "value": "StaticText"},
             "name": {"type": "computedString", "value": "Example Domain"}, "childIds": ["5a"], "backendDOMNodeId": 5},
            {"nodeId": "5a", "parentId": "5", "ignored": false, "role": {"type": "role", "value": "InlineTextBox"},
             "name": {"type": "computedString", "value": "Example Domain"}},
            {"nodeId": "6", "parentId": "3", "ignored": false, "role": {"type": "role", "value": "paragraph"},
             "name": {"type": "computedString", "value": ""}, "childIds": ["7"], "backendDOMNodeId": 6},
            {"nodeId": "7", "parentId": "6", "ignored": false, "role": {"type": "role", "value": "StaticText"},
             "name": {"type": "computedString", "value": "This   domain is\n for \"use\" in examples."},
             "backendDOMNodeId": 7},
            {"nodeId": "8", "parentId": "3", "ignored": false, "role": {"type": "role", "value": "link"},
             "name": {"type": "computedString", "value": "More information..."}, "childIds": ["9"],
             "backendDOMNodeId": 8,
             "properties": [{"name": "focused", "value": {"type": "boolean", "value": true}}]},
            {"nodeId": "9", "parentId": "8", "ignored": false, "role": {"type": "role", "value": "StaticText"},
             "name": {"type": "computedString", "value": "More information..."}, "backendDOMNodeId": 9},
            {"nodeId": "11", "parentId": "3", "ignored": false, "role": {"type": "role", "value": "textbox"},
             "name": {"type": "computedString", "value": "검색"},
             "value": {"type": "string", "value": "안녕"}, "backendDOMNodeId": 11,
             "properties": [{"name": "required", "value": {"type": "boolean", "value": true}},
                            {"name": "invalid", "value": {"type": "token", "value": "false"}}]},
            {"nodeId": "13", "parentId": "3", "ignored": false, "role": {"type": "role", "value": "checkbox"},
             "name": {"type": "computedString", "value": "Agree"}, "backendDOMNodeId": 13,
             "properties": [{"name": "checked", "value": {"type": "tristate", "value": "false"}},
                            {"name": "disabled", "value": {"type": "boolean", "value": true}}]},
        ]})
    }

    #[test]
    fn the_tree_is_compact_lines_with_refs_to_backend_nodes() {
        let snapshot = format_tree(&fixture());
        assert_eq!(
            snapshot.lines,
            vec![
                r#"[e1] document "Example Domain""#,
                r#"  [e2] heading "Example Domain" (level 1)"#,
                r#"  text "This domain is for \"use\" in examples.""#,
                r#"  [e3] link "More information..." (focused)"#,
                r#"  [e4] textbox "검색" (value "안녕", required)"#,
                r#"  [e5] checkbox "Agree" (not checked, disabled)"#,
            ]
        );
        assert_eq!(
            snapshot.refs,
            BTreeMap::from([
                ("e1".to_string(), 1),
                ("e2".to_string(), 4),
                ("e3".to_string(), 8),
                ("e4".to_string(), 11),
                ("e5".to_string(), 13),
            ])
        );
    }

    #[test]
    fn long_names_are_cut_and_cycles_end() {
        let long = "가".repeat(400);
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "button"}, "name": {"value": long}, "childIds": ["2"]},
            {"nodeId": "2", "parentId": "1", "role": {"value": "button"}, "name": {"value": "x"}, "childIds": ["2"]},
        ]});
        let snapshot = format_tree(&tree);
        assert!(
            snapshot.lines[0].chars().count() < 200,
            "{}",
            snapshot.lines[0]
        );
        assert!(snapshot.lines[0].ends_with("…\""));
        let paragraph = "word ".repeat(300);
        let text = format_tree(&json!({"nodes": [
            {"nodeId": "1", "role": {"value": "StaticText"}, "name": {"value": paragraph}},
        ]}));
        assert_eq!(
            text.lines[0],
            format!("text \"{}\"", paragraph.trim()),
            "page text is kept whole"
        );
        // A node without backendDOMNodeId has no ref.
        assert!(snapshot.refs.is_empty());
        assert!(snapshot.lines.len() < 10);
        assert_eq!(format_tree(&json!({})), Snapshot::default());
    }

    #[test]
    fn a_deeply_nested_page_neither_overflows_the_stack_nor_runs_unbounded() {
        // A page of 100 000 nested elements (named, so none is transparent).
        let count = 100_000;
        let nodes = (0..count)
            .map(|index| {
                let mut node = json!({
                    "nodeId": index.to_string(),
                    "role": {"value": "button"},
                    "name": {"value": format!("b{index}")},
                    "backendDOMNodeId": index + 1,
                });
                if index > 0 {
                    node["parentId"] = json!((index - 1).to_string());
                }
                if index + 1 < count {
                    node["childIds"] = json!([(index + 1).to_string()]);
                }
                node
            })
            .collect::<Vec<_>>();
        // Run on a small stack: recursion one frame per level would abort.
        let snapshot = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || format_tree(&json!({ "nodes": nodes })))
            .unwrap()
            .join()
            .expect("formatting a deep tree never overflows the stack");
        assert_eq!(snapshot.refs.len(), MAX_TREE_DEPTH);
        assert_eq!(snapshot.lines.len(), MAX_TREE_DEPTH + 1);
        assert!(snapshot.lines.last().unwrap().contains("omitted"));
        // Order is kept: parents before children.
        assert!(snapshot.lines[0].contains("\"b0\""));
        assert!(snapshot.lines[1].contains("\"b1\""));
    }

    #[test]
    fn pages_stay_within_the_budget_and_continue_by_offset() {
        let lines = (0..5000)
            .map(|index| format!("  [e{index}] link \"항목 \\\"{index}\\\" 설명\""))
            .collect::<Vec<_>>();
        let build = |text: &str, next: Option<usize>| json!({"tabId": "t1", "untrusted": true, "snapshot": text, "nextOffset": next});
        let mut offset = 0;
        let mut seen = Vec::new();
        let mut pages = 0;
        loop {
            let value = page(&lines, offset, SNAPSHOT_BUDGET, build);
            assert!(double_escaped_len(&value) <= SNAPSHOT_BUDGET);
            let text = value["snapshot"].as_str().unwrap();
            seen.extend(text.lines().map(str::to_string));
            pages += 1;
            match value["nextOffset"].as_u64() {
                Some(next) => {
                    assert!(next as usize > offset);
                    offset = next as usize;
                }
                None => break,
            }
        }
        assert!(pages > 2);
        assert_eq!(seen, lines, "every line exactly once, in order");

        let small = vec!["a".to_string(), "b".to_string()];
        let value = page(&small, 0, SNAPSHOT_BUDGET, build);
        assert_eq!(value["snapshot"], "a\nb");
        assert!(value["nextOffset"].is_null());
        let past_end = page(&small, 9, SNAPSHOT_BUDGET, build);
        assert_eq!(past_end["snapshot"], "");
        assert!(past_end["nextOffset"].is_null());
    }
}
