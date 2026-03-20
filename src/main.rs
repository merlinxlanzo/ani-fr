use crate::anime::*;
use colored::Colorize;
use data::get_file;
use directories::ProjectDirs;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::*;
use spinners::{Spinner, Spinners};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
};
use threadpool::ThreadPool;

mod anime;
mod autoclicker;
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

fn watch(link: &str) {
    let mpv_paths = ["mpv", "C:\\Program Files\\MPV Player\\mpv.exe"];
    for path in mpv_paths {
        if let Ok(mut child) = std::process::Command::new(path)
            .arg("--ytdl-format=bestvideo[height<=1080]+bestaudio/best[height<=1080]/best")
            .arg(link)
            .spawn()
        {
            let _ = child.wait();
            return;
        }
    }
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", link]).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(link).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(link).spawn();
}

fn main() {
    let file_path = ProjectDirs::from("", "B0SE", "ani-fr")
        .expect("Failed to get project directory")
        .data_dir()
        .join("anime_data.json");

    get_file(false);

    let mut sp = Spinner::new(Spinners::Moon, String::from("Chargement des animes"));

    let file = std::fs::File::open(file_path).unwrap();
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

        let mut all_anime_names = animes.get_name();
        all_anime_names.push("Otokurikka".to_string());
        all_anime_names.sort();
        all_anime_names.insert(0, mal_label.clone());

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

        let is_otokurikka = ans == "Otokurikka";

        let animes2 = if is_otokurikka {
            let episodes: Vec<String> = (1..=25).map(|i| format!("otokurikka_ep_{}", i)).collect();
            vec![
                Media::new("Otokurikka", "vf", 1, "anime", episodes.clone()),
                Media::new("Otokurikka", "vostfr", 1, "anime", episodes),
            ]
        } else {
            animes.get_seasons_from_str(&ans)
        };

        let vf = animes2.iter().any(|x| x.lang == "vf");

        'lang_loop: loop {
            let mut ans2 = String::from("vostfr");

            if vf {
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

            animes3.sort_by(|a, b| a.season.partial_cmp(&b.season).unwrap());

            'season_loop: loop {
                let ans3 = match Select::new(
                    "Sélectionnez la saison (Échap pour retour) : ",
                    animes3.clone(),
                )
                .prompt()
                {
                    Ok(v) => v,
                    Err(InquireError::OperationCanceled) => break 'season_loop,
                    Err(InquireError::OperationInterrupted) => std::process::exit(0),
                    Err(e) => panic!("{}", e),
                };

                'action_loop: loop {
                    let options = vec!["Télécharger", "Regarder"];

                    let ans4 = match Select::new(
                        "Voulez-vous télécharger ou regarder l'anime ? (Échap pour retour)",
                        options,
                    )
                    .prompt()
                    {
                        Ok(v) => v,
                        Err(InquireError::OperationCanceled) => break 'action_loop,
                        Err(InquireError::OperationInterrupted) => std::process::exit(0),
                        Err(e) => panic!("{}", e),
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

                        loop {
                            let ans5 = match Select::new(
                                "Sélectionnez l'épisode à regarder (Échap pour retour) : ",
                                episode_numbers.clone(),
                            )
                            .prompt()
                            {
                                Ok(v) => v,
                                Err(InquireError::OperationCanceled) => break,
                                Err(InquireError::OperationInterrupted) => std::process::exit(0),
                                Err(e) => panic!("{}", e),
                            };

                            let ep_idx = ans5.replace("Épisode ", "").parse::<usize>().unwrap() - 1;

                            if is_otokurikka {
                                autoclicker::run_episode(ep_idx as u32 + 1);
                            } else {
                                watch(&ans3.episodes[ep_idx]);
                            }

                            // MAL update after watching
                            if let Some(mal_id) = mal_anime_id {
                                if let Some(mut mal_config) = mal::load_config() {
                                    if mal::ensure_token(&mut mal_config) {
                                        let is_completed = ep_idx == ans3.episodes.len() - 1
                                            && ans3.season
                                                == animes3.last().unwrap().season;
                                        mal::update_episode(
                                            mal_id,
                                            ep_idx + 1,
                                            is_completed,
                                            &mal_config,
                                        );
                                    }
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
