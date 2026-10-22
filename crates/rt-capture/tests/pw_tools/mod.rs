//! What the Linux end-to-end tests need from PipeWire's command-line tools, the same from PipeWire 1.0 (Ubuntu 24.04,
//! which CI runs) on. Used by these tests and by rt-engine's `linux_engine`.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;

/// The arguments that make `pw-cat` play raw samples from its standard input (`-`): `--raw` where it has that option
/// (1.4 does), as it reads anything else through libsndfile, and none where it doesn't (1.0), as `-` is always raw
/// there and `--raw` an unknown option that ends it at once.
pub fn pw_cat_raw_input() -> &'static [&'static str] {
    static KNOWS_RAW: OnceLock<bool> = OnceLock::new();
    let knows_raw = *KNOWS_RAW.get_or_init(|| {
        let help = Command::new("pw-cat")
            .arg("--help")
            .output()
            .expect("run pw-cat (pipewire-bin)");
        [help.stdout, help.stderr]
            .iter()
            .any(|text| String::from_utf8_lossy(text).contains("--raw"))
    });
    if knows_raw { &["--raw"] } else { &[] }
}

/// The objects `pw-dump` shows now.
pub fn pw_dump() -> Vec<Value> {
    let output = Command::new("pw-dump").output().expect("run pw-dump (pipewire-bin)");
    parse_pw_dump(&output.stdout)
}

/// The objects in what `pw-dump` printed. That is one JSON array, except that PipeWire 1.0's also prints an array for
/// each object removed while it runs, `[{ "id": 45, "info": null }]`, before or after the full one. Later arrays update
/// the objects of the ones before, as with `pw-dump --monitor`. In order of id.
pub fn parse_pw_dump(output: &[u8]) -> Vec<Value> {
    let mut objects = BTreeMap::new();
    for array in serde_json::Deserializer::from_slice(output).into_iter::<Vec<Value>>() {
        for object in array.expect("pw-dump prints JSON arrays") {
            let Some(id) = object["id"].as_u64() else { continue };
            // A removed object has nothing but its id and a null `info` (or `props`).
            if object.get("type").is_some() {
                objects.insert(id, object);
            } else {
                objects.remove(&id);
            }
        }
    }
    objects.into_values().collect()
}
