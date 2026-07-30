use dsync::config::{Config, MachineConfig};
use dsync::zen::export;

#[test]
fn test_zen_export() {
    // Don't set profile_path — it will auto-discover from ~/.config/zen/profiles.ini
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

    match export::export(&cfg) {
        Ok(data) => {
            let json: serde_json::Value = serde_json::from_slice(&data).unwrap();
            let profile_path = json.get("_source").and_then(|s| s.as_str()).unwrap();
            println!("Zen profile: {profile_path}");
            println!("Export size: {} bytes", data.len());
            println!("Keys: {:?}", json.as_object().map(|o| o.keys().collect::<Vec<_>>()));

            if let Some(spaces) = json.get("spaces").and_then(|v| v.as_array()) {
                println!("Spaces: {}", spaces.len());
            }
            if let Some(groups) = json.get("groups").and_then(|v| v.as_array()) {
                println!("Groups: {}", groups.len());
            }
            if let Some(pinned) = json.get("pinned_tabs").and_then(|v| v.as_array()) {
                println!("Pinned tabs: {}", pinned.len());
            }

            assert!(data.len() > 10, "export too small");
        }
        Err(e) => {
            eprintln!("Export error: {e}");
            panic!("Zen export failed: {e}");
        }
    }
}
