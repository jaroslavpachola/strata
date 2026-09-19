//! One item as a form: a field per property, choices and booleans as
//! drop-downs, and Save writing back only what changed.

use egui::{ComboBox, Grid, TextEdit, Ui};
use strata_core::{Api, Item, Kind, PropertyDef, Uuid, Values, value_text};

/// What became of the form this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum FormOutcome {
    Editing,
    Saved(Item),
    Cancelled,
}

pub struct FormView {
    /// `None` for a new item.
    pub id: Option<Uuid>,
    pub type_name: String,
    fields: Vec<Field>,
    error: Option<String>,
}

struct Field {
    prop: PropertyDef,
    input: String,
    original: String,
}

impl FormView {
    /// A form over an existing item.
    pub fn edit(store: &dyn Api, id: Uuid) -> strata_core::Result<Self> {
        let item = store.get_item(id)?;
        let mut form = Self::new(store, &item.type_name)?;
        form.id = Some(id);
        for field in &mut form.fields {
            if let Some(v) = item.values.get(&field.prop.name) {
                field.input = value_text(v);
                field.original = field.input.clone();
            }
        }
        Ok(form)
    }

    /// An empty form for a new item of `type_name`.
    pub fn new(store: &dyn Api, type_name: &str) -> strata_core::Result<Self> {
        let def = store.get_type(type_name)?;
        Ok(Self {
            id: None,
            type_name: def.name,
            fields: def
                .properties
                .into_iter()
                .map(|prop| Field {
                    prop,
                    input: String::new(),
                    original: String::new(),
                })
                .collect(),
            error: None,
        })
    }

    pub fn show(&mut self, ui: &mut Ui, store: &dyn Api, author: &str) -> FormOutcome {
        let title = match self.id {
            Some(_) => format!("{}: edit", self.type_name),
            None => format!("{}: new", self.type_name),
        };
        ui.heading(title);
        Grid::new(("strata-form", &self.type_name, self.id))
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for field in &mut self.fields {
                    let mut name = field.prop.name.clone();
                    if field.prop.required {
                        name.push_str(" *");
                    }
                    let label = ui.label(name);
                    field_widget(ui, field).labelled_by(label.id);
                    ui.end_row();
                }
            });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let mut outcome = FormOutcome::Editing;
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                match self.save(store, author) {
                    Ok(item) => outcome = FormOutcome::Saved(item),
                    Err(e) => self.error = Some(e),
                }
            }
            if ui.button("Cancel").clicked() {
                outcome = FormOutcome::Cancelled;
            }
        });
        outcome
    }

    /// Every field for a new item, the changed ones for an existing one.
    fn save(&self, store: &dyn Api, author: &str) -> Result<Item, String> {
        let mut values = Values::new();
        for f in &self.fields {
            let keep = match self.id {
                None => !f.input.trim().is_empty(),
                Some(_) => f.input != f.original,
            };
            if keep {
                let value = f
                    .prop
                    .kind
                    .parse_text(&f.input)
                    .map_err(|e| format!("{}: {e}", f.prop.name))?;
                values.insert(f.prop.name.clone(), value);
            }
        }
        match self.id {
            Some(id) => store.update_item(id, values, author),
            None => store.add_item(&self.type_name, values, author),
        }
        .map_err(|e| e.to_string())
    }
}

fn field_widget(ui: &mut Ui, field: &mut Field) -> egui::Response {
    let options: Option<Vec<String>> = match (&field.prop.choices, field.prop.kind) {
        (Some(c), _) => Some(c.clone()),
        (None, Kind::Bool) => Some(vec!["true".into(), "false".into()]),
        _ => None,
    };
    match options {
        Some(mut options) => {
            if !field.prop.required {
                options.insert(0, String::new());
            }
            ComboBox::from_id_salt(("strata-field", &field.prop.name))
                .selected_text(field.input.clone())
                .show_ui(ui, |ui| {
                    for o in options {
                        let label = if o.is_empty() {
                            "(none)".to_string()
                        } else {
                            o.clone()
                        };
                        ui.selectable_value(&mut field.input, o, label);
                    }
                })
                .response
        }
        None if field.prop.kind == Kind::Json => ui.add(
            TextEdit::multiline(&mut field.input)
                .code_editor()
                .desired_rows(3)
                .desired_width(320.0),
        ),
        None => ui.add(TextEdit::singleline(&mut field.input).desired_width(320.0)),
    }
}
