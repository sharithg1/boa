//! JsonViewer.tsx: the viewer for json / jsonb cells, opened from the
//! result grid (Enter on a JSON cell of a read-only grid, double-click, or
//! the braces button). A collapsible tree (objects and arrays fold, showing
//! their key / item count when folded; the first two levels start open) or
//! the pretty-printed text, both coloured like the grid; Expand all /
//! Collapse all, Copy (the pretty text) and Esc / × to close.
//!
//! The tree is flattened into lines and drawn with `uniform_list`, so a
//! large document costs a screenful per frame. Lines don't wrap (the Tauri
//! viewer breaks long strings); they scroll sideways instead.

use std::ops::Range;
use std::time::Duration;

use boa_core::grid::values::{JsonNode, JsonTokenKind, format_json, json_tokens, parse_json};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, Hsla, ListHorizontalSizingBehavior, SharedString, StyledText,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use gpui_component::Icon;
use gpui_component::scroll::Scrollbar;

use crate::views::chrome::IconName;

use crate::actions::Dismiss;
use crate::app_state::AppState;
use crate::grid::ResultGrid;
use crate::models::toasts::NewToast;
use crate::theme::{self, ActivePalette as _, Palette};

/// Key context of the viewer (Esc closes).
pub const CONTEXT: &str = "JsonViewer";

/// Line height of the tree and text.
const LINE_HEIGHT: f32 = 20.;
/// Indent per level (`ml-[7px] pl-[11px]`) and the chevron column (`pl-[18px]`).
const INDENT: f32 = 18.;

/// The cell to show (`JsonTarget`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonTarget {
    pub column: String,
    pub ty: String,
    /// Row number as the grid shows it.
    pub row: usize,
    pub value: String,
}

/// The viewer asks to be closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonViewerEvent {
    Close,
}

/// Tree or pretty text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tree,
    Text,
}

/// How a node is labelled: the root has none, array items their index,
/// object entries their key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Label {
    None,
    Index(usize),
    Key(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Leaf { token: JsonTokenKind, src: String },
    Branch { object: bool, count: usize },
}

/// One node of the document in preorder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    depth: usize,
    label: Label,
    kind: Kind,
    /// Index after this node's subtree.
    end: usize,
    /// Last child of its parent (no trailing comma).
    last: bool,
}

/// A line of the tree view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// A scalar, or an empty `{}` / `[]`.
    Leaf(usize),
    /// An open object / array: its first line.
    Open(usize),
    /// Its closing bracket.
    Close(usize),
    /// A folded object / array (one line with the count).
    Folded(usize),
}

/// The parsed document, flattened, with the open state of each branch.
#[derive(Debug, Clone)]
pub struct JsonTree {
    nodes: Vec<Node>,
    open: Vec<bool>,
    lines: Vec<Line>,
}

impl JsonTree {
    pub fn new(root: &JsonNode) -> Self {
        let mut nodes = Vec::new();
        flatten(root, 0, Label::None, true, &mut nodes);
        // The first two levels start open (`depth < 2`).
        let open = nodes.iter().map(|n| n.depth < 2).collect();
        let mut tree = Self {
            nodes,
            open,
            lines: Vec::new(),
        };
        tree.relayout();
        tree
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    fn relayout(&mut self) {
        let mut lines = Vec::with_capacity(self.nodes.len());
        let mut closes: Vec<(usize, usize)> = Vec::new(); // (end, node)
        let mut i = 0;
        while i < self.nodes.len() {
            while let Some(&(end, node)) = closes.last() {
                if end > i {
                    break;
                }
                lines.push(Line::Close(node));
                closes.pop();
            }
            let node = &self.nodes[i];
            match node.kind {
                Kind::Branch { count, .. } if count > 0 => {
                    if self.open[i] {
                        lines.push(Line::Open(i));
                        closes.push((node.end, i));
                        i += 1;
                    } else {
                        lines.push(Line::Folded(i));
                        i = node.end;
                    }
                }
                _ => {
                    lines.push(Line::Leaf(i));
                    i += 1;
                }
            }
        }
        while let Some((_, node)) = closes.pop() {
            lines.push(Line::Close(node));
        }
        self.lines = lines;
    }

    /// Opens / folds the branch of `node`.
    pub fn toggle(&mut self, node: usize) {
        if let Some(open) = self.open.get_mut(node) {
            *open = !*open;
            self.relayout();
        }
    }

    /// Expand all / Collapse all.
    pub fn set_all(&mut self, open: bool) {
        self.open.iter_mut().for_each(|o| *o = open);
        self.relayout();
    }

    /// The text of a line as copied / tested (indent excluded).
    pub fn line_text(&self, line: Line) -> String {
        let label = |n: &Node| match &n.label {
            Label::None => String::new(),
            Label::Index(i) => format!("{i}: "),
            Label::Key(k) => format!("{}: ", key_text(k)),
        };
        let comma = |n: &Node| if n.last { "" } else { "," };
        match line {
            Line::Leaf(i) => {
                let n = &self.nodes[i];
                let body = match &n.kind {
                    Kind::Leaf { src, .. } => src.clone(),
                    Kind::Branch { object: true, .. } => "{}".into(),
                    Kind::Branch { object: false, .. } => "[]".into(),
                };
                format!("{}{body}{}", label(n), comma(n))
            }
            Line::Open(i) => {
                let n = &self.nodes[i];
                format!("{}{}", label(n), brackets(n).0)
            }
            Line::Close(i) => {
                let n = &self.nodes[i];
                format!("{}{}", brackets(n).1, comma(n))
            }
            Line::Folded(i) => {
                let n = &self.nodes[i];
                let (o, c) = brackets(n);
                format!("{}{o} {} {c}{}", label(n), count_text(n), comma(n))
            }
        }
    }
}

fn brackets(n: &Node) -> (&'static str, &'static str) {
    match n.kind {
        Kind::Branch { object: true, .. } => ("{", "}"),
        _ => ("[", "]"),
    }
}

/// "3 keys" / "1 item".
fn count_text(n: &Node) -> String {
    match n.kind {
        Kind::Branch { object, count } => format!(
            "{count} {}{}",
            if object { "key" } else { "item" },
            if count == 1 { "" } else { "s" }
        ),
        Kind::Leaf { .. } => String::new(),
    }
}

/// A key as `JSON.stringify` writes it.
fn key_text(k: &str) -> String {
    serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\""))
}

fn flatten(node: &JsonNode, depth: usize, label: Label, last: bool, out: &mut Vec<Node>) {
    let at = out.len();
    let leaf = |token, src: &String| Kind::Leaf {
        token,
        src: src.clone(),
    };
    let kind = match node {
        JsonNode::Object(entries) => Kind::Branch {
            object: true,
            count: entries.len(),
        },
        JsonNode::Array(items) => Kind::Branch {
            object: false,
            count: items.len(),
        },
        JsonNode::String(s) => leaf(JsonTokenKind::String, s),
        JsonNode::Number(s) => leaf(JsonTokenKind::Number, s),
        JsonNode::Literal(s) => leaf(JsonTokenKind::Literal, s),
    };
    out.push(Node {
        depth,
        label,
        kind,
        end: at + 1,
        last,
    });
    match node {
        JsonNode::Object(entries) => {
            for (i, (k, v)) in entries.iter().enumerate() {
                flatten(
                    v,
                    depth + 1,
                    Label::Key(k.clone()),
                    i + 1 == entries.len(),
                    out,
                );
            }
        }
        JsonNode::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                flatten(v, depth + 1, Label::Index(i), i + 1 == items.len(), out);
            }
        }
        _ => {}
    }
    out[at].end = out.len();
}

/// The JSON viewer.
pub struct JsonViewer {
    focus_handle: FocusHandle,
    target: JsonTarget,
    mode: Mode,
    tree: Result<JsonTree, String>,
    pretty: String,
    /// The pretty text split in lines, token-coloured on demand.
    text_lines: Vec<String>,
    scroll: UniformListScrollHandle,
}

impl std::fmt::Debug for JsonViewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonViewer")
            .field("target", &self.target)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<JsonViewerEvent> for JsonViewer {}

impl JsonViewer {
    pub fn new(target: JsonTarget, cx: &mut Context<Self>) -> Self {
        let tree = parse_json(&target.value)
            .map(|node| JsonTree::new(&node))
            .map_err(|e| e.message);
        let pretty = format_json(&target.value);
        let text_lines = pretty.lines().map(str::to_owned).collect();
        Self {
            focus_handle: cx.focus_handle(),
            target,
            mode: Mode::Tree,
            tree,
            pretty,
            text_lines,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn target(&self) -> &JsonTarget {
        &self.target
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn tree(&self) -> Option<&JsonTree> {
        self.tree.as_ref().ok()
    }

    /// The pretty-printed text (what Copy copies).
    pub fn pretty(&self) -> &str {
        &self.pretty
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    pub fn toggle(&mut self, node: usize, cx: &mut Context<Self>) {
        if let Ok(tree) = &mut self.tree {
            tree.toggle(node);
            cx.notify();
        }
    }

    pub fn set_all(&mut self, open: bool, cx: &mut Context<Self>) {
        if let Ok(tree) = &mut self.tree {
            tree.set_all(open);
            cx.notify();
        }
    }

    /// Copies the pretty text ("Copied JSON").
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.pretty.clone()));
        if let Some(state) = AppState::try_global(cx) {
            let toasts = state.toasts.clone();
            toasts.update(cx, |t, cx| {
                t.push_for(
                    NewToast::info("Copied JSON"),
                    Some(Duration::from_millis(1600)),
                    cx,
                );
            });
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(JsonViewerEvent::Close);
    }

    fn tree_mode(&self) -> bool {
        self.mode == Mode::Tree && self.tree.is_ok()
    }

    fn line_count(&self) -> usize {
        match (&self.tree, self.mode) {
            (Ok(tree), Mode::Tree) => tree.lines.len(),
            _ => self.text_lines.len(),
        }
    }

    fn styled(p: &Palette, parts: Vec<(String, Option<Hsla>)>) -> StyledText {
        let mut text = String::new();
        let mut highlights = Vec::new();
        for (s, color) in parts {
            let start = text.len();
            text.push_str(&s);
            if let Some(color) = color {
                highlights.push((
                    start..text.len(),
                    HighlightStyle {
                        color: Some(color),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
        let _ = p;
        StyledText::new(text).with_highlights(highlights)
    }

    fn render_tree_line(
        &self,
        tree: &JsonTree,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let line = tree.lines[ix];
        let (Line::Leaf(n) | Line::Open(n) | Line::Close(n) | Line::Folded(n)) = line;
        let node = &tree.nodes[n];
        let punct = Some(p.sql_operator);
        let comma = (!node.last).then(|| (",".to_owned(), punct));
        let mut parts: Vec<(String, Option<Hsla>)> = Vec::new();
        let label = |parts: &mut Vec<(String, Option<Hsla>)>| match &node.label {
            Label::None => {}
            Label::Index(i) => parts.push((format!("{i}: "), Some(p.dim))),
            Label::Key(k) => {
                parts.push((key_text(k), Some(p.sql_column)));
                parts.push((": ".into(), punct));
            }
        };
        let (o, c) = brackets(node);
        let mut count = None;
        match line {
            Line::Leaf(_) => {
                label(&mut parts);
                match &node.kind {
                    Kind::Leaf { token, src } => {
                        parts.push((src.clone(), ResultGrid::json_color(p, *token)));
                    }
                    Kind::Branch { .. } => parts.push((format!("{o}{c}"), punct)),
                }
                parts.extend(comma.clone());
            }
            Line::Open(_) => {
                label(&mut parts);
                parts.push((o.into(), punct));
            }
            Line::Folded(_) => {
                label(&mut parts);
                parts.push((o.into(), punct));
                count = Some(count_text(node));
            }
            Line::Close(_) => {
                parts.push((c.into(), punct));
                parts.extend(comma.clone());
            }
        }
        let branch = matches!(line, Line::Open(_) | Line::Folded(_));
        // Guides: one per enclosing open level (`border-l`), skipping the root.
        let depth = match line {
            Line::Close(_) => node.depth,
            _ => node.depth,
        };
        let mut row = div()
            .id(("json-line", ix))
            .relative()
            .h(px(LINE_HEIGHT))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .pl(px(depth as f32 * INDENT));
        for level in 0..depth {
            row = row.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(level as f32 * INDENT + 7.))
                    .w(px(1.))
                    .bg(p.border_subtle),
            );
        }
        if branch {
            let open = matches!(line, Line::Open(_));
            row = row
                .hover(|s| s.bg(p.hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(n, cx)))
                .child(
                    div().flex_none().w(px(14.)).text_color(p.dim).child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(14.)),
                    ),
                )
                .child(div().w(px(4.)));
        } else {
            row = row.child(div().flex_none().w(px(INDENT)));
        }
        row = row.child(Self::styled(p, parts));
        if let Some(count) = count {
            row = row
                .child(
                    div()
                        .mx(px(4.))
                        .text_size(theme::text::XS)
                        .italic()
                        .text_color(p.dim)
                        .child(count),
                )
                .child(Self::styled(
                    p,
                    [Some((c.to_owned(), punct)), comma.clone()]
                        .into_iter()
                        .flatten()
                        .collect(),
                ));
        }
        row.into_any_element()
    }

    fn render_text_line(&self, ix: usize, p: &Palette) -> AnyElement {
        let line = &self.text_lines[ix];
        let parts = json_tokens(line)
            .into_iter()
            .map(|t| {
                let color = ResultGrid::json_color(p, t.kind);
                (t.text, color)
            })
            .collect();
        div()
            .h(px(LINE_HEIGHT))
            .whitespace_nowrap()
            .child(Self::styled(p, parts))
            .into_any_element()
    }

    fn render_lines(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = cx.palette().clone();
        if self.mode == Mode::Tree
            && let Ok(tree) = &self.tree
        {
            range
                .map(|ix| self.render_tree_line(tree, ix, &p, cx))
                .collect()
        } else {
            range.map(|ix| self.render_text_line(ix, &p)).collect()
        }
    }

    /// The widest line, so the list is wide enough to scroll sideways.
    fn widest_line(&self) -> Option<usize> {
        let len = |ix: usize| -> usize {
            match (&self.tree, self.mode) {
                (Ok(tree), Mode::Tree) => {
                    let line = tree.lines[ix];
                    let (Line::Leaf(n) | Line::Open(n) | Line::Close(n) | Line::Folded(n)) = line;
                    tree.nodes[n].depth * 3 + tree.line_text(line).chars().count()
                }
                _ => self.text_lines[ix].chars().count(),
            }
        };
        (0..self.line_count()).max_by_key(|&ix| len(ix))
    }

    fn button(
        id: &'static str,
        label: impl IntoElement,
        selected: bool,
        p: &Palette,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(24.))
            .px(px(8.))
            .rounded(theme::radius::SM)
            .text_size(theme::text::XS)
            .text_color(if selected { p.body } else { p.quiet })
            .when(selected, |d| d.bg(p.hover_strong))
            .hover(|s| s.bg(p.hover).text_color(p.soft))
            .cursor_pointer()
            .child(label)
    }
}

impl Focusable for JsonViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for JsonViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let tree_controls = self.tree_mode();
        let chars = self.target.value.encode_utf16().count();
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(12.))
            .pl(px(16.))
            .pr(px(12.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_family(theme::MONO_FONT)
                            .text_size(theme::text::SM)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.body)
                            .child(SharedString::from(self.target.column.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .text_size(theme::text::XS)
                            .text_color(p.quiet)
                            .child(format!("Row {} · ", self.target.row))
                            .child(
                                div()
                                    .text_color(p.sql_type)
                                    .child(SharedString::from(self.target.ty.clone())),
                            )
                            .child(format!(" · {} chars", crate::grid::page::grouped(chars))),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .when(tree_controls, |d| {
                        d.child(
                            Self::button("json-expand-all", "Expand all", false, &p)
                                .on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))),
                        )
                        .child(
                            Self::button("json-collapse-all", "Collapse all", false, &p)
                                .on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx))),
                        )
                    })
                    .child(
                        div()
                            .ml(px(4.))
                            .flex()
                            .p(px(2.))
                            .rounded(theme::radius::MD)
                            .border_1()
                            .border_color(p.border)
                            .child(
                                Self::button("json-mode-tree", "Tree", self.mode == Mode::Tree, &p)
                                    .h(px(20.))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.set_mode(Mode::Tree, cx)),
                                    ),
                            )
                            .child(
                                Self::button("json-mode-text", "Text", self.mode == Mode::Text, &p)
                                    .h(px(20.))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.set_mode(Mode::Text, cx)),
                                    ),
                            ),
                    )
                    .child(
                        Self::button(
                            "json-copy",
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(Icon::new(IconName::Copy).size(px(14.)))
                                .child("Copy"),
                            false,
                            &p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                    )
                    .child(
                        Self::button(
                            "json-close",
                            Icon::new(IconName::X).size(px(14.)),
                            false,
                            &p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            );
        let invalid = match (&self.tree, self.mode) {
            (Err(message), Mode::Tree) => Some(message.clone()),
            _ => None,
        };
        let list = uniform_list(
            "json-lines",
            self.line_count(),
            cx.processor(|this, range: Range<usize>, _, cx| this.render_lines(range, cx)),
        )
        .track_scroll(&self.scroll)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(self.widest_line())
        .size_full();
        div()
            .id("json-viewer")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.close(cx)))
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::UI_FONT)
            .child(header)
            .when_some(invalid, |d, message| {
                d.child(
                    div()
                        .px(px(16.))
                        .pt(px(12.))
                        .text_size(theme::text::XS)
                        .text_color(p.status_err_text)
                        .child(format!("Not valid JSON: {message}")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .px(px(16.))
                    .py(px(12.))
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::SM)
                    .line_height(px(LINE_HEIGHT))
                    .text_color(p.body)
                    .child(list)
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::new(&self.scroll)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(text: &str) -> JsonTree {
        JsonTree::new(&parse_json(text).unwrap())
    }

    fn texts(t: &JsonTree) -> Vec<String> {
        t.lines().iter().map(|&l| t.line_text(l)).collect()
    }

    #[test]
    fn first_two_levels_start_open() {
        let t = tree(r#"{"a": 1, "b": {"c": [1, 2], "d": {}}, "e": []}"#);
        assert_eq!(
            texts(&t),
            [
                "{",
                "\"a\": 1,",
                "\"b\": {",
                "\"c\": [ 2 items ],",
                "\"d\": {}",
                "},",
                "\"e\": []",
                "}",
            ]
        );
    }

    #[test]
    fn toggling_folds_and_opens_a_branch() {
        let mut t = tree(r#"[{"x": true}, null]"#);
        assert_eq!(
            texts(&t),
            ["[", "0: {", "\"x\": true", "},", "1: null", "]"]
        );
        // Fold item 0 (node 1).
        t.toggle(1);
        assert_eq!(texts(&t), ["[", "0: { 1 key },", "1: null", "]"]);
        t.toggle(0);
        assert_eq!(texts(&t), ["[ 2 items ]"]);
        t.set_all(true);
        assert_eq!(t.lines().len(), 6);
        t.set_all(false);
        assert_eq!(t.lines(), [Line::Folded(0)]);
    }

    #[test]
    fn scalars_are_one_line() {
        let t = tree(r#""just a string""#);
        assert_eq!(texts(&t), ["\"just a string\""]);
    }
}
