// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Pure UI transitions. Effects are performed only by the runtime.
use crate::cli::Call;
use hblctl_core::{
    codec,
    registry::{FuncSpec, Registry, input_schema_with_tables},
    safety::{Policy, Safety},
};
use serde_json::{Map, Value};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Operations,
    Form,
}

pub(crate) struct FormField {
    pub(crate) name: String,
    pub(crate) schema: Value,
    pub(crate) required: bool,
    pub(crate) value: String,
}

pub(crate) struct Operation {
    pub(crate) command: String,
    pub(crate) name: String,
    pub(crate) spec: FuncSpec,
}

pub(crate) enum Event {
    Up,
    Down,
    Tab,
    Text(char),
    Backspace,
    Escape,
    Run,
    Reconnect,
    Completed(Result<Value, String>),
    ShellOutput(String),
    Connection(String),
    Log(String),
    Scroll(i16),
}

pub(crate) enum Effect {
    Invoke(Call),
    Reconnect,
}

pub(crate) struct App {
    pub(crate) registry: Registry,
    pub(crate) policy: Policy,
    pub(crate) operations: Vec<Operation>,
    pub(crate) selected: usize,
    pub(crate) filter: String,
    pub(crate) fields: Vec<FormField>,
    pub(crate) field: usize,
    pub(crate) focus: Focus,
    pub(crate) confirmation: Option<String>,
    pub(crate) result: String,
    pub(crate) connection: String,
    pub(crate) busy: bool,
    pub(crate) logs: std::collections::VecDeque<String>,
    pub(crate) scroll: u16,
}

impl App {
    pub(crate) fn new(registry: Registry, policy: Policy) -> Self {
        let operations = registry
            .operations()
            .map(|(c, f)| Operation {
                command: c.name.clone(),
                name: format!("{}.{}", c.name, f.name),
                spec: f.clone(),
            })
            .collect();
        let mut a = Self {
            registry,
            policy,
            operations,
            selected: 0,
            filter: String::new(),
            fields: vec![],
            field: 0,
            focus: Focus::Operations,
            confirmation: None,
            result: String::new(),
            connection: "disconnected (lazy connection)".into(),
            busy: false,
            logs: std::collections::VecDeque::default(),
            scroll: 0,
        };
        a.form();
        a
    }

    pub(crate) fn visible(&self) -> Vec<&Operation> {
        let filter = self.filter.to_lowercase();
        self.operations
            .iter()
            .filter(|op| op.name.to_lowercase().contains(&filter))
            .collect()
    }

    pub(crate) fn operation(&self) -> Option<&Operation> {
        self.visible().get(self.selected).copied()
    }

    fn form(&mut self) {
        self.field = 0;
        self.fields = self.operation().map_or_else(Vec::new, |op| {
            let schema = input_schema_with_tables(&self.registry, &op.spec);
            op.spec
                .request
                .iter()
                .map(|f| {
                    let prop = schema["properties"][&f.name].clone();
                    let value = prop.get("default").map_or_else(String::new, |v| {
                        v.as_str().map_or_else(|| v.to_string(), str::to_owned)
                    });
                    FormField {
                        name: f.name.clone(),
                        required: schema["required"]
                            .as_array()
                            .is_some_and(|a| a.contains(&Value::String(f.name.clone()))),
                        schema: prop,
                        value,
                    }
                })
                .collect()
        });
    }

    pub(crate) fn update(&mut self, event: Event) -> Option<Effect> {
        match event {
            Event::Completed(result) => {
                self.busy = false;
                self.scroll = 0;
                self.result = match result {
                    Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
                    Err(e) => format!("Error: {e}"),
                };
            }
            Event::ShellOutput(s) => self.result = s,
            Event::Connection(s) => self.connection = s,
            Event::Log(s) => {
                self.logs.push_back(s);
                if self.logs.len() > 500 {
                    self.logs.pop_front();
                }
            }
            Event::Scroll(n) => self.scroll = self.scroll.saturating_add_signed(n),
            Event::Escape => {
                if self.confirmation.take().is_none() {
                    self.filter.clear();
                    self.selected = 0;
                    self.form();
                }
            }
            Event::Reconnect if !self.busy && self.confirmation.is_none() => {
                self.busy = true;
                self.connection = "connecting".into();
                return Some(Effect::Reconnect);
            }
            Event::Run if !self.busy => return self.prepare(),
            Event::Text(c) => {
                if let Some(s) = &mut self.confirmation {
                    s.push(c);
                } else if self.focus == Focus::Form {
                    if let Some(f) = self.fields.get_mut(self.field) {
                        f.value.push(c);
                    }
                } else {
                    self.filter.push(c);
                    self.selected = 0;
                    self.form();
                }
            }
            Event::Backspace => {
                if let Some(s) = &mut self.confirmation {
                    s.pop();
                } else if self.focus == Focus::Form {
                    if let Some(f) = self.fields.get_mut(self.field) {
                        f.value.pop();
                    }
                } else {
                    self.filter.pop();
                    self.selected = 0;
                    self.form();
                }
            }
            Event::Tab if self.confirmation.is_none() => {
                self.focus = if self.focus == Focus::Operations {
                    Focus::Form
                } else {
                    Focus::Operations
                }
            }
            Event::Up | Event::Down if self.confirmation.is_none() => {
                let down = matches!(event, Event::Down);
                if self.focus == Focus::Form {
                    self.field = if down {
                        (self.field + 1).min(self.fields.len().saturating_sub(1))
                    } else {
                        self.field.saturating_sub(1)
                    };
                } else {
                    self.selected = if down {
                        (self.selected + 1).min(self.visible().len().saturating_sub(1))
                    } else {
                        self.selected.saturating_sub(1)
                    };
                    self.form();
                }
            }
            _ => {}
        }
        None
    }

    fn prepare(&mut self) -> Option<Effect> {
        let op = self.operation()?;
        if let Err(e) = self.policy.check(&op.name, op.spec.safety) {
            self.result = e.to_string();
            return None;
        }
        let args: Map<String, Value> = self
            .fields
            .iter()
            .filter(|f| !f.value.is_empty() || (!f.required && f.schema["type"] == "string"))
            .map(|f| (f.name.clone(), Value::String(f.value.clone())))
            .collect();
        if let Err(e) = codec::encode_with_tables(&self.registry, &op.spec, &args) {
            self.result = e.to_string();
            return None;
        }
        if op.spec.safety == Safety::Destructive && self.confirmation.as_deref() != Some(op.name.as_str()) {
            if self.confirmation.is_none() {
                self.confirmation = Some(String::new());
            } else {
                self.result = format!("Type {} exactly to confirm", op.name);
            }
            return None;
        }
        let call = Call {
            command: op.command.clone(),
            func: op.spec.name.clone(),
            args,
        };
        self.confirmation = None;
        self.busy = true;
        self.result = "Running…".into();
        Some(Effect::Invoke(call))
    }
}
