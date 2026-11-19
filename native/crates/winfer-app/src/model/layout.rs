//! The tiled pane layout: pure data and functions (port of `src/features/layout/tree.ts`). The
//! JSON shape is the Tauri app's, so saved layouts and exported data move between the two.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    Row,
    Col,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LayoutNode {
    Leaf {
        #[serde(rename = "paneId")]
        pane_id: String,
    },
    Split {
        dir: SplitDir,
        sizes: Vec<f64>,
        children: Vec<LayoutNode>,
    },
}

use LayoutNode::{Leaf, Split};

pub fn leaf(id: &str) -> LayoutNode {
    Leaf { pane_id: id.to_string() }
}

pub fn split(dir: SplitDir, children: Vec<LayoutNode>, sizes: Option<Vec<f64>>) -> LayoutNode {
    let sizes = sizes.unwrap_or_else(|| vec![1.0 / children.len() as f64; children.len()]);
    Split { dir, sizes, children }
}

pub fn leaves(node: &LayoutNode) -> Vec<String> {
    match node {
        Leaf { pane_id } => vec![pane_id.clone()],
        Split { children, .. } => children.iter().flat_map(leaves).collect(),
    }
}

/// Dwindle insertion: the target leaf splits in two, alternating direction by depth, so repeated
/// inserts spiral. Without a target the new pane splits the last leaf.
pub fn insert_dwindle(root: Option<&LayoutNode>, new_id: &str, target: Option<&str>) -> LayoutNode {
    let Some(root) = root else { return leaf(new_id) };
    let ids = leaves(root);
    let target = target.filter(|t| ids.iter().any(|id| id == t)).unwrap_or_else(|| ids.last().unwrap());

    fn visit(node: &LayoutNode, depth: usize, target: &str, new_id: &str) -> LayoutNode {
        match node {
            Leaf { pane_id } if pane_id == target => {
                let dir = if depth % 2 == 0 { SplitDir::Row } else { SplitDir::Col };
                split(dir, vec![node.clone(), leaf(new_id)], None)
            }
            Leaf { .. } => node.clone(),
            Split { dir, sizes, children } => Split {
                dir: *dir,
                sizes: sizes.clone(),
                children: children.iter().map(|c| visit(c, depth + 1, target, new_id)).collect(),
            },
        }
    }
    visit(root, 0, target, new_id)
}

/// Removes a leaf; splits left with one child collapse into that child.
pub fn remove_leaf(root: Option<&LayoutNode>, id: &str) -> Option<LayoutNode> {
    match root? {
        Leaf { pane_id } => (pane_id != id).then(|| leaf(pane_id)),
        Split { dir, sizes, children } => {
            let mut kept: Vec<(LayoutNode, f64)> = Vec::new();
            for (i, child) in children.iter().enumerate() {
                if let Some(next) = remove_leaf(Some(child), id) {
                    kept.push((next, sizes[i]));
                }
            }
            match kept.len() {
                0 => None,
                1 => Some(kept.remove(0).0),
                _ => {
                    let total: f64 = kept.iter().map(|k| k.1).sum();
                    let sizes = kept.iter().map(|k| k.1 / total).collect();
                    Some(Split { dir: *dir, sizes, children: kept.into_iter().map(|k| k.0).collect() })
                }
            }
        }
    }
}

pub type Path = Vec<usize>;

const MIN_SIZE: f64 = 0.1;

/// Replaces the sizes of the split at `path` (used by gutter drags).
pub fn set_sizes_at(root: &LayoutNode, path: &[usize], new_sizes: &[f64]) -> LayoutNode {
    let Split { dir, sizes, children } = root else { return root.clone() };
    let Some((i, rest)) = path.split_first() else {
        return Split { dir: *dir, sizes: new_sizes.to_vec(), children: children.clone() };
    };
    Split {
        dir: *dir,
        sizes: sizes.clone(),
        children: children
            .iter()
            .enumerate()
            .map(|(j, c)| if j == *i { set_sizes_at(c, rest, new_sizes) } else { c.clone() })
            .collect(),
    }
}

/// Index path from the root to a leaf.
pub fn path_to(root: &LayoutNode, id: &str) -> Option<Path> {
    match root {
        Leaf { pane_id } => (pane_id == id).then(Vec::new),
        Split { children, .. } => children.iter().enumerate().find_map(|(i, c)| {
            path_to(c, id).map(|mut sub| {
                sub.insert(0, i);
                sub
            })
        }),
    }
}

fn node_at<'a>(root: &'a LayoutNode, path: &[usize]) -> &'a LayoutNode {
    path.iter().fold(root, |node, i| match node {
        Split { children, .. } => &children[*i],
        Leaf { .. } => node,
    })
}

/// Grows (`delta > 0`) or shrinks the pane along an axis, moving space from or to its neighbor in
/// the deepest enclosing split of that direction.
pub fn resize_pane(root: &LayoutNode, id: &str, axis: SplitDir, delta: f64) -> LayoutNode {
    let Some(path) = path_to(root, id) else { return root.clone() };
    for depth in (0..path.len()).rev() {
        let split_path = &path[..depth];
        let Split { dir, sizes, children } = node_at(root, split_path) else { continue };
        if *dir != axis {
            continue;
        }
        let i = path[depth];
        let j = if i + 1 < children.len() { i + 1 } else { i.wrapping_sub(1) };
        let mut sizes = sizes.clone();
        let d = if delta > 0.0 { delta.min(sizes[j] - MIN_SIZE) } else { delta.max(MIN_SIZE - sizes[i]) };
        if d == 0.0 || d.is_nan() {
            return root.clone();
        }
        sizes[i] += d;
        sizes[j] -= d;
        return set_sizes_at(root, split_path, &sizes);
    }
    root.clone()
}

pub fn swap_leaves(root: &LayoutNode, a: &str, b: &str) -> LayoutNode {
    match root {
        Leaf { pane_id } if pane_id == a => leaf(b),
        Leaf { pane_id } if pane_id == b => leaf(a),
        Leaf { .. } => root.clone(),
        Split { dir, sizes, children } => Split {
            dir: *dir,
            sizes: sizes.clone(),
            children: children.iter().map(|c| swap_leaves(c, a, b)).collect(),
        },
    }
}

pub fn equalize(root: &LayoutNode) -> LayoutNode {
    match root {
        Leaf { .. } => root.clone(),
        Split { dir, children, .. } => {
            let children: Vec<_> = children.iter().map(equalize).collect();
            Split { dir: *dir, sizes: vec![1.0 / children.len() as f64; children.len()], children }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Auto,
    MainStack,
    Grid,
    Columns,
    Rows,
}

pub const PRESETS: [Preset; 5] = [Preset::Auto, Preset::MainStack, Preset::Grid, Preset::Columns, Preset::Rows];
pub const GRID: usize = 2;

/// Rebuilds the layout for the given panes (in order) from a preset.
pub fn build_preset(preset: Preset, ids: &[String]) -> Option<LayoutNode> {
    let leaf_of = |ids: &[String]| ids.iter().map(|id| leaf(id)).collect::<Vec<_>>();
    match ids.len() {
        0 => return None,
        1 => return Some(leaf(&ids[0])),
        _ => {}
    }
    Some(match preset {
        Preset::Columns => split(SplitDir::Row, leaf_of(ids), None),
        Preset::Rows => split(SplitDir::Col, leaf_of(ids), None),
        Preset::MainStack => {
            let rest = if ids.len() == 2 { leaf(&ids[1]) } else { split(SplitDir::Row, leaf_of(&ids[1..]), None) };
            split(SplitDir::Col, vec![leaf(&ids[0]), rest], Some(vec![0.6, 0.4]))
        }
        Preset::Grid => {
            let cols = (ids.len() as f64).sqrt().ceil() as usize;
            let mut rows: Vec<LayoutNode> = ids
                .chunks(cols)
                .map(|row| if row.len() == 1 { leaf(&row[0]) } else { split(SplitDir::Row, leaf_of(row), None) })
                .collect();
            if rows.len() == 1 { rows.remove(0) } else { split(SplitDir::Col, rows, None) }
        }
        Preset::Auto => {
            let mut tree = None;
            for (i, id) in ids.iter().enumerate() {
                let target = (i > 0).then(|| ids[i - 1].as_str());
                tree = Some(insert_dwindle(tree.as_ref(), id, target));
            }
            tree?
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropSide {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

/// Moves `id` next to `target` on `side`, or swaps the two for `Center`. Joining a split that
/// already runs in that direction adds a sibling instead of nesting.
pub fn move_pane(root: &LayoutNode, id: &str, target: &str, side: DropSide) -> LayoutNode {
    if id == target {
        return root.clone();
    }
    if side == DropSide::Center {
        return swap_leaves(root, id, target);
    }
    let Some(without) = remove_leaf(Some(root), id) else { return root.clone() };
    let want = if matches!(side, DropSide::Left | DropSide::Right) { SplitDir::Row } else { SplitDir::Col };
    let before = matches!(side, DropSide::Left | DropSide::Top);

    fn visit(node: &LayoutNode, id: &str, target: &str, want: SplitDir, before: bool) -> LayoutNode {
        match node {
            Leaf { pane_id } if pane_id == target => {
                let pair = if before { vec![leaf(id), node.clone()] } else { vec![node.clone(), leaf(id)] };
                split(want, pair, None)
            }
            Leaf { .. } => node.clone(),
            Split { dir, sizes, children } => {
                let at = children.iter().position(|c| matches!(c, Leaf { pane_id } if pane_id == target));
                if let Some(i) = at.filter(|_| *dir == want) {
                    // Split the target's share between it and the dropped pane.
                    let mut children = children.clone();
                    let mut sizes = sizes.clone();
                    let half = sizes[i] / 2.0;
                    children.insert(if before { i } else { i + 1 }, leaf(id));
                    sizes.splice(i..=i, [half, half]);
                    return Split { dir: *dir, sizes, children };
                }
                Split {
                    dir: *dir,
                    sizes: sizes.clone(),
                    children: children.iter().map(|c| visit(c, id, target, want, before)).collect(),
                }
            }
        }
    }
    visit(&without, id, target, want, before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwindle_alternates_direction() {
        let a = insert_dwindle(None, "a", None);
        let b = insert_dwindle(Some(&a), "b", Some("a"));
        let c = insert_dwindle(Some(&b), "c", Some("b"));
        let Split { dir, children, .. } = &c else { panic!() };
        assert_eq!(*dir, SplitDir::Row);
        assert!(matches!(&children[1], Split { dir: SplitDir::Col, .. }));
        assert_eq!(leaves(&c), ["a", "b", "c"]);
    }

    #[test]
    fn removing_collapses_and_renormalizes() {
        let tree = split(SplitDir::Row, vec![leaf("a"), leaf("b"), leaf("c")], Some(vec![0.5, 0.25, 0.25]));
        let Some(Split { sizes, .. }) = remove_leaf(Some(&tree), "c") else { panic!() };
        assert!((sizes[0] - 2.0 / 3.0).abs() < 1e-9);
        let two = split(SplitDir::Row, vec![leaf("a"), leaf("b")], None);
        assert_eq!(remove_leaf(Some(&two), "b"), Some(leaf("a")));
    }

    #[test]
    fn moving_into_same_direction_adds_a_sibling() {
        let tree = split(SplitDir::Row, vec![leaf("a"), leaf("b"), leaf("c")], None);
        let moved = move_pane(&tree, "c", "a", DropSide::Left);
        let Split { children, sizes, .. } = &moved else { panic!() };
        assert_eq!(leaves(&moved), ["c", "a", "b"]);
        assert_eq!(children.len(), 3);
        assert!((sizes[0] - sizes[1]).abs() < 1e-9);
    }

    #[test]
    fn json_matches_the_tauri_app() {
        let tree = split(SplitDir::Row, vec![leaf("a"), leaf("b")], None);
        let json = serde_json::to_value(&tree).unwrap();
        assert_eq!(json["kind"], "split");
        assert_eq!(json["dir"], "row");
        assert_eq!(json["children"][0]["kind"], "leaf");
        assert_eq!(json["children"][0]["paneId"], "a");
        assert_eq!(serde_json::from_value::<LayoutNode>(json).unwrap(), tree);
    }

    #[test]
    fn resize_respects_minimum() {
        let tree = split(SplitDir::Row, vec![leaf("a"), leaf("b")], None);
        let Split { sizes, .. } = resize_pane(&tree, "a", SplitDir::Row, 0.9) else { panic!() };
        assert!((sizes[1] - MIN_SIZE).abs() < 1e-9);
    }

    #[test]
    fn grid_preset_tiles_rows() {
        let ids: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let Some(Split { dir, children, .. }) = build_preset(Preset::Grid, &ids) else { panic!() };
        assert_eq!(dir, SplitDir::Col);
        assert_eq!(children.len(), 2);
    }
}
