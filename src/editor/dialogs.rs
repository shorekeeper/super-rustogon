//! Modal dialog system for the editor.
//!
//! One dialog at a time. When a dialog is active it blocks
//! every other input path in the editor: the graph canvas, the
//! inspector, the timeline, and the toolbar all check
//! `Editor::dialog.is_some()` before handling their own mouse
//! and keyboard events. This keeps focus semantics simple and
//! avoids reentrancy headaches.
//!
//! The dialog itself is purely a data structure. Rendering
//! goes through `draw`, input through `update`. Confirming or
//! cancelling produces a `DialogOutcome`, which the editor
//! shell dispatches back to whatever code originally opened
//! the dialog.

use crate::pipeline::Vertex;
use crate::text::{push_text_centered, text_height, text_width};
use crate::text_small::{push_small, push_small_centered};
use crate::ui::draw::{push_outline, push_quad, push_quad_alpha};
use crate::win32::{Input, Mouse};

/// What the dialog was about. The editor shell matches on
/// this to decide what to do next when the dialog closes.
#[derive(Clone, Debug)]
pub enum DialogKind {
    /// Author pressed Back while there are unsaved changes.
    /// Choosing "Save" writes the file and then exits,
    /// "Discard" exits without saving, "Cancel" keeps
    /// editing.
    UnsavedChangesOnBack,
    /// Author pressed PrePlay with unsaved changes. The
    /// save action is highly recommended but not required;
    /// a play test with the unsaved state still works.
    UnsavedChangesOnPreplay,
    /// Generic message the user acknowledges with Enter or
    /// the OK button. Used for save confirmations, parse
    /// errors during import, and fatal save failures.
    Info { title: String, body: String },
    /// Confirmation with custom title / body / action label.
    /// Used for destructive ops like "Delete section".
    Confirm { title: String, body: String, confirm_label: String },
}

/// Outcome of the dialog on close. `Stay` keeps the dialog
/// open. Everything else closes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    Stay,
    /// Author chose the primary action (Save, OK, Confirm).
    Primary,
    /// Author chose the secondary action (Discard).
    Secondary,
    /// Author cancelled (Cancel button, ESC).
    Cancel,
}

/// Mutable state for one dialog instance.
pub struct Dialog {
    pub kind: DialogKind,
    /// Cached hover intensity per button (0..=2). Animated
    /// toward the target every frame.
    hover: [f32; 3],
    hover_target: [bool; 3],
    escape_was_down: bool,
    enter_was_down: bool,
    left_was_down: bool,
}

impl Dialog {
    pub fn new(kind: DialogKind) -> Self {
        Dialog {
            kind,
            hover: [0.0; 3],
            hover_target: [false; 3],
            escape_was_down: false,
            enter_was_down: false,
            // See the matching note in `Picker::new`: we start
            // with the mouse considered already down so the
            // opening click does not get re-interpreted as a
            // button press on the dialog itself.
            left_was_down: true,
        }
    }

    /// Update hover state and check for mouse or keyboard
    /// activation. Returns the outcome for this frame;
    /// `Stay` means the dialog is still active.
    pub fn update(
        &mut self, dt: f32,
        pointer: (f32, f32), mouse: Mouse, input: Input,
    ) -> DialogOutcome {
        let esc_edge = input.escape && !self.escape_was_down;
        let enter_edge = input.enter && !self.enter_was_down;
        self.escape_was_down = input.escape;
        self.enter_was_down = input.enter;
        let clicked = mouse.left_down && !self.left_was_down;
        self.left_was_down = mouse.left_down;

        if esc_edge { return DialogOutcome::Cancel; }

        let rects = self.button_rects();
        let count = rects.len();
        for i in 0..count {
            let r = rects[i];
            let h = self.rect_hit(r, pointer);
            self.hover_target[i] = h;
            if h && clicked {
                return match i {
                    0 => DialogOutcome::Primary,
                    1 => match &self.kind {
                        DialogKind::UnsavedChangesOnBack
                        | DialogKind::UnsavedChangesOnPreplay =>
                            DialogOutcome::Secondary,
                        _ => DialogOutcome::Cancel,
                    },
                    _ => DialogOutcome::Cancel,
                };
            }
        }
        if enter_edge { return DialogOutcome::Primary; }

        // Tween hover eases.
        let blend = 1.0 - (-14.0_f32 * dt).exp();
        for i in 0..3 {
            let target = if self.hover_target[i] { 1.0 } else { 0.0 };
            self.hover[i] += (target - self.hover[i]) * blend;
        }

        DialogOutcome::Stay
    }

    /// Emit triangles for the dialog backdrop, the panel, the
    /// title, the body text, and the buttons.
    pub fn draw(&self, out: &mut Vec<Vertex>,
                view_left: f32, view_right: f32,
                view_top: f32, view_bottom: f32)
    {
        // Full screen dimming backdrop. Blocks the editor
        // visually and semantically.
        push_quad_alpha(out,
            view_left, view_top, view_right, view_bottom,
            [0.0, 0.0, 0.0], 0.70);

        let panel = self.panel_rect();
        push_quad(out, panel.0, panel.1, panel.2, panel.3,
            [0.10, 0.04, 0.15]);
        push_outline(out, panel.0, panel.1, panel.2, panel.3,
            0.004, [1.00, 0.45, 0.75]);

        let (title, body) = self.text();
        let cx = (panel.0 + panel.2) * 0.5;
        push_text_centered(out, &title, cx,
            panel.1 + 0.06, 0.010, [1.00, 0.80, 0.92]);

        // Word wrap body line by line using the small font so
        // we can fit longer explanations without a huge panel.
        let body_y = panel.1 + 0.14;
        for (i, line) in body.lines().enumerate() {
            let y = body_y + i as f32 * 0.028;
            push_small_centered(out, line, cx, y, 0.0045,
                [1.00, 1.00, 1.00]);
        }

        let rects = self.button_rects();
        let labels = self.button_labels();
        for (i, r) in rects.iter().enumerate() {
            let h = self.hover[i].clamp(0.0, 1.0);
            let bg = mix3([0.10, 0.04, 0.15], [0.22, 0.10, 0.30], h);
            let ring = if i == 0 {
                mix3([1.00, 0.45, 0.75], [1.00, 0.80, 0.92], h)
            } else {
                mix3([0.55, 0.42, 0.52], [1.00, 1.00, 1.00], h)
            };
            push_quad(out, r.0, r.1, r.2, r.3, bg);
            push_outline(out, r.0, r.1, r.2, r.3,
                0.003 + 0.001 * h, ring);
            let bcx = (r.0 + r.2) * 0.5;
            let bcy = (r.1 + r.3) * 0.5;
            let label = labels[i];
            let w = text_width(label, 0.0050);
            use crate::text::push_text;
            push_text(out, label,
                bcx - w * 0.5,
                bcy - text_height(0.0050) * 0.5,
                0.0050, [1.00, 1.00, 1.00]);
        }
    }

    fn panel_rect(&self) -> (f32, f32, f32, f32) {
        (-0.80, -0.28, 0.80, 0.28)
    }

    fn text(&self) -> (String, String) {
        match &self.kind {
            DialogKind::UnsavedChangesOnBack => (
                "UNSAVED CHANGES".into(),
                "YOU HAVE UNSAVED CHANGES IN THIS LEVEL.\n\
                 SAVE BEFORE LEAVING THE EDITOR?".into(),
            ),
            DialogKind::UnsavedChangesOnPreplay => (
                "UNSAVED CHANGES".into(),
                "THE PRE-PLAY WILL USE YOUR CURRENT UNSAVED\n\
                 STATE. SAVE FIRST OR CONTINUE WITHOUT\n\
                 SAVING?".into(),
            ),
            DialogKind::Info { title, body } =>
                (title.clone(), body.clone()),
            DialogKind::Confirm { title, body, .. } =>
                (title.clone(), body.clone()),
        }
    }

    fn button_labels(&self) -> Vec<&'static str> {
        match &self.kind {
            DialogKind::UnsavedChangesOnBack
            | DialogKind::UnsavedChangesOnPreplay =>
                vec!["SAVE AND CONTINUE", "DISCARD", "CANCEL"],
            DialogKind::Info { .. } => vec!["OK"],
            DialogKind::Confirm { confirm_label, .. } => {
                let s: &'static str = Box::leak(
                    confirm_label.clone().into_boxed_str());
                vec![s, "CANCEL"]
            }
        }
    }

    fn button_rects(&self) -> Vec<(f32, f32, f32, f32)> {
        let panel = self.panel_rect();
        let count = self.button_labels().len();
        let y0 = panel.3 - 0.08;
        let y1 = panel.3 - 0.02;
        let total = panel.2 - panel.0 - 0.10;
        let gap = 0.02;
        let w = (total - gap * (count as f32 - 1.0)) / count as f32;
        let mut rects = Vec::new();
        for i in 0..count {
            let x0 = panel.0 + 0.05 + (w + gap) * i as f32;
            let x1 = x0 + w;
            rects.push((x0, y0, x1, y1));
        }
        rects
    }

    fn rect_hit(&self, r: (f32, f32, f32, f32), p: (f32, f32)) -> bool {
        p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3
    }
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t,
     a[1] + (b[1] - a[1]) * t,
     a[2] + (b[2] - a[2]) * t]
}