use colored::*;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

fn dedup_consecutive(episodes: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::with_capacity(episodes.len());
    for ep in episodes {
        if deduped.last() != Some(&ep) {
            deduped.push(ep);
        }
    }
    deduped
}

fn deserialize_episodes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let episodes: Vec<String> = Vec::deserialize(deserializer)?;
    Ok(dedup_consecutive(episodes))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Medias {
    pub media: Vec<Media>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Media {
    pub name: String,
    pub lang: String,
    pub season: i8,
    pub media_type: String,
    #[serde(deserialize_with = "deserialize_episodes")]
    pub episodes: Vec<String>,
}

impl Media {
    pub fn new(name: &str, lang: &str, season: i8, media_type: &str, episodes: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            lang: lang.to_string(),
            season,
            media_type: media_type.to_string(),
            episodes: dedup_consecutive(episodes),
        }
    }
}

impl Display for Media {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.media_type == "film" {
            write!(f, "{}", "film".yellow())
        } else if self.media_type == "oav" {
            write!(f, "{}", "OAV".yellow())
        } else {
            write!(f, "saison {}", self.season.to_string().yellow())
        }
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
