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

fn write_mpv_script(skip_times: &[mal::SkipTime], auto_next: bool) -> Option<std::path::PathBuf> {
    let pos_file = mal::last_position_path();
    let pos_file_escaped = pos_file.display().to_string().replace('\\', "\\\\");
    let signal_file_escaped = next_episode_signal_path()
        .display()
        .to_string()
        .replace('\\', "\\\\");

    let mut lua = String::new();

    // Skip times section
    if !skip_times.is_empty() {
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
mp.observe_property("time-pos", "number", function(_, pos)
    if pos then
        for i, s in ipairs(skips) do
            if not skipped[i] and pos >= s.start and pos < s.ends then
                skipped[i] = true
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
                mp.osd_message("Skip " .. string.upper(s.type), 2)
            end
        end
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

fn watch(link: &str, skip_times: &[mal::SkipTime], auto_next: bool, start_pos: Option<f64>) -> bool {
    let restore_fs = was_fullscreen();
    // Clear old signals
    let _ = std::fs::remove_file(mal::last_position_path());
    let _ = std::fs::remove_file(next_episode_signal_path());
    let _ = std::fs::remove_file(fullscreen_state_path());

    let script_path = write_mpv_script(skip_times, auto_next);
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

fn main() {
    if env::args().any(|a| a == "--debug") {
        DEBUG.store(true, Ordering::Relaxed);
        eprintln!("{}", "[DEBUG MODE]".yellow());
    }

    let file_path = data::data_file_path();

    if !data::ensure_local_data(&file_path) {
        data::sync_remote_in_background(file_path.clone());
    }

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
                let options = vec!["Déconnexion", "Retour"];
                if let Ok(choice) = Select::new("MAL :", options).prompt() {
                    if choice == "Déconnexion" {
                        mal::logout();
                    }
                }
            } else {
                mal::login();
            }
            continue 'main_loop;
        }

        // Resolve MAL ID early so watching is seamless
        let mal_anime_id: Option<u64> = if mal::is_logged_in() {
            if let Some(mut mal_config) = mal::load_config() {
                if mal::ensure_token(&mut mal_config) {
                    let mut cache = mal::load_cache();
                    mal::resolve_mal_id(&ans, &mal_config, &mut cache)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
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
                                let resume_pos = history_timestamp.take();
                                let auto_next = watch(ep_url, &skip_times, has_next, resume_pos);

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
