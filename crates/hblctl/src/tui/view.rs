// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Rendering has no side effects and can use a `TestBackend`.
use std::fmt::Write as _;

use super::app::{App, Focus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(8),
        Constraint::Length(2),
    ])
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(format!(
            "{} | --allow {} | {}{}",
            app.registry.target,
            app.policy.max,
            app.connection,
            if app.busy { " | busy" } else { "" }
        )),
        rows[0],
    );
    let cols = Layout::horizontal([Constraint::Percentage(43), Constraint::Percentage(57)]).split(rows[1]);
    let right = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(cols[1]);
    operations(frame, app, cols[0]);
    form(frame, app, right[0]);
    frame.render_widget(
        Paragraph::new(app.result.as_str())
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0))
            .block(Block::bordered().title("Result | PgUp/PgDn scroll")),
        right[1],
    );
    let log_height = usize::from(rows[2].height.saturating_sub(2));
    let logs = app
        .logs
        .iter()
        .rev()
        .take(log_height)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|s| Line::from(s.as_str()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(logs).block(Block::bordered().title("Log | session tracing")),
        rows[2],
    );
    frame.render_widget(Paragraph::new("↑/↓ select  Tab list/form  type/backspace edit or filter  Enter run\nEsc cancel/clear  Ctrl-R reconnect  Ctrl-C quit"), rows[3]);
    if let Some(text) = &app.confirmation {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(90);
        let height = area.height.min(8);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let name = app.operation().map_or("", |op| op.name.as_str());
        frame.render_widget(
            Paragraph::new(format!(
                "Type the full operation name: {name}\n\n{text}\n\nEnter confirm | Esc cancel"
            ))
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title("Confirm destructive operation")
                    .border_style(Style::default().fg(Color::Red)),
            ),
            popup,
        );
    }
}

fn block(title: &str, active: bool) -> Block<'_> {
    Block::bordered()
        .title(title)
        .border_style(Style::default().fg(if active { Color::Cyan } else { Color::White }))
}

fn operations(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut last = "";
    let mut selected_row = 0;
    let mut items = vec![];
    for (index, op) in app.visible().into_iter().enumerate() {
        if op.command != last {
            items.push(ListItem::new(op.command.clone()).style(Style::default().fg(Color::Cyan)));
            last = &op.command;
        }
        if index == app.selected {
            selected_row = items.len();
        }
        let refused = app.policy.check(&op.name, op.spec.safety).err();
        let mut lines = vec![Line::from(format!("  {} [{}]", op.spec.name, op.spec.safety))];
        if !op.spec.verified {
            lines.push(Line::from("    unverified layout"));
        }
        if refused.is_some() {
            lines.push(Line::from(format!("    above --allow {}", app.policy.max)));
        }
        items.push(
            ListItem::new(lines).style(Style::default().fg(if op.spec.safety > app.policy.max {
                Color::DarkGray
            } else {
                Color::White
            })),
        );
    }
    let mut state = ListState::default().with_selected((!items.is_empty()).then_some(selected_row));
    frame.render_stateful_widget(
        List::new(items)
            .block(block(
                &format!("Operations | filter: {}", app.filter),
                app.focus == Focus::Operations,
            ))
            .highlight_symbol("> ")
            .highlight_style(Style::default().bg(Color::Blue)),
        area,
        &mut state,
    );
}

fn form(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let fields = app
        .fields
        .iter()
        .map(|f| {
            let mut lines = vec![Line::from(format!(
                "{}{} = {}",
                f.name,
                if f.required { " *required" } else { "" },
                f.value
            ))];
            let mut kind = f.schema["type"].as_str().unwrap_or("").to_owned();
            if let Some(min) = f.schema.get("minimum") {
                let _ = write!(
                    kind,
                    " {min}..={}",
                    f.schema
                        .get("maximum")
                        .map_or_else(String::new, ToString::to_string)
                );
            }
            if let Some(max) = f.schema.get("maxLength") {
                let _ = write!(kind, " max length {max}");
            }
            if f.schema.get("pattern").is_some() {
                kind.push_str(" (hex pairs)");
            }
            lines.push(Line::from(kind));
            if let Some(default) = f.schema.get("default") {
                lines.push(Line::from(format!("default: {default}")));
            }
            if let Some(doc) = f.schema["description"].as_str() {
                lines.push(Line::from(doc.to_owned()));
            }
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected((!fields.is_empty()).then_some(app.field));
    frame.render_stateful_widget(
        List::new(fields)
            .block(block(
                "Form | integers: decimal or 0x-hex",
                app.focus == Focus::Form,
            ))
            .highlight_symbol("> "),
        area,
        &mut state,
    );
}
