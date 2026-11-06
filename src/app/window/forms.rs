//! Form filling: field overlays bound to PDFium's form-fill engine.

use slint::{ModelRc, SharedString, VecModel};

use super::DocWindow;
use super::pages::EditKind;
use crate::pdf::engine::Priority;
use crate::pdf::forms::{self, Field, FieldKind};
use crate::ui::FormFieldView;

impl DocWindow {
    /// Fetches the document's form fields (after open and after each change).
    pub(crate) fn load_fields(&self) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let fields = forms::fields(doc);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| {
                    if w.st.borrow().doc.as_ref().is_some_and(|d| d.id == doc_id) {
                        w.st.borrow_mut().fields = fields;
                        w.refresh_fields();
                    }
                });
            });
        });
    }

    pub fn fields(&self) -> Vec<Field> {
        self.st.borrow().fields.clone()
    }

    /// Positions field overlays for the visible pages.
    pub(crate) fn refresh_fields(&self) {
        let pages: Vec<usize> = {
            use slint::Model;
            (0..self.pages_model.row_count())
                .filter_map(|r| self.pages_model.row_data(r).map(|p| p.index as usize))
                .collect()
        };
        let st = self.st.borrow();
        let views: Vec<FormFieldView> = st
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                pages.contains(&f.page) && f.page < st.layout.rects.len() && f.kind != FieldKind::PushButton
            })
            .map(|(i, f)| {
                let v = st.layout.rect_to_view(f.page, f.rect);
                let label = if !f.label.is_empty() { f.label.clone() } else { f.name.clone() };
                FormFieldView {
                    index: i as i32,
                    kind: match f.kind {
                        FieldKind::Text => "text",
                        FieldKind::CheckBox => "check",
                        FieldKind::Radio => "radio",
                        FieldKind::ComboBox | FieldKind::ListBox => "choice",
                        FieldKind::Signature => "signature",
                        FieldKind::PushButton => "button",
                    }
                    .into(),
                    x: v.x,
                    y: v.y,
                    width: v.w,
                    height: v.h,
                    value: f.value.as_str().into(),
                    options: ModelRc::new(VecModel::from(
                        f.options.iter().map(|o| SharedString::from(o.as_str())).collect::<Vec<_>>(),
                    )),
                    selected: f.selected.map(|s| s as i32).unwrap_or(-1),
                    checked: f.checked,
                    read_only: f.read_only,
                    multiline: f.multiline,
                    password: f.password,
                    label: label.into(),
                }
            })
            .collect();
        drop(st);
        self.forms_model.set_vec(views);
    }

    fn field(&self, index: i32) -> Option<Field> {
        self.st.borrow().fields.get(index.max(0) as usize).cloned()
    }

    pub fn field_commit_text(&self, index: i32, text: &str) {
        let Some(f) = self.field(index) else { return };
        if f.read_only || f.value == text {
            return;
        }
        let text = text.to_string();
        self.edit_document(EditKind::Form { page: f.page }, move |doc| {
            forms::set_text(doc, f.page, f.annot_index, &text)
                .then_some(())
                .ok_or_else(|| "The field rejected the value.".into())
        });
    }

    pub fn field_toggle(&self, index: i32) {
        let Some(f) = self.field(index) else { return };
        if f.read_only {
            return;
        }
        self.edit_document(EditKind::Form { page: f.page }, move |doc| {
            forms::click(doc, &f).then_some(()).ok_or_else(|| "The field could not be changed.".into())
        });
    }

    pub fn field_choose(&self, index: i32, option: i32) {
        let Some(f) = self.field(index) else { return };
        if f.read_only || option < 0 || f.selected == Some(option as usize) {
            return;
        }
        self.edit_document(EditKind::Form { page: f.page }, move |doc| {
            forms::select_option(doc, f.page, f.annot_index, option as usize)
                .then_some(())
                .ok_or_else(|| "The option could not be selected.".into())
        });
    }

    /// A signature field was clicked: sign it with the chosen signature.
    pub fn field_sign(&self, index: i32) {
        let Some(f) = self.field(index) else { return };
        self.sign_into(f.page, f.rect);
    }
}
