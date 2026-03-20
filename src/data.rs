use directories::ProjectDirs;
use std::io;
use std::path::Path;

const ANIME_DATA_URL: &str =
    "https://raw.githubusercontent.com/frivoxfr/ani-data/refs/heads/main/anime_data.json";

fn download_to_vec() -> Option<Vec<u8>> {
    reqwest::blocking::get(ANIME_DATA_URL)
        .ok()
        .and_then(|r| r.bytes().ok())
        .map(|b| b.to_vec())
}

fn merge_data(local_path: &Path, remote_bytes: &[u8]) {
    let remote: serde_json::Value = match serde_json::from_slice(remote_bytes) {
        Ok(v) => v,
        Err(_) => return,
    };

    let local: serde_json::Value = if local_path.exists() {
        let data = std::fs::read(local_path).unwrap_or_default();
        serde_json::from_slice(&data).unwrap_or(serde_json::json!({"media": []}))
    } else {
        serde_json::json!({"media": []})
    };

    let remote_media = remote.get("media").and_then(|m| m.as_array());
    let local_media = local.get("media").and_then(|m| m.as_array());

    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();

    if let Some(local_entries) = local_media {
        for entry in local_entries {
            let key = (
                entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase(),
                entry.get("lang").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                entry.get("season").and_then(|v| v.as_i64()).unwrap_or(1),
            );
            if seen.insert(key) {
                merged.push(entry.clone());
            }
        }
    }

    if let Some(remote_entries) = remote_media {
        for entry in remote_entries {
            let key = (
                entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase(),
                entry.get("lang").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                entry.get("season").and_then(|v| v.as_i64()).unwrap_or(1),
            );
            if seen.insert(key) {
                merged.push(entry.clone());
            }
        }
    }

    let result = serde_json::json!({"media": merged});
    let out = std::fs::File::create(local_path).expect("Failed to create file");
    serde_json::to_writer(out, &result).expect("Failed to write merged data");
}

pub fn get_file(overwrite: bool) {
    let dir = ProjectDirs::from("", "B0SE", "ani-fr").expect("Failed to get project directory");
    let data_dir = dir.data_dir();
    let file_path = data_dir.join("anime_data.json");

    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).expect("Failed to create data directory");
    }

    if !file_path.exists() || overwrite {
        if let Some(bytes) = download_to_vec() {
            let mut out = std::fs::File::create(&file_path).expect("Failed to create file");
            io::copy(&mut bytes.as_slice(), &mut out).expect("Failed to write to file");
        }
    } else if let Some(remote_bytes) = download_to_vec() {
        merge_data(&file_path, &remote_bytes);
    }
}
