//! Fuzzy-filtered lists: the model picker, command palette, session browser, subagents, completions.

use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub value: String,
    pub group: String,
    pub current: bool,
}

impl PickItem {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, value: impl Into<String>) -> Self {
        PickItem { label: label.into(), detail: detail.into(), value: value.into(), group: String::new(), current: false }
    }
    pub fn group(mut self, g: impl Into<String>) -> Self {
        self.group = g.into();
        self
    }
    pub fn current(mut self, c: bool) -> Self {
        self.current = c;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Model,
    Palette,
    Session,
    Effort,
    Mode,
    Compaction,
    TrimAt,
    CompactAt,
    /// Live list of subagents; enter jumps to the agent's card.
    Agents,
}

pub struct Picker {
    pub kind: PickKind,
    pub title: String,
    pub items: Vec<PickItem>,
    pub filter: String,
    pub selected: usize,
    pub opened: Instant,
    pub loading: bool,
}

impl Picker {
    pub fn new(kind: PickKind, title: impl Into<String>, items: Vec<PickItem>) -> Self {
        let selected = items.iter().position(|i| i.current).unwrap_or(0);
        let mut p = Picker { kind, title: title.into(), items, filter: String::new(), selected: 0, opened: Instant::now(), loading: false };
        p.selected = p.visible().iter().position(|&i| i == selected).unwrap_or(0);
        p
    }

    /// Indices of matching items, best first when filtering.
    pub fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.items.len()).collect();
        }
        let mut scored: Vec<(i64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let hay = format!("{} {} {}", it.label, it.value, it.detail);
                score(&self.filter, &hay).map(|s| (s, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, i)| i).collect()
    }

    pub fn chosen(&self) -> Option<&PickItem> {
        self.visible().get(self.selected).map(|&i| &self.items[i])
    }

    pub fn move_by(&mut self, d: i32) {
        let n = self.visible().len();
        if n == 0 {
            return;
        }
        self.selected = ((self.selected as i32 + d).rem_euclid(n as i32)) as usize;
    }

    pub fn set_filter(&mut self, f: String) {
        self.filter = f;
        self.selected = 0;
    }
}

/// Subsequence fuzzy score (higher is better), or None when `query` doesn't match.
pub fn score(query: &str, hay: &str) -> Option<i64> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let h: Vec<char> = hay.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let mut score = 0i64;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in h.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 10;
            if last == Some(i.wrapping_sub(1)) {
                score += 15;
            }
            if i == 0 || matches!(h[i - 1], '/' | ' ' | '-' | '_' | '.') {
                score += 20;
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // Contiguous match bonus and shorter strings first.
    if hay.to_lowercase().contains(&query.to_lowercase()) {
        score += 50;
    }
    Some(score - h.len() as i64 / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_tight_matches() {
        let a = score("opus", "claude/opus").unwrap();
        let b = score("opus", "o p u s something").unwrap();
        assert!(a > b);
        assert!(score("xyz", "claude").is_none());
    }

    #[test]
    fn picker_filters_and_wraps() {
        let items = vec![PickItem::new("Alpha", "", "a"), PickItem::new("Beta", "", "b").current(true)];
        let mut p = Picker::new(PickKind::Palette, "t", items);
        assert_eq!(p.chosen().unwrap().value, "b");
        p.move_by(1);
        assert_eq!(p.chosen().unwrap().value, "a");
        p.set_filter("bet".into());
        assert_eq!(p.visible(), vec![1]);
    }
}
