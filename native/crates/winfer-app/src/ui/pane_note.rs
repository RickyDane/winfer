//! A pane's private note (port of `features/terminal/PaneNote.tsx`): a sticky-note button in the
//! pane header opens a small popover to jot down what the task is about. It's only for the user,
//! never shown to the agent, and saves as you type.

use super::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};

pub(super) struct NoteUi {
    pane: String,
    input: Entity<TextareaState>,
    at: Point<Pixels>,
}

impl Root {
    pub(super) fn toggle_pane_note(&mut self, pane: &str, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane_note.as_ref().is_some_and(|n| n.pane == pane) {
            self.close_pane_note(cx);
            return;
        }
        let text = self.state.read(cx).pane_notes.get(pane).cloned().unwrap_or_default();
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(6, 12).soft_wrap(true).placeholder("Notes to organize this task…");
            state.set_value(text, window, cx);
            state
        });
        let id = pane.to_string();
        cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let note = input.read(cx).value().to_string();
                this.update_state(cx, |s| s.set_pane_note(&id, &note));
            }
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.pane_note = Some(NoteUi { pane: pane.to_string(), input, at });
        cx.notify();
    }

    pub(super) fn close_pane_note(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.pane_note.take().is_some();
        if was {
            self.focused_view = None;
            cx.notify();
        }
        was
    }

    pub(super) fn render_pane_note(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let note = self.pane_note.as_ref()?;
        let c = *c;
        Some(
            div().id("pane-note-backdrop").absolute().size_full().occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.close_pane_note(cx);
                }))
                .child(
                    div().id("pane-note").absolute().left((note.at.x - px(320.0)).max(px(8.0))).top(note.at.y + px(10.0)).w(px(320.0)).flex().flex_col().gap_2().rounded_lg().bg(c.raised).p_3().shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div().flex().items_center().justify_between()
                                .child(div().text_xs().text_color(c.fg).child("Pane note"))
                                .child(
                                    div().id("pane-note-x").rounded_sm().p_1().cursor_pointer().text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_pane_note(cx);
                                        }))
                                        .child(Icon::new(IconName::X).size(px(14.0))),
                                ),
                        )
                        .child(div().rounded_md().bg(c.panel).p_1p5().text_xs().child(Textarea::new(&note.input).appearance(false)))
                        .child(div().text_size(px(11.0)).text_color(c.subtle).child("Only for you. Not shared with the agent. Saves automatically.")),
                )
                .into_any_element(),
        )
    }
}
