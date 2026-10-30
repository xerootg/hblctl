// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Transaction and recovery regression tests.

use hblctl_core::safety::{Policy, Safety};
use hblctl_mods::{
    bundle::{Bundle, hash},
    camera::{File, MemoryFs},
    engine::{Engine, Status},
};
use serde_json::json;
use std::collections::BTreeMap;

fn from_manifest(
    manifest: serde_json::Value,
    mut members: BTreeMap<String, Vec<u8>>,
) -> hblctl_mods::Result<Bundle> {
    members.insert(
        "schema.json".into(),
        include_bytes!("../../../reference/schema.json").to_vec(),
    );
    Bundle::from_manifest(manifest, members)
}

fn fixture(before: &[u8], after: &[u8]) -> Bundle {
    let manifest = json!({"format":1,"version":"1.0.0","status":"ready","reason":"test","license":"test",
    "preconditions":{"models":["test"],"firmware":["1"],"profile_reviewed":true,"inventory_pending":false,"stock_hashes":[],"reject_symlinks":true,"system_mount":"test-ro"},
    "features":[{"id":"toy","requires":[],"conflicts":[],"files":["file"],"status":"ready"}],
    "files":[{"id":"file","upstream":"gothenburg","target":"/system/toy","before":hash(before),"mode":"0644","kind":"source","status":"ready","reason":"test","artifact":"artifacts/toy","output_sha256":hash(after)}],
    "recipes":[],"steps":[{"id":"write","phase":"install","order":0,"operation":"write-files","safety":"persistent","transport_floor":"read","on_failure":"retain-journal-and-stop"}],
    "backup":{"root":"/data/toolkit","original_files":[],"journal":[],"phases":[],"restore_policy":"exact-owned-bytes-or-verified-interrupted-prefix"},
    "verification":{"install":[],"restore":[]},"reboot":{"install":"required-after-commit","restore":"required-after-commit","upgrade":"restore-and-verify-before-install"}});
    from_manifest(
        manifest,
        BTreeMap::from([("artifacts/toy".into(), after.to_vec())]),
    )
    .unwrap()
}
fn camera(bytes: &[u8]) -> MemoryFs {
    let mut fs = MemoryFs::default();
    fs.files.insert(
        "/system/toy".into(),
        File {
            bytes: bytes.to_vec(),
            mode: 0o644,
        },
    );
    fs
}
fn policy() -> Policy {
    Policy::up_to(Safety::Persistent)
}

#[test]
fn apply_backups_journal_status_revert() {
    let b = fixture(b"stock", b"modified");
    let mut fs = camera(b"stock");
    let initial = fs.files.clone();
    let dir = tempfile::tempdir().unwrap();
    let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
    assert_eq!(e.status("toy").unwrap(), Status::Stock);
    let p = e.plan(&["toy".into()]).unwrap();
    e.apply(&p).unwrap();
    assert_eq!(e.status("toy").unwrap(), Status::Installed);
    assert!(dir.path().join("backup-0").exists());
    assert!(dir.path().join("journal.json").exists());
    e.revert(Some("toy")).unwrap();
    assert_eq!(fs.files, initial);
}
#[test]
fn refuses_preconditions_and_policy() {
    let b = fixture(b"stock", b"new");
    let dir = tempfile::tempdir().unwrap();
    let mut fs = camera(b"alien");
    assert!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .plan(&["toy".into()])
            .is_err()
    );
    let mut fs = camera(b"stock");
    assert!(
        Engine::new(&b, &mut fs, dir.path(), Policy::default())
            .plan(&["toy".into()])
            .is_err()
    );
    fs.model = "wrong".into();
    assert!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .plan(&["toy".into()])
            .is_err()
    );
}
#[test]
fn failures_and_power_loss_recover() {
    for power in [false, true] {
        for n in 1..10 {
            let b = fixture(b"stock", b"modified");
            let dir = tempfile::tempdir().unwrap();
            let mut fs = camera(b"stock");
            let initial = fs.files.clone();
            if power {
                fs.power_loss_on_write = Some(n);
            } else {
                fs.fail_on_write = Some(n);
            }
            {
                let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
                let p = e.plan(&["toy".into()]).unwrap();
                let _ = e.apply(&p);
            }
            fs.reconnect();
            let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
            if dir.path().join("journal.json").exists() {
                e.revert(None).unwrap();
            }
            assert_eq!(fs.files, initial, "power={power} write={n}");
        }
    }
}
#[test]
fn modified_state_is_unknown() {
    let b = fixture(b"stock", b"new");
    let dir = tempfile::tempdir().unwrap();
    let mut fs = camera(b"alien");
    assert_eq!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .status("toy")
            .unwrap(),
        Status::Unknown
    );
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(64))]
    #[test]
    fn apply_revert_exact(before in proptest::collection::vec(proptest::prelude::any::<u8>(),0..128), after in proptest::collection::vec(proptest::prelude::any::<u8>(),0..128)) {
        let b=fixture(&before,&after); let dir=tempfile::tempdir().unwrap(); let mut fs=camera(&before); let initial=fs.files.clone();
        let mut e=Engine::new(&b,&mut fs,dir.path(),policy()); let p=e.plan(&["toy".into()]).unwrap(); e.apply(&p).unwrap(); e.revert(None).unwrap();
        proptest::prop_assert_eq!(fs.files,initial);
    }
}
#[test]
fn preview_loads_but_cannot_execute() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/x2d-toolkit-bundle-0.1.0-preview.1.tar.zst");
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture_path = fixture_dir.path().join("preview.tar.zst");
    std::fs::copy(&path, &fixture_path).unwrap();
    std::fs::write(
        fixture_dir.path().join("preview.tar.zst.sha256"),
        include_str!("reference.sha256"),
    )
    .unwrap();
    let b = Bundle::load(&fixture_path).unwrap();
    assert_eq!(b.manifest()["status"], "blocked");
    let mut fs = MemoryFs::default();
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .plan(&["afc".into()])
            .is_err()
    );
}

#[test]
fn recipes_are_deterministic_and_reject_ambiguity() {
    use hblctl_mods::bundle::recipe;
    let r = json!({"id":"edit","input":"/system/input","algorithm":"byte-edits-v1","input_sha256":hash(b"abcde"),"output_sha256":hash(b"aXYcd!"),"edits":[{"offset":1,"before_hex":"62","after_hex":"5859"},{"offset":4,"before_hex":"65","after_hex":"21"}]});
    assert_eq!(recipe(&r, b"abcde").unwrap(), b"aXYcd!");
    assert_eq!(recipe(&r, b"abcde").unwrap(), recipe(&r, b"abcde").unwrap());
    assert!(recipe(&r, b"bad input").is_err());
    let mut bad = r.clone();
    bad["algorithm"] = json!("shell");
    assert!(recipe(&bad, b"abcde").is_err());
    let mut bad = r.clone();
    bad["edits"][1]["offset"] = json!(1);
    assert!(recipe(&bad, b"abcde").is_err());
    let mut bad = r;
    bad["edits"][0]["before_hex"] = json!("ff");
    assert!(recipe(&bad, b"abcde").is_err());
}
#[test]
fn semantic_validation_rejects_cycles_unknown_ids_and_traversal() {
    let b = fixture(b"stock", b"new");
    for case in 0..5 {
        let mut m = b.manifest().clone();
        match case {
            0 => m["features"][0]["requires"] = json!(["toy"]),
            1 => m["features"][0]["files"] = json!(["missing"]),
            2 => m["files"][0]["target"] = json!("/system/../data/escape"),
            3 => m["steps"][0]["order"] = json!(2),
            _ => m["unknown"] = json!(true),
        }
        assert!(from_manifest(m, BTreeMap::from([("artifacts/toy".into(), b"new".to_vec())])).is_err());
    }
}
#[test]
fn symlinks_stale_plans_and_unknown_changes_are_refused() {
    let b = fixture(b"stock", b"new");
    let dir = tempfile::tempdir().unwrap();
    let mut fs = camera(b"stock");
    fs.symlinks.insert("/system".into());
    assert!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .plan(&["toy".into()])
            .is_err()
    );
    fs.symlinks.clear();
    let p = Engine::new(&b, &mut fs, dir.path(), policy())
        .plan(&["toy".into()])
        .unwrap();
    fs.files.get_mut("/system/toy").unwrap().bytes = b"changed".to_vec();
    assert!(Engine::new(&b, &mut fs, dir.path(), policy()).apply(&p).is_err());
    fs.files.get_mut("/system/toy").unwrap().bytes = b"stock".to_vec();
    Engine::new(&b, &mut fs, dir.path(), policy()).apply(&p).unwrap();
    fs.files.get_mut("/system/toy").unwrap().bytes = b"foreign".to_vec();
    assert!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .revert(None)
            .is_err()
    );
    assert_eq!(fs.files["/system/toy"].bytes, b"foreign");
}
#[test]
fn absent_targets_are_removed_by_revert() {
    let b = fixture(b"stock", b"new");
    let mut m = b.manifest().clone();
    m["files"][0]["before"] = json!("ABSENT");
    let b = from_manifest(m, BTreeMap::from([("artifacts/toy".into(), b"new".to_vec())])).unwrap();
    let mut fs = MemoryFs::default();
    let dir = tempfile::tempdir().unwrap();
    let initial = fs.files.clone();
    let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
    let p = e.plan(&["toy".into()]).unwrap();
    e.apply(&p).unwrap();
    e.revert(None).unwrap();
    assert_eq!(fs.files, initial);
}

#[test]
fn firmware_conflicts_and_transport_floors_refuse_before_changes() {
    let b = fixture(b"stock", b"new");
    let dir = tempfile::tempdir().unwrap();
    let mut fs = camera(b"stock");
    fs.firmware = "wrong".into();
    assert!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .plan(&["toy".into()])
            .is_err()
    );
    for case in 0..3 {
        let mut manifest = b.manifest().clone();
        match case {
            0 => manifest["features"][0]["conflicts"] = json!(["toy"]),
            1 => manifest["steps"][0]["transport_floor"] = json!("destructive"),
            _ => manifest["files"][0]["target"] = json!("/data/toolkit/collision"),
        }
        let bundle = from_manifest(
            manifest,
            BTreeMap::from([("artifacts/toy".into(), b"new".to_vec())]),
        )
        .unwrap();
        let mut fs = camera(b"stock");
        let initial = fs.files.clone();
        assert!(
            Engine::new(&bundle, &mut fs, dir.path(), policy())
                .plan(&["toy".into()])
                .is_err()
        );
        assert_eq!(fs.files, initial);
    }
}
#[test]
fn interrupted_status_and_corrupt_backup_detection() {
    let b = fixture(b"stock", b"new");
    let dir = tempfile::tempdir().unwrap();
    let mut fs = camera(b"stock");
    fs.power_loss_on_write = Some(4);
    {
        let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
        let p = e.plan(&["toy".into()]).unwrap();
        assert!(e.apply(&p).is_err());
    }
    fs.reconnect();
    assert_eq!(
        Engine::new(&b, &mut fs, dir.path(), policy())
            .status("toy")
            .unwrap(),
        Status::Interrupted
    );
    Engine::new(&b, &mut fs, dir.path(), policy())
        .revert(None)
        .unwrap();
    let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
    let p = e.plan(&["toy".into()]).unwrap();
    e.apply(&p).unwrap();
    std::fs::write(dir.path().join("backup-0"), b"corrupted").unwrap();
    assert_eq!(e.status("toy").unwrap(), Status::Unknown);
    assert!(e.revert(None).is_err());
}
#[test]
fn missing_and_wrong_sidecars_are_rejected() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/x2d-toolkit-bundle-0.1.0-preview.1.tar.zst");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bundle.tar.zst");
    std::fs::copy(source, &path).unwrap();
    assert!(Bundle::load(&path).is_err());
    std::fs::write(dir.path().join("bundle.tar.zst.sha256"), "0".repeat(64)).unwrap();
    assert!(Bundle::load(&path).unwrap_err().to_string().contains("sidecar"));
}

#[test]
fn shared_files_remain_until_the_last_owner_is_reverted() {
    let b = fixture(b"stock", b"new");
    let mut m = b.manifest().clone();
    let mut other = m["features"][0].clone();
    other["id"] = json!("other");
    m["features"].as_array_mut().unwrap().push(other);
    let b = from_manifest(m, BTreeMap::from([("artifacts/toy".into(), b"new".to_vec())])).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut fs = camera(b"stock");
    let initial = fs.files.clone();
    let mut e = Engine::new(&b, &mut fs, dir.path(), policy());
    let p = e.plan(&["toy".into(), "other".into()]).unwrap();
    e.apply(&p).unwrap();
    e.revert(Some("toy")).unwrap();
    assert_eq!(e.status("other").unwrap(), Status::Installed);
    e.revert(Some("other")).unwrap();
    assert_eq!(fs.files, initial);
}
