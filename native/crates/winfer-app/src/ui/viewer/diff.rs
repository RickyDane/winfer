//! The viewer's diff mode: the shown file against its last commit (git `HEAD`), inline or side by
//! side. The committed copy comes from `git show`; the diff itself is computed here and follows the
//! file as it changes on disk.

use super::{Content, ViewerPanel, expand};
use crate::backend;
use crate::theme::Colors;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use similar::{ChangeTag, TextDiff};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::Duration;

/// Unchanged lines kept around each change.
const CONTEXT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Same,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub kind: Kind,
    /// 1-based line numbers in the committed and the current file.
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: SharedString,
}

/// A row of the inline diff.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Hunk(SharedString),
    Line(Line),
}

/// A row of the side-by-side diff: the committed line on the left, the current one on the right.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitRow {
    Hunk(SharedString),
    Pair(Option<Line>, Option<Line>),
}

#[derive(Debug, Default)]
pub struct FileDiff {
    pub inline: Vec<Row>,
    pub split: Vec<SplitRow>,
    pub added: usize,
    pub removed: usize,
    /// The highest line number shown, for the gutter's width.
    pub last_line: usize,
}

fn clean(s: &str) -> SharedString {
    let s = s.trim_end_matches(['\n', '\r']).replace('\t', "    ");
    if s.is_empty() { " ".into() } else { s.into() }
}

/// Pairs up the removed and added lines of one change, side by side.
fn pair(rows: &mut Vec<SplitRow>, removed: &mut Vec<Line>, added: &mut Vec<Line>) {
    let n = removed.len().max(added.len());
    let (mut r, mut a) = (removed.drain(..), added.drain(..));
    for _ in 0..n {
        rows.push(SplitRow::Pair(r.next(), a.next()));
    }
}

/// `old` → `new` line by line, in hunks with a few lines of context.
pub fn compute(old: &str, new: &str) -> FileDiff {
    let (old, new) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
    let diff = TextDiff::configure().timeout(Duration::from_secs(2)).diff_lines(old.as_str(), new.as_str());
    let mut out = FileDiff::default();
    for group in diff.grouped_ops(CONTEXT) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let (o, n) = (first.old_range().start..last.old_range().end, first.new_range().start..last.new_range().end);
        let header: SharedString = format!("@@ -{},{} +{},{} @@", o.start + 1, o.len(), n.start + 1, n.len()).into();
        out.inline.push(Row::Hunk(header.clone()));
        out.split.push(SplitRow::Hunk(header));
        out.last_line = out.last_line.max(o.end).max(n.end);
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        for op in &group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => Kind::Same,
                    ChangeTag::Insert => Kind::Added,
                    ChangeTag::Delete => Kind::Removed,
                };
                let line = Line { kind, old: change.old_index().map(|i| i + 1), new: change.new_index().map(|i| i + 1), text: clean(change.value()) };
                out.inline.push(Row::Line(line.clone()));
                match kind {
                    Kind::Removed => {
                        out.removed += 1;
                        removed.push(line);
                    }
                    Kind::Added => {
                        out.added += 1;
                        added.push(line);
                    }
                    Kind::Same => {
                        pair(&mut out.split, &mut removed, &mut added);
                        out.split.push(SplitRow::Pair(Some(line.clone()), Some(line)));
                    }
                }
            }
        }
        pair(&mut out.split, &mut removed, &mut added);
    }
    out
}

/// What the file is compared with.
enum Base {
    Committed(String),
    /// In a repository but not in its last commit (new, untracked or ignored).
    New,
    NotGit,
}

fn git(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output().map_err(|e| format!("Couldn't run git: {e}"))
}

fn committed(path: &str) -> Result<Base, String> {
    let file = std::path::PathBuf::from(expand(path));
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else { return Err("Not a file.".into()) };
    if !git(dir, &["rev-parse", "--is-inside-work-tree"])?.status.success() {
        return Ok(Base::NotGit);
    }
    let shown = git(dir, &["show", &format!("HEAD:./{name}")])?;
    Ok(if shown.status.success() { Base::Committed(String::from_utf8_lossy(&shown.stdout).into_owned()) } else { Base::New })
}

#[derive(Default)]
enum DiffState {
    #[default]
    Loading,
    Failed(String),
    NotGit,
    Ready { diff: Arc<FileDiff>, new_file: bool },
}

pub(super) struct DiffView {
    pub on: bool,
    pub split: bool,
    /// The file and modification time the diff is for.
    key: Option<(String, u64)>,
    state: DiffState,
    list: ListState,
    task: Option<Task<()>>,
}

impl Default for DiffView {
    fn default() -> Self {
        Self { on: false, split: false, key: None, state: DiffState::Loading, list: ListState::new(0, ListAlignment::Top, px(600.0)), task: None }
    }
}

impl DiffView {
    /// Added and removed line counts, once known.
    pub fn counts(&self) -> Option<(usize, usize)> {
        match &self.state {
            DiffState::Ready { diff, .. } => Some((diff.added, diff.removed)),
            _ => None,
        }
    }

    fn rows(&self) -> usize {
        match &self.state {
            DiffState::Ready { diff, .. } if self.split => diff.split.len(),
            DiffState::Ready { diff, .. } => diff.inline.len(),
            _ => 0,
        }
    }

    /// Fits the list to the rows, keeping the scroll position when `keep`.
    fn reset_list(&mut self, keep: bool) {
        let top = keep.then(|| self.list.logical_scroll_top());
        self.list.reset(self.rows());
        if let Some(mut top) = top {
            top.item_ix = top.item_ix.min(self.rows().saturating_sub(1));
            self.list.scroll_to(top);
        }
    }
}

impl ViewerPanel {
    /// Whether the body shows the diff instead of the file.
    pub(super) fn diff_shown(&self) -> bool {
        self.diff.on && !self.editing && matches!(self.content, Content::Text { .. })
    }

    pub(super) fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        self.diff.on = !self.diff.on;
        self.diff.key = None;
        cx.notify();
    }

    pub(super) fn set_diff_split(&mut self, split: bool, cx: &mut Context<Self>) {
        if self.diff.split != split {
            self.diff.split = split;
            self.diff.reset_list(false);
            cx.notify();
        }
    }

    /// Compares again with the last commit, which may have moved on.
    pub(super) fn refresh_diff(&mut self) {
        self.diff.key = None;
    }

    /// Starts computing the diff when the file or its contents changed since the last one.
    fn sync_diff(&mut self, cx: &mut Context<Self>) {
        if !self.diff_shown() {
            return;
        }
        let (Some(path), Content::Text { text, modified, .. }) = (self.shown.clone(), &self.content) else { return };
        let key = (path.clone(), *modified);
        if self.diff.key.as_ref() == Some(&key) {
            return;
        }
        let same_file = self.diff.key.as_ref().is_some_and(|(p, _)| *p == path) && matches!(self.diff.state, DiffState::Ready { .. });
        if !same_file {
            self.diff.state = DiffState::Loading;
            self.diff.reset_list(false);
        }
        self.diff.key = Some(key.clone());
        let text = text.to_string();
        self.diff.task = Some(cx.spawn(async move |this, cx| {
            let result = backend::background(move || {
                committed(&path).map(|base| match base {
                    Base::NotGit => None,
                    Base::Committed(old) => Some((compute(&old, &text), false)),
                    Base::New => Some((compute("", &text), true)),
                })
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.diff.key.as_ref() != Some(&key) {
                    return;
                }
                this.diff.state = match result {
                    Ok(None) => DiffState::NotGit,
                    Ok(Some((diff, new_file))) => DiffState::Ready { diff: Arc::new(diff), new_file },
                    Err(err) => DiffState::Failed(err),
                };
                this.diff.reset_list(same_file);
                cx.notify();
            });
        }));
    }

    pub(super) fn render_diff(&mut self, font: SharedString, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        self.sync_diff(cx);
        let message = |text: SharedString, color: Hsla| div().p_4().text_xs().text_color(color).child(text).into_any_element();
        let (diff, new_file) = match &self.diff.state {
            DiffState::Loading => return message("Loading changes…".into(), c.subtle),
            DiffState::Failed(err) => return message(err.clone().into(), c.danger),
            DiffState::NotGit => return message("This file isn't in a git repository, so there's no commit to compare it with.".into(), c.subtle),
            DiffState::Ready { diff, .. } if diff.inline.is_empty() => return message("No changes since the last commit.".into(), c.subtle),
            DiffState::Ready { diff, new_file } => (diff.clone(), *new_file),
        };
        let split = self.diff.split;
        let gutter = px(diff.last_line.max(1).to_string().len() as f32 * 7.5 + 14.0);
        let rows = list(self.diff.list.clone(), move |ix, _, _| {
            if split {
                diff.split.get(ix).map_or_else(|| div().into_any_element(), |row| split_row(row, gutter, c))
            } else {
                diff.inline.get(ix).map_or_else(|| div().into_any_element(), |row| inline_row(row, gutter, c))
            }
        })
        .size_full();
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(font)
            .text_size(px(12.0))
            .line_height(px(18.0))
            .when(new_file, |d| d.child(div().px_3().py_1().text_xs().text_color(c.subtle).child("New file: not in the last commit")))
            .child(div().flex_1().min_h_0().child(rows))
            .into_any_element()
    }
}

fn tint(kind: Kind, c: Colors) -> Option<Hsla> {
    match kind {
        Kind::Added => Some(Hsla { a: 0.12, ..c.success }),
        Kind::Removed => Some(Hsla { a: 0.12, ..c.danger }),
        Kind::Same => None,
    }
}

fn marker(kind: Kind, c: Colors) -> Div {
    let (sign, color) = match kind {
        Kind::Added => ("+", c.success),
        Kind::Removed => ("-", c.danger),
        Kind::Same => (" ", c.subtle),
    };
    div().flex_none().w(px(14.0)).text_color(color).child(sign)
}

fn number(n: Option<usize>, width: Pixels, c: Colors) -> Div {
    div().flex_none().w(width).pr_2().text_right().text_color(c.subtle).child(n.map(|n| n.to_string()).unwrap_or_default())
}

fn text(line: &Line, c: Colors) -> Div {
    div().flex_1().min_w_0().pr_2().text_color(if line.kind == Kind::Same { c.muted } else { c.fg }).child(line.text.clone())
}

fn hunk(header: &SharedString, c: Colors) -> AnyElement {
    div().w_full().px_3().py_0p5().bg(c.raised).text_size(px(11.0)).text_color(c.accent).child(header.clone()).into_any_element()
}

fn inline_row(row: &Row, gutter: Pixels, c: Colors) -> AnyElement {
    match row {
        Row::Hunk(header) => hunk(header, c),
        Row::Line(line) => div()
            .w_full()
            .flex()
            .when_some(tint(line.kind, c), |d, bg| d.bg(bg))
            .child(number(line.old, gutter, c))
            .child(number(line.new, gutter, c))
            .child(marker(line.kind, c))
            .child(text(line, c))
            .into_any_element(),
    }
}

/// One half of a side-by-side row: the left shows committed line numbers, the right current ones.
fn side(line: Option<&Line>, left: bool, gutter: Pixels, c: Colors) -> Div {
    let half = div().flex_1().min_w_0().flex().when(!left, |d| d.border_l_1().border_color(c.edge));
    match line {
        None => half.bg(Hsla { a: 0.5, ..c.raised }),
        Some(line) => half
            .when_some(tint(line.kind, c), |d, bg| d.bg(bg))
            .child(number(if left { line.old } else { line.new }, gutter, c))
            .child(marker(line.kind, c))
            .child(text(line, c)),
    }
}

fn split_row(row: &SplitRow, gutter: Pixels, c: Colors) -> AnyElement {
    match row {
        SplitRow::Hunk(header) => hunk(header, c),
        SplitRow::Pair(old, new) => div().w_full().flex().child(side(old.as_ref(), true, gutter, c)).child(side(new.as_ref(), false, gutter, c)).into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, Row, SplitRow, compute};

    fn kinds(rows: &[Row]) -> Vec<Option<Kind>> {
        rows.iter().map(|r| if let Row::Line(l) = r { Some(l.kind) } else { None }).collect()
    }

    #[test]
    fn identical_files_have_no_hunks() {
        let d = compute("a\nb\n", "a\r\nb\r\n");
        assert!(d.inline.is_empty() && d.split.is_empty());
        assert_eq!((d.added, d.removed), (0, 0));
    }

    #[test]
    fn a_changed_line_is_removed_then_added_and_paired() {
        let d = compute("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!((d.added, d.removed), (2, 1));
        assert_eq!(kinds(&d.inline), [None, Some(Kind::Same), Some(Kind::Removed), Some(Kind::Added), Some(Kind::Same), Some(Kind::Added)]);
        let SplitRow::Pair(Some(old), Some(new)) = &d.split[2] else { panic!("{:?}", d.split[2]) };
        assert_eq!((old.old, old.text.as_ref(), new.new, new.text.as_ref()), (Some(2), "b", Some(2), "B"));
        let SplitRow::Pair(None, Some(added)) = &d.split[4] else { panic!("{:?}", d.split[4]) };
        assert_eq!(added.new, Some(4));
        assert_eq!(d.last_line, 4);
    }

    #[test]
    fn far_apart_changes_get_their_own_hunks() {
        let old: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 2\n", "two\n").replace("line 28\n", "twenty-eight\n");
        let d = compute(&old, &new);
        let hunks: Vec<_> = d.inline.iter().filter_map(|r| if let Row::Hunk(h) = r { Some(h.to_string()) } else { None }).collect();
        assert_eq!(hunks, ["@@ -1,5 +1,5 @@", "@@ -25,6 +25,6 @@"]);
    }
}
