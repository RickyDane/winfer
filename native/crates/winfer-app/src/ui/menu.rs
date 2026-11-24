//! Context menus (port of `src/components/ContextMenu.tsx`): right-click menus in the sidebar,
//! the pane header's "…" menu and the worktree list.

use super::commands::Command;
use super::*;
use std::rc::Rc;

pub(super) type MenuRun = Rc<dyn Fn(&mut Root, &mut Window, &mut Context<Root>)>;

#[derive(Clone)]
pub(super) enum MenuItem {
    Item { label: String, hint: Option<String>, danger: bool, disabled: bool, run: MenuRun },
    Separator,
}

impl MenuItem {
    pub fn run(label: impl Into<String>, run: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static) -> Self {
        Self::Item { label: label.into(), hint: None, danger: false, disabled: false, run: Rc::new(run) }
    }

    pub fn command(label: impl Into<String>, command: Command) -> Self {
        Self::run(label, move |this, window, cx| this.execute(command.clone(), window, cx))
    }

    pub fn commands(label: impl Into<String>, commands: Vec<Command>) -> Self {
        Self::run(label, move |this, window, cx| {
            for c in &commands {
                this.execute(c.clone(), window, cx);
            }
        })
    }

    pub fn hint(mut self, keys: Option<String>) -> Self {
        if let Self::Item { hint, .. } = &mut self {
            *hint = keys;
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let Self::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    pub fn disabled(mut self, off: bool) -> Self {
        if let Self::Item { disabled, .. } = &mut self {
            *disabled = off;
        }
        self
    }
}

pub(super) struct OpenMenu {
    pub at: Point<Pixels>,
    pub title: Option<String>,
    pub items: Vec<MenuItem>,
}

const MENU_WIDTH: f32 = 240.0;

/// Separators only between groups: none first, last or twice in a row.
pub(super) fn tidy(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::new();
    for item in items {
        if matches!(item, MenuItem::Separator) && (out.is_empty() || matches!(out.last(), Some(MenuItem::Separator))) {
            continue;
        }
        out.push(item);
    }
    while matches!(out.last(), Some(MenuItem::Separator)) {
        out.pop();
    }
    out
}

impl Root {
    pub(super) fn open_menu(&mut self, at: Point<Pixels>, title: Option<String>, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.menu = Some(OpenMenu { at, title, items: tidy(items) });
        cx.notify();
    }

    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.menu.take().is_some();
        cx.notify();
        open
    }

    pub(super) fn copy_with_toast(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast("Copied to the clipboard", cx);
    }

    pub(super) fn render_menu(&self, c: &Colors, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let c = *c;
        let viewport = window.viewport_size();
        let height = menu.items.iter().map(|i| if matches!(i, MenuItem::Separator) { 9.0 } else { 28.0 }).sum::<f32>() + if menu.title.is_some() { 30.0 } else { 8.0 };
        // Kept on screen: flipped left or up when it would run off an edge.
        let x = (menu.at.x / px(1.0)).min(viewport.width / px(1.0) - MENU_WIDTH - 8.0).max(8.0);
        let y = (menu.at.y / px(1.0)).min(viewport.height / px(1.0) - height - 8.0).max(8.0);
        let mut list = div().id("context-menu").absolute().left(px(x)).top(px(y)).w(px(MENU_WIDTH)).flex().flex_col().rounded_lg().border_1().border_color(c.edge).bg(c.raised).p_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        if let Some(title) = &menu.title {
            list = list.child(div().px_2().py_1().text_size(px(11.0)).text_color(c.subtle).truncate().child(title.clone()));
        }
        for (i, item) in menu.items.iter().enumerate() {
            list = match item {
                MenuItem::Separator => list.child(div().my_1().h(px(1.0)).bg(c.border)),
                MenuItem::Item { label, hint, danger, disabled, run } => {
                    let run = run.clone();
                    let off = *disabled;
                    list.child(
                        div().id(("menu-item", i)).flex().items_center().gap_2().rounded_md().px_2().py_1p5().text_xs()
                            .text_color(if *danger { c.danger } else { c.fg })
                            .when(off, |d| d.opacity(0.4))
                            .when(!off, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover)))
                            .child(div().flex_1().truncate().child(label.clone()))
                            .when_some(hint.clone(), |d, h| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(h)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if off {
                                    return;
                                }
                                this.menu = None;
                                run(this, window, cx);
                                cx.notify();
                            })),
                    )
                }
            };
        }
        Some(
            div().id("menu-backdrop").absolute().size_full().occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.close_menu(cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                    this.close_menu(cx);
                }))
                .child(list)
                .into_any_element(),
        )
    }
}
