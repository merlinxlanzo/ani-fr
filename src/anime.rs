use colored::*;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Serialize, Deserialize, Debug)]
pub struct Medias {
    pub media: Vec<Media>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Media {
    pub name: String,
    pub lang: String,
    pub season: i8,
    media_type: String,
    pub episodes: Vec<String>,
}

impl Media {
    pub fn new(name: &str, lang: &str, season: i8, media_type: &str, episodes: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            lang: lang.to_string(),
            season,
            media_type: media_type.to_string(),
            episodes,
        }
    }
}

impl Display for Media {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "saison {}", self.season.to_string().yellow())
    }
}

fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Medias {
    pub fn get_name(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for anime in &self.media {
            let norm = normalize_name(&anime.name);
            if seen.insert(norm) {
                names.push(anime.name.clone());
            }
        }
        names
    }
    pub fn get_seasons_from_str(&self, name: &str) -> Vec<Media> {
        let norm = normalize_name(name);
        self.media
            .iter()
            .filter(|x| normalize_name(&x.name) == norm)
            .cloned()
            .collect()
    }
}
