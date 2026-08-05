use std::path::PathBuf;

use dsync::config::{Config, MachineConfig, ZenConfig};
use dsync::zen::{export, import};

#[test]
fn test_zen_roundtrip() {
    let cfg = Config {
        machine: MachineConfig {
            name: "test".into(),
        },
        hub: None,
        hub_connect: None,
        zen: None,
        projects: None,
        remote: None,
    };

    // Step 1: export
    let data = export::export(&cfg).expect("export should succeed");
    assert!(data.len() > 100, "export data too small");

    let json: serde_json::Value = serde_json::from_slice(&data).unwrap();
    let profile_path = json.get("_source").and_then(|s| s.as_str()).unwrap();
    println!("Exporting from: {profile_path}");

    // Step 2: save to temp file
    let tmp = std::env::temp_dir().join("dsync-zen-test.json");
    std::fs::write(&tmp, &data).unwrap();
    println!("Export saved to: {}", tmp.display());

    // Step 3: re-import from the saved file
    import::import(&cfg, &data).expect("import should succeed");
    println!("Import successful");

    // Step 4: re-export and verify it's still valid
    let data2 = export::export(&cfg).expect("second export should succeed");
    let json2: serde_json::Value = serde_json::from_slice(&data2).unwrap();

    let orig_spaces = json.get("spaces").and_then(|v| v.as_array()).map(|a| a.len());
    let new_spaces = json2.get("spaces").and_then(|v| v.as_array()).map(|a| a.len());
    assert_eq!(orig_spaces, new_spaces, "spaces count should match");

    let orig_groups = json.get("groups").and_then(|v| v.as_array()).map(|a| a.len());
    let new_groups = json2.get("groups").and_then(|v| v.as_array()).map(|a| a.len());
    assert_eq!(orig_groups, new_groups, "groups count should match");

    let pinned1 = json.get("pinned_tabs").and_then(|v| v.as_array()).map(|a| a.len());
    let pinned2 = json2.get("pinned_tabs").and_then(|v| v.as_array()).map(|a| a.len());
    assert_eq!(pinned1, pinned2, "pinned tabs count should match");

    // Cleanup
    let _ = std::fs::remove_file(&tmp);

    println!("✓ Round-trip test passed");
}

#[test]
fn test_zen_import_specific_profile() {
    // Test with explicitly specified profile
    let profile_path = PathBuf::from(
        std::env::var("HOME").unwrap() + "/.config/zen/oxblbp7e.Default (release)",
    );

    if !profile_path.exists() {
        eprintln!("Skipping: profile not found at {}", profile_path.display());
        return;
    }

    let cfg = Config {
        machine: MachineConfig {
            name: "test".into(),
        },
        hub: None,
        hub_connect: None,
        zen: Some(ZenConfig { profile_path }),
        projects: None,
        remote: None,
    };

    // Export
    let data = export::export(&cfg).expect("export should succeed");
    println!("Export size from explicit profile: {} bytes", data.len());

    // Re-import
    import::import(&cfg, &data).expect("import should succeed");
    println!("✓ Import test passed");
}
