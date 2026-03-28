use crate::anime::*;
use colored::Colorize;
use data::get_file;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::*;
use spinners::{Spinner, Spinners};
use std::{
    collections::HashMap,
    env,
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    thread,
};
use threadpool::ThreadPool;

static DEBUG: AtomicBool = AtomicBool::new(false);

struct WatchingState {
    name: String,
    lang: String,
    season: i8,
    episode: usize,
    mal_id: Option<u64>,
    total_episodes: usize,
    is_last_season: bool,
}

static WATCHING: Mutex<Option<WatchingState>> = Mutex::new(None);

fn is_debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

mod anime;
mod ext;
mod data;
mod mal;

fn to_title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn download(anime: &Media, selected_indices: Vec<usize>) -> anyhow::Result<()> {
    let anime_name_title = to_title_case(&anime.name);
    let season_dir = Path::new(&anime_name_title).join(format!("S{}", anime.season));

    if !season_dir.exists() {
        fs::create_dir_all(&season_dir)?;
    }

    let pool = ThreadPool::new(12);
    let m = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{spinner:.blue} [{elapsed_precise}] [{bar:40.green/white}] {percent:>3}% {msg}",
    )?
    .progress_chars("=>-");

    let anime_name = anime_name_title.clone();
    let anime_season = anime.season;

    for &index in &selected_indices {
        let episode_url = anime.episodes[index].clone();
        let m = m.clone();
        let style = style.clone();
        let season_dir = season_dir.clone();
        let anime_name = anime_name.clone();
        let episode_num = index + 1;

        pool.execute(move || {
            let output_template = format!(
                "{}/{} S{}E{:02}.%(ext)s",
                season_dir.display(),
                anime_name,
                anime_season,
                episode_num
            );
            let pb = m.add(ProgressBar::new(100));
            pb.set_style(style);
            pb.set_message(format!("| Épisode {:02}", episode_num));

            let mut child = match Command::new("yt-dlp")
                .arg("--newline")
                .arg("--progress")
                .arg("-o")
                .arg(&output_template)
                .arg(&episode_url)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(err) => {
                    pb.abandon_with_message(format!("Erreur lancement yt-dlp: {}", err));
                    return;
                }
            };

            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);

                for line in reader.lines().map_while(Result::ok) {
                    if !line.contains("[download]") {
                        continue;
                    }

                    if let Some(percent) = extract_percent(&line) {
                        pb.set_position(percent as u64);
                    }

                    if let Some(speed) = extract_speed(&line) {
                        pb.set_message(format!(
                            "| Épisode {:02} | {}",
                            episode_num,
                            speed.yellow()
                        ));
                    }
                }
            }

            match child.wait() {
                Ok(status) if status.success() => {
                    pb.finish_with_message(format!(
                        "| Épisode {:02} | {}",
                        episode_num,
                        "terminé".cyan()
                    ));
                }
                _ => {
                    pb.abandon_with_message(format!(
                        "| Épisode {:02} | {}",
                        episode_num,
                        "échec".red()
                    ));
                }
            }
        });
    }

    pool.join();
    Ok(())
}

fn extract_percent(line: &str) -> Option<f32> {
    let percent_pos = line.find('%')?;
    let start = line[..percent_pos].rfind(' ')?;
    line[start..percent_pos].trim().parse().ok()
}

fn extract_speed(line: &str) -> Option<&str> {
    let at = line.find(" at ")? + 4;
    let eta = line.find(" ETA ")?;
    Some(line[at..eta].trim())
}

fn fullscreen_state_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "B0SE", "ani-fr")
        .expect("Failed to get project directory")
        .data_dir()
        .join("fs_state.txt")
}

fn next_episode_signal_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "B0SE", "ani-fr")
        .expect("Failed to get project directory")
        .data_dir()
        .join("next_episode.signal")
}

fn skip_cache_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "B0SE", "ani-fr")
        .expect("Failed to get project directory")
        .data_dir()
        .join("skip_cache.json")
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct CachedSkip {
    skip_type: String, // "op" or "ed"
    start: f64,
    end: f64,
}

fn load_skip_cache(anime_name: &str, season: i8) -> Vec<CachedSkip> {
    let path = skip_cache_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let map: HashMap<String, Vec<CachedSkip>> = serde_json::from_str(&data).unwrap_or_default();
    let key = format!("{}|{}", anime_name.to_lowercase(), season);
    map.get(&key).cloned().unwrap_or_default()
}

fn save_skip_cache(anime_name: &str, season: i8, skips: &[CachedSkip]) {
    let path = skip_cache_path();
    let data = std::fs::read_to_string(&path).unwrap_or_default();
    let mut map: HashMap<String, Vec<CachedSkip>> = serde_json::from_str(&data).unwrap_or_default();
    let key = format!("{}|{}", anime_name.to_lowercase(), season);
    map.insert(key, skips.to_vec());
    if let Ok(json) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, json);
    }
}

fn parse_chapters_debug() -> Vec<CachedSkip> {
    let dbg_path = directories::ProjectDirs::from("", "B0SE", "ani-fr")
        .unwrap().data_dir().join("chapters_debug.txt");
    let content = match std::fs::read_to_string(&dbg_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // Parse chapter lines: [idx] "Title" @ 123.4s
    let mut chapters: Vec<(String, f64)> = Vec::new();
    for line in content.lines() {
        if let Some(start) = line.find('"') {
            if let Some(end) = line[start+1..].find('"') {
                let title = line[start+1..start+1+end].to_string();
                if let Some(at) = line.find("@ ") {
                    if let Some(s_off) = line[at+2..].find('s') {
                        let s_end = at + 2 + s_off;
                        if let Ok(time) = line[at+2..s_end].parse::<f64>() {
                            chapters.push((title, time));
                        }
                    }
                }
            }
        }
    }

    let mut skips = Vec::new();
    for (i, (title, start)) in chapters.iter().enumerate() {
        let t = title.to_lowercase();
        let is_op = t == "op" || t == "opening" || t == "intro" || t.contains("opening") || t.contains("intro");
        let is_ed = t == "ed" || t == "ending" || t == "credits" || t.contains("ending") || t.contains("credits");
        if is_op || is_ed {
            let end = if i + 1 < chapters.len() {
                chapters[i + 1].1
            } else {
                start + 90.0 // fallback
            };
            skips.push(CachedSkip {
                skip_type: if is_op { "op".to_string() } else { "ed".to_string() },
                start: *start,
                end,
            });
        }
    }
    skips
}

fn write_mpv_script(skip_times: &[mal::SkipTime], cached_skips: &[CachedSkip], auto_next: bool) -> Option<std::path::PathBuf> {
    let pos_file = mal::last_position_path();
    let pos_file_escaped = pos_file.display().to_string().replace('\\', "\\\\");
    let signal_file_escaped = next_episode_signal_path()
        .display()
        .to_string()
        .replace('\\', "\\\\");

    let mut lua = String::new();

    // Skip times section
    let has_aniskip = !skip_times.is_empty();
    if has_aniskip {
        lua.push_str("local skips = {\n");
        for s in skip_times {
            lua.push_str(&format!(
                "  {{start={:.3}, ends={:.3}, type=\"{}\"}},\n",
                s.start, s.end, s.skip_type
            ));
        }
        lua.push_str("}\n");
        lua.push_str(&format!(
            r#"local skipped = {{}}
local auto_next = {}
local skip_timer = nil
mp.observe_property("time-pos", "number", function(_, pos)
    if pos then
        for i, s in ipairs(skips) do
            if not skipped[i] and pos >= s.start and pos < s.ends then
                skipped[i] = true
                if skip_timer then skip_timer:kill() end
                local label = string.upper(s.type)
                mp.osd_message("Skip " .. label .. " dans 5s...", 5)
                skip_timer = mp.add_timeout(5, function()
                    if string.lower(s.type) == "ed" and auto_next then
                        mp.osd_message("Épisode suivant...", 2)
                        local f = io.open("{}", "w")
                        if f then
                            f:write("next")
                            f:close()
                        end
                        mp.command("quit")
                        return
                    end
                    mp.commandv("seek", tostring(s.ends), "absolute")
                    mp.osd_message("Skip " .. label, 2)
                end)
            end
        end
    end
end)
"#,
            if auto_next { "true" } else { "false" },
            signal_file_escaped
        ));
    }

    // Fallback: use mpv embedded chapters (OP/ED) when AniSkip has no data
    // Also always use chapter fallback alongside AniSkip data
    {
        lua.push_str(&format!(
            r#"local chapter_skipped = {{}}
local chapter_auto_next = {}
local chapters_logged = false
local dbg_file
local chapter_skip_timer = nil
local function check_chapters()
    local count = mp.get_property_number("chapter-list/count", 0)
    if count == 0 then return end
    if not chapters_logged then
        chapters_logged = true
        dbg_file = io.open("{dbg}", "w")
        if dbg_file then
            dbg_file:write("Chapters found: " .. count .. "\n")
            for j = 0, count - 1 do
                local ct = mp.get_property("chapter-list/" .. j .. "/title", "?")
                local cs = mp.get_property_number("chapter-list/" .. j .. "/time", 0)
                dbg_file:write("  [" .. j .. "] \"" .. ct .. "\" @ " .. string.format("%.1f", cs) .. "s\n")
            end
            dbg_file:flush()
        end
    end
    local pos = mp.get_property_number("time-pos")
    if not pos then return end
    for i = 0, count - 1 do
        local title = mp.get_property("chapter-list/" .. i .. "/title", "")
        local start = mp.get_property_number("chapter-list/" .. i .. "/time", 0)
        local next_start = nil
        if i + 1 < count then
            next_start = mp.get_property_number("chapter-list/" .. (i + 1) .. "/time", nil)
        else
            next_start = mp.get_property_number("duration", nil)
        end
        if title and next_start then
            local t = string.lower(title)
            local is_op = t == "op" or t == "opening" or t == "intro" or string.find(t, "opening") or string.find(t, "intro")
            local is_ed = t == "ed" or t == "ending" or t == "credits" or string.find(t, "ending") or string.find(t, "credits")
            if (is_op or is_ed) and not chapter_skipped[i] and pos >= start and pos < next_start then
                chapter_skipped[i] = true
                if dbg_file then
                    dbg_file:write("SKIP triggered: \"" .. title .. "\" pos=" .. string.format("%.1f", pos) .. " -> " .. string.format("%.1f", next_start) .. "\n")
                    dbg_file:flush()
                end
                if chapter_skip_timer then chapter_skip_timer:kill() end
                local skip_target = next_start
                local skip_title = title
                local skip_is_ed = is_ed
                mp.osd_message("Skip " .. string.upper(skip_title) .. " dans 5s...", 5)
                chapter_skip_timer = mp.add_timeout(5, function()
                    if skip_is_ed and chapter_auto_next then
                        mp.osd_message("Épisode suivant...", 2)
                        local f = io.open("{}", "w")
                        if f then
                            f:write("next")
                            f:close()
                        end
                        mp.command("quit")
                        return
                    end
                    mp.commandv("seek", tostring(skip_target), "absolute")
                    mp.osd_message("Skip " .. string.upper(skip_title), 2)
                end)
            end
        end
    end
end
mp.observe_property("time-pos", "number", function(_, pos)
    if pos then check_chapters() end
end)
mp.observe_property("chapter-list/count", "number", function()
    check_chapters()
end)
"#,
            if auto_next { "true" } else { "false" },
            signal_file_escaped,
            dbg = directories::ProjectDirs::from("", "B0SE", "ani-fr")
                .unwrap().data_dir().join("chapters_debug.txt")
                .display().to_string().replace('\\', "\\\\")
        ));
    }

    // Cached skip times: show Netflix-style "Skip Intro/Outro" button
    if !cached_skips.is_empty() {
        lua.push_str("local cached_skips = {\n");
        for s in cached_skips {
            lua.push_str(&format!(
                "  {{start={:.3}, ends={:.3}, type=\"{}\"}},\n",
                s.start, s.end, s.skip_type
            ));
        }
        lua.push_str("}\n");
        lua.push_str(&format!(
            r#"local cached_auto_next = {}
local cached_active = nil
local cached_triggered = {{}}
local overlay = mp.create_osd_overlay("ass-events")
local function show_skip_button(label)
    overlay.data = "{{\\an3\\fs28\\bord2\\shad1\\1c&HFFFFFF&\\3c&H000000&\\pos(" ..
        (mp.get_property_number("osd-width", 1920) - 40) .. "," ..
        (mp.get_property_number("osd-height", 1080) - 60) .. ")}}" ..
        "⏭ Skip " .. label
    overlay:update()
end
local function hide_skip_button()
    overlay:remove()
    cached_active = nil
    mp.remove_key_binding("cached-skip")
end
local function do_cached_skip()
    if not cached_active then return end
    local s = cached_active
    hide_skip_button()
    if string.lower(s.type) == "ed" and cached_auto_next then
        mp.osd_message("Épisode suivant...", 2)
        local f = io.open("{}", "w")
        if f then
            f:write("next")
            f:close()
        end
        mp.command("quit")
        return
    end
    mp.commandv("seek", tostring(s.ends), "absolute")
    mp.osd_message("Skip " .. string.upper(s.type), 2)
end
mp.observe_property("time-pos", "number", function(_, pos)
    if not pos then return end
    for i, s in ipairs(cached_skips) do
        if not cached_triggered[i] and pos >= s.start and pos < s.ends then
            cached_triggered[i] = true
            cached_active = s
            local label = s.type == "op" and "Intro" or "Outro"
            show_skip_button(label)
            mp.add_forced_key_binding("ENTER", "cached-skip", do_cached_skip)
        end
    end
    if cached_active and pos >= cached_active.ends then
        hide_skip_button()
    end
end)
"#,
            if auto_next { "true" } else { "false" },
            signal_file_escaped
        ));
    }

    // Auto-next when episode finishes naturally (EOF)
    if auto_next {
        lua.push_str(&format!(
            r#"mp.register_event("end-file", function(event)
    if event.reason == "eof" then
        local f = io.open("{}", "w")
        if f then
            f:write("next")
            f:close()
        end
    end
end)
"#,
            signal_file_escaped
        ));
    }

    lua.push_str(&format!(
        r#"local last_saved_pos = 0
mp.observe_property("time-pos", "number", function(_, pos)
    if pos and math.abs(pos - last_saved_pos) >= 5 then
        last_saved_pos = pos
        local f = io.open("{pos}", "w")
        if f then
            f:write(string.format("%.3f", pos))
            f:close()
        end
    end
end)
mp.register_event("shutdown", function()
    local pos = mp.get_property_number("time-pos")
    if pos then
        local f = io.open("{pos}", "w")
        if f then
            f:write(string.format("%.3f", pos))
            f:close()
        end
    end
    local fs = mp.get_property_bool("fullscreen")
    local f = io.open("{fs}", "w")
    if f then
        f:write(fs and "1" or "0")
        f:close()
    end
end)
"#,
        pos = pos_file_escaped,
        fs = fullscreen_state_path().display().to_string().replace('\\', "\\\\")
    ));

    let dir = directories::ProjectDirs::from("", "B0SE", "ani-fr")
        .expect("Failed to get project directory");
    let script_path = dir.data_dir().join("aniskip.lua");
    if std::fs::write(&script_path, &lua).is_ok() {
        Some(script_path)
    } else {
        None
    }
}

fn was_fullscreen() -> bool {
    std::fs::read_to_string(fullscreen_state_path())
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

fn watch(link: &str, skip_times: &[mal::SkipTime], cached_skips: &[CachedSkip], auto_next: bool, start_pos: Option<f64>) -> bool {
    let restore_fs = was_fullscreen();
    // Clear old signals
    let _ = std::fs::remove_file(mal::last_position_path());
    let _ = std::fs::remove_file(next_episode_signal_path());
    let _ = std::fs::remove_file(fullscreen_state_path());

    let script_path = write_mpv_script(skip_times, cached_skips, auto_next);
    let mpv_paths = ["mpv", "C:\\Program Files\\MPV Player\\mpv.exe"];
    for path in mpv_paths {
        let mut cmd = std::process::Command::new(path);
        cmd.arg("--ytdl-format=bestvideo[height<=1080]+bestaudio/best[height<=1080]/best");
        if restore_fs {
            cmd.arg("--fs");
        }
        if let Some(ref sp) = script_path {
            cmd.arg(format!("--script={}", sp.display()));
        }
        if let Some(pos) = start_pos {
            cmd.arg(format!("--start={:.3}", pos));
        }
        cmd.arg(link);
        if is_debug() {
            cmd.stderr(Stdio::piped());
        }
        if let Ok(mut child) = cmd.spawn() {
            let status = child.wait();
            if is_debug() {
                if let Some(stderr) = child.stderr.take() {
                    let err_output: String = BufReader::new(stderr)
                        .lines()
                        .filter_map(|l| l.ok())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !err_output.is_empty() {
                        eprintln!("{}", err_output);
                    }
                }
                if let Ok(s) = &status {
                    if !s.success() {
                        eprintln!("{}", format!("mpv exited with: {}", s).red());
                    }
                }
            }
            if is_debug() {
                let dbg_path = directories::ProjectDirs::from("", "B0SE", "ani-fr")
                    .unwrap().data_dir().join("chapters_debug.txt");
                if let Ok(content) = std::fs::read_to_string(&dbg_path) {
                    eprintln!("[DEBUG] {}", content.trim());
                } else {
                    eprintln!("[DEBUG] No chapters detected by mpv");
                }
                let _ = std::fs::remove_file(&dbg_path);
            }
            return next_episode_signal_path().exists();
        }
    }
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", link]).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(link).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(link).spawn();
    false
}

fn save_on_exit() {
    let state = WATCHING.lock().unwrap();
    if let Some(ws) = state.as_ref() {
        // Update MAL
        if let Some(mal_id) = ws.mal_id {
            if let Some(mut mal_config) = mal::load_config() {
                if mal::ensure_token(&mut mal_config) {
                    let is_completed = ws.episode == ws.total_episodes
                        && ws.is_last_season;
                    mal::update_episode(mal_id, ws.episode, is_completed, &mal_config);
                }
            }
        }
        // Save history
        let last_pos = mal::read_last_position();
        mal::update_history(&ws.name, &ws.lang, ws.season, ws.episode, last_pos);
    }
}

fn main() {
    if env::args().any(|a| a == "--debug") {
        DEBUG.store(true, Ordering::Relaxed);
        eprintln!("{}", "[DEBUG MODE]".yellow());
    }

    ctrlc::set_handler(|| {
        save_on_exit();
        std::process::exit(0);
    }).ok();

    let file_path = data::data_file_path();

    data::ensure_local_data(&file_path);

    let mut sp = Spinner::new(Spinners::Moon, String::from("Chargement des animes"));

    let file = std::fs::File::open(&file_path).unwrap();
    let animes: Medias = match serde_json::from_reader(&file) {
        Ok(v) => v,
        Err(_e) => {
            get_file(true);
            eprintln!("\nNouvelle base de données téléchargée, veuillez relancer le programme. Si le problème persiste, veuillez ouvrir une issue sur GitHub.");
            std::process::exit(0);
        }
    };
    drop(file);

    data::sync_remote_in_background(file_path.clone());

    sp.stop_with_symbol(" ✔️ ");

    'main_loop: loop {
        let mal_label = if mal::is_logged_in() {
            ">>> MAL connecté <<<".to_string()
        } else {
            ">>> Connexion MAL <<<".to_string()
        };

        let mut all_anime_names: Vec<String> = Vec::new();
        all_anime_names.push(mal_label.clone());

        // Add recent watch history at the top
        let history = mal::load_history();
        let mut history_labels: Vec<String> = Vec::new();
        if !history.entries.is_empty() {
            all_anime_names.push("─── Récemment regardés ───".to_string());
            for entry in history.entries.iter().take(3) {
                let label = format!(
                    "↪ {} - Ép.{} ({})",
                    entry.name,
                    entry.episode,
                    mal::format_timestamp(entry.timestamp)
                );
                history_labels.push(label.clone());
                all_anime_names.push(label);
            }
            all_anime_names.push("───────────────────────".to_string());
        }

        let mut anime_names = animes.get_name();
        anime_names.push("\u{4f}\u{74}\u{6f}\u{6b}\u{75}\u{72}\u{69}\u{6b}\u{6b}\u{61}".to_string());
        anime_names.sort();
        all_anime_names.extend(anime_names);

        let ans = match Select::new(
            "Sélectionnez les animes (Échap pour quitter) : ",
            all_anime_names,
        )
        .prompt()
        {
            Ok(v) => v,
            Err(InquireError::OperationInterrupted | InquireError::OperationCanceled) => {
                break 'main_loop;
            }
            Err(e) => panic!("{}", e),
        };

        // Skip separators
        if ans == "───────────────────────" {
            continue 'main_loop;
        }

        let ans = if ans == "─── Récemment regardés ───" {
            let full_history_labels: Vec<String> = history
                .entries
                .iter()
                .map(|e| {
                    format!(
                        "↪ {} - Ép.{} ({})",
                        e.name,
                        e.episode,
                        mal::format_timestamp(e.timestamp)
                    )
                })
                .collect();

            match Select::new(
                "Historique complet (Échap pour retour) : ",
                full_history_labels.clone(),
            )
            .prompt()
            {
                Ok(selected) => {
                    history_labels = full_history_labels;
                    selected
                }
                Err(_) => continue 'main_loop,
            }
        } else {
            ans
        };


        let mut history_episode: Option<usize> = None;
        let mut history_lang: Option<String> = None;
        let mut history_season: Option<i8> = None;
        let mut history_timestamp: Option<f64> = None;
        let ans = if let Some(hist_entry) = history_labels.iter().position(|l| *l == ans) {
            let entry = &history.entries[hist_entry];
            history_episode = Some(entry.episode);
            history_lang = Some(entry.lang.clone());
            history_season = Some(entry.season);
            if entry.timestamp > 0.0 {
                history_timestamp = Some(entry.timestamp);
            }
            entry.name.clone()
        } else {
            ans
        };

        if ans == mal_label {
            if mal::is_logged_in() {
                let options = vec!["Ma liste anime", "Mon historique", "Déconnexion", "Retour"];
                if let Ok(choice) = Select::new("MAL :", options).prompt() {
                    match choice {
                        "Ma liste anime" => {
                            if let Some(mut cfg) = mal::load_config() {
                                if mal::ensure_token(&mut cfg) {
                                    if let Some(name) = mal::get_username(&cfg) {
                                        let url = format!("https://myanimelist.net/animelist/{}", name);
                                        let _ = open::that(&url);
                                    }
                                }
                            }
                        }
                        "Mon historique" => {
                            if let Some(mut cfg) = mal::load_config() {
                                if mal::ensure_token(&mut cfg) {
                                    if let Some(name) = mal::get_username(&cfg) {
                                        let url = format!("https://myanimelist.net/history/{}", name);
                                        let _ = open::that(&url);
                                    }
                                }
                            }
                        }
                        "Déconnexion" => {
                            mal::logout();
                        }
                        _ => {}
                    }
                }
            } else {
                mal::login();
            }
            continue 'main_loop;
        }

        // Resolve MAL ID early so watching is seamless
        let mal_anime_id: Option<u64> = {
            let mut cache = mal::load_cache();
            if mal::is_logged_in() {
                if let Some(mut mal_config) = mal::load_config() {
                    if mal::ensure_token(&mut mal_config) {
                        mal::resolve_mal_id(&ans, &mal_config, &mut cache)
                    } else {
                        mal::resolve_mal_id_public(&ans, &mut cache)
                    }
                } else {
                    mal::resolve_mal_id_public(&ans, &mut cache)
                }
            } else {
                mal::resolve_mal_id_public(&ans, &mut cache)
            }
        };

        let is_ext = ans == "\u{4f}\u{74}\u{6f}\u{6b}\u{75}\u{72}\u{69}\u{6b}\u{6b}\u{61}";

        let animes2 = if is_ext {
            let episodes: Vec<String> = (1..=25).map(|i| format!("{}_ep_{}", "\u{6f}\u{74}\u{6b}", i)).collect();
            vec![
                Media::new("\u{4f}\u{74}\u{6f}\u{6b}\u{75}\u{72}\u{69}\u{6b}\u{6b}\u{61}", "vf", 1, "anime", episodes.clone()),
                Media::new("\u{4f}\u{74}\u{6f}\u{6b}\u{75}\u{72}\u{69}\u{6b}\u{6b}\u{61}", "vostfr", 1, "anime", episodes),
            ]
        } else {
            animes.get_seasons_from_str(&ans)
        };

        let vf = animes2.iter().any(|x| x.lang == "vf");
        let from_history = history_lang.is_some();

        'lang_loop: loop {
            let mut ans2 = String::from("vostfr");

            if from_history {
                ans2 = history_lang.take().unwrap_or_else(|| "vostfr".to_string());
            } else if vf {
                ans2 = match Select::new("VF ou VOSTFR ? (Échap pour retour)", vec!["VF", "VOSTFR"])
                    .prompt()
                {
                    Ok(v) => String::from(v),
                    Err(InquireError::OperationCanceled) => break 'lang_loop,
                    Err(InquireError::OperationInterrupted) => std::process::exit(0),
                    Err(e) => panic!("{}", e),
                };
            } else {
                println!("Pas de VF disponible");
            }

            let mut animes3: Vec<Media> = animes2
                .iter()
                .filter(|x| x.lang == ans2.to_lowercase())
                .cloned()
                .collect();

            if animes3.is_empty() {
                println!("Aucune saison disponible pour cette langue.");
                if !vf {
                    break 'lang_loop;
                }
                continue 'lang_loop;
            }

            let can_change_lang = vf && !from_history;

            // Sort: seasons first (by number), then films
            animes3.sort_by(|a, b| {
                let a_is_film = a.media_type == "film";
                let b_is_film = b.media_type == "film";
                a_is_film.cmp(&b_is_film).then(a.season.cmp(&b.season))
            });

            let mut used_history_shortcut = from_history;
            'season_loop: loop {
                let ans3 = if used_history_shortcut {
                    let target_season = history_season.unwrap_or(1);
                    match animes3.iter().find(|a| a.season == target_season) {
                        Some(a) => a.clone(),
                        None => animes3[0].clone(),
                    }
                } else if animes3.len() == 1 {
                    // Only one option (single season or single film) — skip selection
                    animes3[0].clone()
                } else {
                    match Select::new(
                        "Sélectionnez la saison / film (Échap pour retour) : ",
                        animes3.clone(),
                    )
                    .prompt()
                    {
                        Ok(v) => v,
                        Err(InquireError::OperationCanceled) => {
                            if can_change_lang {
                                break 'season_loop;
                            } else {
                                break 'lang_loop;
                            }
                        }
                        Err(InquireError::OperationInterrupted) => std::process::exit(0),
                        Err(e) => panic!("{}", e),
                    }
                };

                'action_loop: loop {
                    let mut skip_episode_select = used_history_shortcut;
                    let ans4 = if used_history_shortcut {
                        used_history_shortcut = false;
                        "Regarder"
                    } else {
                        let options = vec!["Télécharger", "Regarder"];

                        match Select::new(
                            "Voulez-vous télécharger ou regarder l'anime ? (Échap pour retour)",
                            options,
                        )
                        .prompt()
                        {
                            Ok(v) => v,
                            Err(InquireError::OperationCanceled) => {
                                if animes3.len() == 1 {
                                    if can_change_lang {
                                        break 'season_loop;
                                    } else {
                                        break 'lang_loop;
                                    }
                                } else {
                                    break 'action_loop;
                                }
                            }
                            Err(InquireError::OperationInterrupted) => std::process::exit(0),
                            Err(e) => panic!("{}", e),
                        }
                    };

                    if ans4 == "Télécharger" {
                        let mut ep_choices = vec![];
                        for i in 1..=ans3.episodes.len() {
                            ep_choices.push(format!("Épisode {}", i));
                        }

                        let selected_eps = match MultiSelect::new(
                            "Sélectionnez les épisodes à télécharger (Espace pour choisir, Échap pour retour) : ",
                            ep_choices,
                        )
                        .prompt()
                        {
                            Ok(v) => v,
                            Err(InquireError::OperationCanceled) => continue 'action_loop,
                            Err(InquireError::OperationInterrupted) => std::process::exit(0),
                            Err(e) => panic!("{}", e),
                        };

                        if selected_eps.is_empty() {
                            println!("{}", "Aucun épisode sélectionné.".yellow());
                            continue 'action_loop;
                        }

                        let indices: Vec<usize> = selected_eps
                            .iter()
                            .map(|s| s.replace("Épisode ", "").parse::<usize>().unwrap() - 1)
                            .collect();

                        if let Err(e) = download(&ans3, indices) {
                            eprintln!("Erreur lors du téléchargement: {}", e);
                        }
                    } else {
                        let mut episode_numbers = vec![];
                        for i in 1..=ans3.episodes.len() {
                            episode_numbers.push(format!("Épisode {}", i));
                        }

                        let skip_cache: Arc<Mutex<HashMap<usize, Vec<mal::SkipTime>>>> =
                            Arc::new(Mutex::new(HashMap::new()));
                        if let Some(mal_id) = mal_anime_id {
                            let cache = skip_cache.clone();
                            let ep_count = ans3.episodes.len();
                            thread::spawn(move || {
                                for ep in 1..=ep_count {
                                    let times = mal::fetch_skip_times(mal_id, ep);
                                    cache.lock().unwrap().insert(ep, times);
                                }
                            });
                        }

                        let mut last_ep_idx: usize = history_episode
                            .map(|e| (e - 1).min(ans3.episodes.len() - 1))
                            .unwrap_or(0);
                        loop {
                            let ep_idx = if skip_episode_select {
                                skip_episode_select = false;
                                last_ep_idx
                            } else {
                                let ans5 = match Select::new(
                                    "Sélectionnez l'épisode à regarder (Échap pour retour) : ",
                                    episode_numbers.clone(),
                                )
                                .with_starting_cursor(last_ep_idx)
                                .prompt()
                                {
                                    Ok(v) => v,
                                    Err(InquireError::OperationCanceled) => break,
                                    Err(InquireError::OperationInterrupted) => std::process::exit(0),
                                    Err(e) => panic!("{}", e),
                                };
                                ans5.replace("Épisode ", "").parse::<usize>().unwrap() - 1
                            };
                            last_ep_idx = ep_idx;

                            let mut current_ep = ep_idx;
                            loop {
                                if is_ext {
                                    ext::run_episode(current_ep as u32 + 1);
                                    break;
                                }

                                let has_next = current_ep + 1 < ans3.episodes.len();
                                let skip_times = if let Some(mal_id) = mal_anime_id {
                                    let ep_num = current_ep + 1;
                                    let cached = {
                                        let mut attempts = 0;
                                        loop {
                                            if let Some(times) = skip_cache.lock().unwrap().remove(&ep_num) {
                                                break Some(times);
                                            }
                                            attempts += 1;
                                            if attempts > 20 { break None; }
                                            std::thread::sleep(std::time::Duration::from_millis(100));
                                        }
                                    };
                                    cached.unwrap_or_else(|| mal::fetch_skip_times(mal_id, ep_num))
                                } else {
                                    Vec::new()
                                };
                                let ep_url = &ans3.episodes[current_ep];
                                if is_debug() {
                                    eprintln!("[DEBUG] Episode URL: {}", ep_url);
                                    for st in &skip_times {
                                        eprintln!("[DEBUG] Skip: type={} start={:.1} end={:.1}", st.skip_type, st.start, st.end);
                                    }
                                    if skip_times.is_empty() {
                                        eprintln!("[DEBUG] No skip times found");
                                    }
                                }
                                // Load cached skip times if no AniSkip data
                                let cached_skips = if skip_times.is_empty() {
                                    load_skip_cache(&ans, ans3.season)
                                } else {
                                    Vec::new()
                                };
                                if is_debug() && !cached_skips.is_empty() {
                                    eprintln!("[DEBUG] Using cached skip times: {:?}", cached_skips);
                                }
                                let resume_pos = history_timestamp.take();
                                // Set watching state for Ctrl+C failsafe
                                *WATCHING.lock().unwrap() = Some(WatchingState {
                                    name: ans.clone(),
                                    lang: ans3.lang.clone(),
                                    season: ans3.season,
                                    episode: current_ep + 1,
                                    mal_id: mal_anime_id,
                                    total_episodes: ans3.episodes.len(),
                                    is_last_season: ans3.season == animes3.last().unwrap().season,
                                });
                                let auto_next = watch(ep_url, &skip_times, &cached_skips, has_next, resume_pos);
                                // Clear watching state (normal exit will handle saving below)
                                *WATCHING.lock().unwrap() = None;

                                // Save detected chapters to cache for future episodes
                                let detected = parse_chapters_debug();
                                if !detected.is_empty() {
                                    save_skip_cache(&ans, ans3.season, &detected);
                                    if is_debug() {
                                        eprintln!("[DEBUG] Saved skip cache: {:?}", detected);
                                    }
                                }

                                // MAL update after watching
                                if let Some(mal_id) = mal_anime_id {
                                    if let Some(mut mal_config) = mal::load_config() {
                                        if mal::ensure_token(&mut mal_config) {
                                            let is_completed = current_ep == ans3.episodes.len() - 1
                                                && ans3.season
                                                    == animes3.last().unwrap().season;
                                            mal::update_episode(
                                                mal_id,
                                                current_ep + 1,
                                                is_completed,
                                                &mal_config,
                                            );
                                        }
                                    }
                                }

                                // Save watch history
                                let last_pos = mal::read_last_position();
                                mal::update_history(&ans, &ans3.lang, ans3.season, current_ep + 1, last_pos);

                                // Auto-advance to next episode if ED was skipped
                                if auto_next && has_next {
                                    current_ep += 1;
                                    last_ep_idx = current_ep;
                                    println!(
                                        "{}",
                                        format!("▶ Épisode {} automatique...", current_ep + 1).cyan()
                                    );
                                    let _ = std::fs::remove_file(next_episode_signal_path());
                                } else {
                                    last_ep_idx = current_ep;
                                    break;
                                }
                            }
                        }
                    }
                }

                if !vf {
                    break 'lang_loop;
                }
            }
        }
    }
}
