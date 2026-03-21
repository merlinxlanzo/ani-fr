use chrono::Utc;
use colored::Colorize;
use directories::ProjectDirs;
use inquire::Text;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default)]
pub struct MalConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct MalCache {
    pub mappings: HashMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub name: String,
    pub lang: String,
    pub season: i8,
    pub episode: usize,
    pub timestamp: f64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct WatchHistory {
    pub entries: Vec<HistoryEntry>,
}

fn data_dir() -> PathBuf {
    ProjectDirs::from("", "B0SE", "ani-fr")
        .expect("Failed to get project directory")
        .data_dir()
        .to_path_buf()
}

fn config_path() -> PathBuf {
    data_dir().join("mal_config.json")
}

fn cache_path() -> PathBuf {
    data_dir().join("mal_cache.json")
}

pub fn load_config() -> Option<MalConfig> {
    let path = config_path();
    if !path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_config(config: &MalConfig) {
    let path = config_path();
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = std::fs::write(path, json);
    }
}

pub fn load_cache() -> MalCache {
    let path = cache_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        MalCache::default()
    }
}

fn save_cache(cache: &MalCache) {
    let path = cache_path();
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(path, json);
    }
}

pub fn is_logged_in() -> bool {
    load_config()
        .map(|c| !c.access_token.is_empty())
        .unwrap_or(false)
}

pub fn logout() {
    let path = config_path();
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    println!("{}", "Déconnecté de MAL.".green());
}

fn generate_code_verifier() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..128)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn parse_json(resp: reqwest::blocking::Response) -> Option<serde_json::Value> {
    let text = resp.text().ok()?;
    serde_json::from_str(&text).ok()
}

fn post_form(url: &str, params: &[(&str, &str)]) -> Option<serde_json::Value> {
    let client = reqwest::blocking::Client::new();
    let body = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoded(v)))
        .collect::<Vec<_>>()
        .join("&");

    let resp = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .ok()?;
    parse_json(resp)
}

fn urlencoded(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

pub fn login() {
    if let Some(config) = load_config() {
        if !config.access_token.is_empty() {
            println!("{}", "Déjà connecté à MAL.".green());
            return;
        }
    }

    let client_id =
        match Text::new("Entrez votre Client ID MAL (https://myanimelist.net/apiconfig) :")
            .prompt()
        {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => {
                println!("{}", "Client ID invalide.".red());
                return;
            }
        };

    let client_secret = match Text::new("Entrez votre Client Secret MAL :").prompt() {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            println!("{}", "Client Secret invalide.".red());
            return;
        }
    };

    let code_verifier = generate_code_verifier();
    let redirect_uri = "http://localhost:7878/callback";

    let auth_url = format!(
        "https://myanimelist.net/v1/oauth2/authorize?response_type=code&client_id={}&code_challenge={}&code_challenge_method=plain&redirect_uri={}",
        client_id, code_verifier, urlencoded(redirect_uri)
    );

    println!("Ouverture du navigateur pour l'authentification MAL...");
    if open::that(&auth_url).is_err() {
        println!("Impossible d'ouvrir le navigateur. Ouvrez ce lien manuellement :");
        println!("{}", auth_url);
    }

    println!("En attente de la réponse... (Ctrl+C pour annuler)");

    let listener = match TcpListener::bind("127.0.0.1:7878") {
        Ok(l) => l,
        Err(e) => {
            println!("{} {}", "Erreur serveur local:".red(), e);
            return;
        }
    };

    let code = match listener.accept() {
        Ok((mut stream, _)) => {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Connexion réussie ! Vous pouvez fermer cette page.</h2></body></html>";
            let _ = stream.write_all(response.as_bytes());

            extract_code_from_request(&request)
        }
        Err(e) => {
            println!("{} {}", "Erreur connexion:".red(), e);
            return;
        }
    };

    drop(listener);

    let code = match code {
        Some(c) => c,
        None => {
            println!("{}", "Code d'autorisation non reçu.".red());
            return;
        }
    };

    let json = post_form(
        "https://myanimelist.net/v1/oauth2/token",
        &[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code.as_str()),
            ("code_verifier", code_verifier.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ],
    );

    match json {
        Some(json) => {
            let access = json.get("access_token").and_then(|v| v.as_str());
            let refresh = json.get("refresh_token").and_then(|v| v.as_str());
            let expires = json.get("expires_in").and_then(|v| v.as_i64());

            if let (Some(access), Some(refresh), Some(expires)) = (access, refresh, expires) {
                let config = MalConfig {
                    client_id,
                    client_secret,
                    access_token: access.to_string(),
                    refresh_token: refresh.to_string(),
                    expires_at: Utc::now().timestamp() + expires,
                };
                save_config(&config);
                println!("{}", "Connecté à MAL avec succès !".green());
                return;
            }
            if let Some(err) = json.get("error").and_then(|v| v.as_str()) {
                println!("{} {}", "Erreur MAL:".red(), err);
                return;
            }
            println!("{}", "Réponse inattendue de MAL.".red());
        }
        None => println!("{}", "Erreur requête token.".red()),
    }
}

fn extract_code_from_request(request: &str) -> Option<String> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    for param in query.split('&') {
        let mut parts = param.splitn(2, '=');
        if parts.next()? == "code" {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}

pub fn ensure_token(config: &mut MalConfig) -> bool {
    if Utc::now().timestamp() < config.expires_at {
        return true;
    }

    let json = post_form(
        "https://myanimelist.net/v1/oauth2/token",
        &[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", config.refresh_token.as_str()),
        ],
    );

    if let Some(json) = json {
        let access = json.get("access_token").and_then(|v| v.as_str());
        let refresh = json.get("refresh_token").and_then(|v| v.as_str());
        let expires = json.get("expires_in").and_then(|v| v.as_i64());

        if let (Some(access), Some(refresh), Some(expires)) = (access, refresh, expires) {
            config.access_token = access.to_string();
            config.refresh_token = refresh.to_string();
            config.expires_at = Utc::now().timestamp() + expires;
            save_config(config);
            return true;
        }
    }
    false
}

fn search_mal(query: &str, config: &MalConfig) -> Vec<(u64, String)> {
    let url = format!(
        "https://api.myanimelist.net/v2/anime?q={}&limit=10",
        urlencoded(query)
    );

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.access_token))
        .send();

    let mut results = Vec::new();
    if let Ok(resp) = resp {
        if let Some(json) = parse_json(resp) {
            if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                for item in data {
                    if let Some(node) = item.get("node") {
                        let id = node.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                        let title = node
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string();
                        if id > 0 {
                            results.push((id, title));
                        }
                    }
                }
            }
        }
    }
    results
}

fn auto_search(name: &str, config: &MalConfig) -> Vec<(u64, String)> {
    // Try full name first, then progressively shorter queries
    let words: Vec<&str> = name.split_whitespace().collect();
    let attempts: Vec<String> = {
        let mut a = vec![name.to_string()];
        // Try first 7, 5, 3 words
        for &n in &[7, 5, 3] {
            if words.len() > n {
                a.push(words[..n].join(" "));
            }
        }
        a
    };

    for query in &attempts {
        let results = search_mal(query, config);
        if !results.is_empty() {
            return results;
        }
    }
    Vec::new()
}

pub fn resolve_mal_id(
    french_name: &str,
    config: &MalConfig,
    cache: &mut MalCache,
) -> Option<u64> {
    if let Some(&id) = cache.mappings.get(french_name) {
        return Some(id);
    }

    println!("Recherche sur MAL : {}...", french_name.cyan());
    let results = auto_search(french_name, config);

    if results.is_empty() {
        println!("{}", "Aucun résultat trouvé sur MAL.".yellow());
        // Fallback: let user search manually once
        let query = match Text::new("Recherche MAL (titre en anglais/japonais) :").prompt() {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => return None,
        };
        let results = search_mal(&query, config);
        if let Some((id, title)) = results.first() {
            println!("MAL : {} {}", title.green(), format!("(ID: {})", id).dimmed());
            cache.mappings.insert(french_name.to_string(), *id);
            save_cache(cache);
            return Some(*id);
        }
        return None;
    }

    // Auto-pick the first result
    let (id, title) = &results[0];
    println!("MAL : {} {}", title.green(), format!("(ID: {})", id).dimmed());
    cache.mappings.insert(french_name.to_string(), *id);
    save_cache(cache);
    Some(*id)
}

pub struct SkipTime {
    pub start: f64,
    pub end: f64,
    pub skip_type: String,
}

pub fn fetch_skip_times(mal_id: u64, episode: usize) -> Vec<SkipTime> {
    let url = format!(
        "https://api.aniskip.com/v2/skip-times/{}/{}?types=op&types=ed&episodeLength=0",
        mal_id, episode
    );

    let client = reqwest::blocking::Client::new();
    let resp = match client.get(&url).send() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let json = match parse_json(resp) {
        Some(j) => j,
        None => return Vec::new(),
    };

    let found = json.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
    if !found {
        return Vec::new();
    }

    let mut skip_times = Vec::new();
    if let Some(results) = json.get("results").and_then(|v| v.as_array()) {
        for item in results {
            let skip_type = item
                .get("skipType")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(interval) = item.get("interval") {
                let start = interval.get("startTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let end = interval.get("endTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if end > start {
                    skip_times.push(SkipTime { start, end, skip_type });
                }
            }
        }
    }
    skip_times
}

pub fn update_episode(mal_id: u64, episode: usize, is_completed: bool, config: &MalConfig) {
    let status = if is_completed { "completed" } else { "watching" };
    let ep_str = episode.to_string();

    let url = format!(
        "https://api.myanimelist.net/v2/anime/{}/my_list_status",
        mal_id
    );

    let body = format!(
        "status={}&num_watched_episodes={}",
        urlencoded(status),
        urlencoded(&ep_str)
    );

    let client = reqwest::blocking::Client::new();
    let resp = client
        .patch(&url)
        .header("Authorization", format!("Bearer {}", config.access_token))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send();

    match resp {
        Ok(r) => {
            let status_code = r.status();
            if status_code.is_success() {
                let status_text = if is_completed {
                    "complété".green().to_string()
                } else {
                    format!("épisode {}", episode).cyan().to_string()
                };
                println!("MAL mis à jour : {}", status_text);
            } else {
                println!("{} ({})", "Erreur mise à jour MAL".red(), status_code);
            }
        }
        Err(e) => println!("{} {}", "Erreur MAL:".red(), e),
    }
}

fn history_path() -> PathBuf {
    data_dir().join("watch_history.json")
}

pub fn load_history() -> WatchHistory {
    let path = history_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        WatchHistory::default()
    }
}

fn save_history(history: &WatchHistory) {
    let path = history_path();
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = std::fs::write(path, json);
    }
}

pub fn update_history(name: &str, lang: &str, season: i8, episode: usize, timestamp: f64) {
    let mut history = load_history();

    // Update existing entry or add new one
    if let Some(entry) = history.entries.iter_mut().find(|e| e.name == name) {
        entry.lang = lang.to_string();
        entry.season = season;
        entry.episode = episode;
        entry.timestamp = timestamp;
        entry.updated_at = Utc::now().timestamp();
    } else {
        history.entries.push(HistoryEntry {
            name: name.to_string(),
            lang: lang.to_string(),
            season,
            episode,
            timestamp,
            updated_at: Utc::now().timestamp(),
        });
    }

    history.entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    history.entries.truncate(50);

    save_history(&history);
}

pub fn last_position_path() -> PathBuf {
    data_dir().join("last_pos.txt")
}

pub fn read_last_position() -> f64 {
    let path = last_position_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        data.trim().parse().unwrap_or(0.0)
    } else {
        0.0
    }
}

pub fn format_timestamp(seconds: f64) -> String {
    let total = seconds as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}
