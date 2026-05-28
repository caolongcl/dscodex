//! `/polish` preview view.
//!
//! Displays the polished prompt the assistant produced and lets the user
//! either send it as a real user message (Enter) or bounce back to the
//! composer with the original `/polish <draft>` prefilled for revision (Esc).

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::key_hint;
use crate::render::renderable::Renderable;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::bottom_pane_view::ViewCompletion;

/// Invoked when the user confirms with Enter; the argument is the polished
/// prompt text to submit as a fresh user message.
pub(crate) type PolishSendCallback = Box<dyn FnOnce(String) + Send + Sync>;
/// Invoked when the user cancels with Esc; the argument is the original
/// draft so the composer can be restored to `/polish <draft>`.
pub(crate) type PolishReviseCallback = Box<dyn FnOnce(String) + Send + Sync>;

pub(crate) struct PolishPreviewView {
    polished: String,
    draft: String,
    on_send: Option<PolishSendCallback>,
    on_revise: Option<PolishReviseCallback>,
    completion: Option<ViewCompletion>,
}

impl PolishPreviewView {
    pub(crate) fn new(
        polished: String,
        draft: String,
        on_send: PolishSendCallback,
        on_revise: PolishReviseCallback,
    ) -> Self {
        Self {
            polished,
            draft,
            on_send: Some(on_send),
            on_revise: Some(on_revise),
            completion: None,
        }
    }

    fn body_lines(&self, width: u16) -> u16 {
        // Conservative wrap estimate; Paragraph::Wrap below does the real work.
        // We just need a reasonable height for the renderer.
        let usable = width.max(20) as usize - 2;
        if usable == 0 {
            return 1;
        }
        let mut total = 0u16;
        for raw_line in self.polished.lines() {
            let chars = raw_line.chars().count().max(1);
            let lines = chars.div_ceil(usable) as u16;
            total = total.saturating_add(lines);
        }
        if self.polished.is_empty() {
            return 1;
        }
        total.clamp(1, 15)
    }
}

impl BottomPaneView for PolishPreviewView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if let Some(send) = self.on_send.take() {
                    let polished = std::mem::take(&mut self.polished);
                    (send)(polished);
                }
                self.completion = Some(ViewCompletion::Accepted);
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.on_ctrl_c();
            }
            _ => {}
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        if let Some(revise) = self.on_revise.take() {
            let draft = std::mem::take(&mut self.draft);
            (revise)(draft);
        }
        self.completion = Some(ViewCompletion::Cancelled);
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.completion.is_some()
    }

    fn completion(&self) -> Option<ViewCompletion> {
        self.completion
    }
}

impl Renderable for PolishPreviewView {
    fn desired_height(&self, width: u16) -> u16 {
        // 1 title + body wrap + 1 footer + 1 padding.
        1u16.saturating_add(self.body_lines(width))
            .saturating_add(2)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        Clear.render(area, buf);

        let title = Line::from(vec![
            Span::from("✨ Polished prompt").bold(),
            Span::from(" (preview)").dim(),
        ]);
        let mut lines: Vec<Line<'static>> = vec![title];
        for raw_line in self.polished.lines() {
            lines.push(Line::from(raw_line.to_string()));
        }
        if self.polished.is_empty() {
            lines.push(Line::from("(no output)".dim()));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            "Press ".into(),
            key_hint::plain(KeyCode::Enter).into(),
            " to send · ".into(),
            key_hint::plain(KeyCode::Esc).into(),
            " to edit the draft".into(),
        ]));

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}
