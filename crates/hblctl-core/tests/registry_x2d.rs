// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Guards on the shipped X2D 4.2.0 registry: it must load, and dangerous operations must keep their class.

use hblctl_core::registry::Registry;
use hblctl_core::safety::Safety;

const X2D: &str = include_str!("../registry/x2d-4.2.0.json");

#[test]
fn loads_and_validates() {
    let r = Registry::from_json(X2D).expect("registry is valid");
    assert!(r.target.contains("X2D"));
    assert!(r.operations().count() >= 40);
}

#[test]
fn dangerous_operations_stay_destructive() {
    let r = Registry::from_json(X2D).unwrap();
    for (cmd, func) in [
        ("hbl-shell", "start-shell"),
        ("os-system-command", "run-path"),
        ("run-script", "run-staged-script"),
        ("set-camera-type", "set-type"),
        ("erase-config-store", "erase-config"),
        ("fw-upgrade-media-slot", "upgrade-media"),
        ("prod-config", "set"),
        ("prod-config", "initialize"),
        ("request-calib-head", "remove-temporary-calibration"),
        ("verify-calibs", "verify-selected"),
        ("data-transfer", "host-transfer"),
    ] {
        let f = r
            .command(cmd)
            .unwrap_or_else(|| panic!("missing {cmd}"))
            .func(func)
            .unwrap_or_else(|| panic!("missing {cmd}.{func}"));
        assert_eq!(f.safety, Safety::Destructive, "{cmd}.{func}");
    }
}

#[test]
fn reads_are_reads() {
    let r = Registry::from_json(X2D).unwrap();
    for (cmd, func) in [
        ("prod-config", "get"),
        ("prod-config", "print"),
        ("firmware-version", "build-version"),
        ("rtc", "get-time"),
    ] {
        assert_eq!(
            r.command(cmd).unwrap().func(func).unwrap().safety,
            Safety::Read,
            "{cmd}.{func}"
        );
    }
}
