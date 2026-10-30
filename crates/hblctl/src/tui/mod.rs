// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Registry-generated interactive frontend.
mod app;
mod run;
mod view;

pub(crate) use run::run;

#[cfg(test)]
mod tests;
