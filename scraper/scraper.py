#!/usr/bin/env python3
"""
Scraper for anime-sama.to catalogue.
Populates anime_data.json used by the ani-fr Rust CLI.
"""

import argparse
import json
import os
import re
import shutil
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

import requests
from bs4 import BeautifulSoup
from colorama import Fore, Style, init as colorama_init
from platformdirs import user_data_dir

BASE_URL = "https://anime-sama.to"
CATALOGUE_URL = f"{BASE_URL}/catalogue/"
USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
    "AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/125.0.0.0 Safari/537.36"
)
HEADERS = {"User-Agent": USER_AGENT}

# Languages we care about, mapped from flag classes or text to our keys
LANG_MAP = {
    "vostfr": "vostfr",
    "vf": "vf",
}


@dataclass
class AnimeEntry:
    name: str
    lang: str
    media_type: str
    season: int
    episodes: list[str] = field(default_factory=list)

    def key(self) -> tuple:
        return (self.name, self.lang, self.season)

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "lang": self.lang,
            "media_type": self.media_type,
            "season": self.season,
            "episodes": self.episodes,
        }


@dataclass
class CatalogueItem:
    name: str
    slug: str
    media_type: str
    languages: list[str]


def info(msg: str) -> None:
    print(f"{Fore.CYAN}[INFO]{Style.RESET_ALL} {msg}")


def success(msg: str) -> None:
    print(f"{Fore.GREEN}[OK]{Style.RESET_ALL} {msg}")


def warn(msg: str) -> None:
    print(f"{Fore.YELLOW}[WARN]{Style.RESET_ALL} {msg}")


def error(msg: str) -> None:
    print(f"{Fore.RED}[ERROR]{Style.RESET_ALL} {msg}")


# ---------------------------------------------------------------------------
# Network helpers
# ---------------------------------------------------------------------------

def fetch(url: str, delay: float) -> Optional[requests.Response]:
    """GET a URL with delay, returns Response or None on failure."""
    time.sleep(delay)
    try:
        resp = requests.get(url, headers=HEADERS, timeout=30)
        resp.raise_for_status()
        return resp
    except requests.RequestException as exc:
        warn(f"Failed to fetch {url}: {exc}")
        return None


# ---------------------------------------------------------------------------
# Catalogue scraping
# ---------------------------------------------------------------------------

def scrape_catalogue_page(page: int, delay: float) -> list[CatalogueItem]:
    """Scrape a single catalogue page and return items found."""
    url = f"{CATALOGUE_URL}?page={page}"
    resp = fetch(url, delay)
    if resp is None:
        return []

    soup = BeautifulSoup(resp.text, "html.parser")
    items: list[CatalogueItem] = []

    for card in soup.select("div.catalog-card"):
        link = card.select_one("a[href*='/catalogue/']")
        if not link:
            continue
        href = link.get("href", "")
        match = re.search(r"/catalogue/([^/?#]+)", href)
        if not match:
            continue
        slug = match.group(1)

        h2 = card.select_one("h2")
        if h2 is None:
            continue
        name = h2.get_text(strip=True)

        # Parse info-rows: each has span.info-label + p.info-value
        media_type = ""
        languages: list[str] = []
        for row in card.select("div.info-row"):
            label_el = row.select_one("span.info-label")
            value_el = row.select_one("p.info-value")
            if not label_el or not value_el:
                continue
            label = label_el.get_text(strip=True).lower()
            value = value_el.get_text(strip=True).lower()

            if label == "types":
                media_type = value
            elif label == "langues":
                for key in LANG_MAP:
                    if key in value:
                        languages.append(LANG_MAP[key])

        if not languages:
            languages = ["vostfr"]

        items.append(CatalogueItem(
            name=name,
            slug=slug,
            media_type=media_type,
            languages=languages,
        ))

    return items


def scrape_catalogue(max_pages: Optional[int], delay: float, include_films: bool) -> list[CatalogueItem]:
    """Scrape the full catalogue."""
    all_items: list[CatalogueItem] = []
    page = 1
    limit = max_pages or 999

    while page <= limit:
        info(f"Scraping catalogue page {page}...")
        items = scrape_catalogue_page(page, delay)
        if not items:
            # No more items means we've gone past the last page
            if page > 1:
                break
            warn(f"No items found on page {page}")
            break

        for item in items:
            type_lower = item.media_type.lower()
            if "anime" in type_lower:
                all_items.append(item)
            elif include_films and "film" in type_lower:
                item.media_type = "film"
                all_items.append(item)

        page += 1

    return all_items


# ---------------------------------------------------------------------------
# Season discovery
# ---------------------------------------------------------------------------

def discover_seasons(slug: str, delay: float) -> list[tuple[int, str]]:
    """Fetch an anime page and find available (season, lang) pairs.

    The page contains JS calls like: panneauAnime("Saison 1", "saison1/vostfr");
    """
    url = f"{BASE_URL}/catalogue/{slug}/"
    resp = fetch(url, delay)
    if resp is None:
        return [(1, "vostfr")]

    text = resp.text
    pairs: list[tuple[int, str]] = []
    # Match panneauAnime("...", "saison1/vostfr")
    for m in re.finditer(r'panneauAnime\s*\(\s*"[^"]*"\s*,\s*"saison(\d+)/([^"]+)"', text):
        season = int(m.group(1))
        lang = m.group(2).lower()
        if lang in LANG_MAP:
            pairs.append((season, LANG_MAP[lang]))

    if not pairs:
        # Fallback: look for raw saison links
        for m in re.finditer(r"saison(\d+)/(vostfr|vf)", text):
            pairs.append((int(m.group(1)), m.group(2)))

    if not pairs:
        return [(1, "vostfr")]

    return sorted(set(pairs))


# ---------------------------------------------------------------------------
# Episode scraping
# ---------------------------------------------------------------------------

def fetch_episodes(slug: str, season: int, lang: str, delay: float) -> list[str]:
    """Fetch episodes.js and extract episode URLs.

    Prefers sibnet.ru URLs from any eps variable. Falls back to the first
    available source if sibnet is not present.
    """
    url = f"{BASE_URL}/catalogue/{slug}/saison{season}/{lang}/episodes.js"
    resp = fetch(url, delay)
    if resp is None:
        return []

    js_text = resp.text

    # Parse all epsN variables
    all_sources: list[list[str]] = []
    sibnet_source: list[str] = []

    for m in re.finditer(r"(?:var\s+)?(eps\w+)\s*=\s*\[([^\]]*)\]", js_text):
        urls = re.findall(r"""['"]([^'"]+)['"]""", m.group(2))
        if not urls:
            continue
        all_sources.append(urls)
        if not sibnet_source and any("sibnet.ru" in u for u in urls):
            sibnet_source = [u for u in urls if "sibnet.ru" in u]

    if sibnet_source:
        return sibnet_source
    if all_sources:
        return all_sources[0]

    warn(f"No episode URLs found in {url}")
    return []


# ---------------------------------------------------------------------------
# Data management
# ---------------------------------------------------------------------------

def load_data(path: Path) -> dict:
    """Load existing anime_data.json or return empty structure."""
    if path.exists():
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    return {"media": []}


def build_existing_keys(data: dict) -> dict[tuple, int]:
    """Build a map of (name, lang, season) -> index and episode count."""
    keys: dict[tuple, int] = {}
    for i, entry in enumerate(data.get("media", [])):
        key = (entry["name"], entry["lang"], entry.get("season", 1))
        keys[key] = len(entry.get("episodes", []))
    return keys


def save_data(data: dict, path: Path, dry_run: bool) -> None:
    if dry_run:
        info("Dry run: not saving changes.")
        return

    # Backup
    if path.exists():
        backup = path.with_name("anime_data_backup.json")
        shutil.copy2(path, backup)
        info(f"Backup saved to {backup}")

    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
    success(f"Saved {path}")


# ---------------------------------------------------------------------------
# Main logic
# ---------------------------------------------------------------------------

def resolve_data_path(cli_path: Optional[str]) -> Path:
    if cli_path:
        return Path(cli_path)
    data_dir = user_data_dir("ani-fr", "B0SE")
    return Path(data_dir) / "anime_data.json"


def run(args: argparse.Namespace) -> None:
    colorama_init()

    data_path = resolve_data_path(args.data_path)
    info(f"Data file: {data_path}")

    data = load_data(data_path)
    existing = build_existing_keys(data)
    info(f"Loaded {len(existing)} existing entries")

    # Scrape catalogue
    catalogue = scrape_catalogue(args.pages, args.delay, args.include_films)
    info(f"Found {len(catalogue)} anime(s) in catalogue")

    stats = {"new_animes": 0, "new_seasons": 0, "new_episodes": 0, "updated": 0}
    seen_names: set[str] = set()
    new_entries: list[dict] = []

    for i, item in enumerate(catalogue):
        name_lower = item.name.strip().lower()
        media_type = "film" if "film" in item.media_type.lower() else "anime"

        info(f"[{i + 1}/{len(catalogue)}] Processing: {item.name}")

        season_lang_pairs = discover_seasons(item.slug, args.delay)

        for season, lang in season_lang_pairs:
            key = (name_lower, lang, season)

            if key in existing:
                if not args.check_updates:
                    continue
                old_count = existing[key]
                episodes = fetch_episodes(item.slug, season, lang, args.delay)
                if len(episodes) > old_count:
                    info(
                        f"  Updated: {name_lower} S{season} {lang}: "
                        f"{old_count} -> {len(episodes)} episodes"
                    )
                    if not args.dry_run:
                        for entry in data["media"]:
                            if (entry["name"], entry["lang"], entry.get("season", 1)) == key:
                                entry["episodes"] = episodes
                                break
                    stats["updated"] += 1
                    stats["new_episodes"] += len(episodes) - old_count
                continue

            # New entry
            episodes = fetch_episodes(item.slug, season, lang, args.delay)
            if not episodes:
                warn(f"  No episodes found for {name_lower} S{season} {lang}, skipping")
                continue

            entry = AnimeEntry(
                name=name_lower,
                lang=lang,
                media_type=media_type,
                season=season,
                episodes=episodes,
            )

            new_entries.append(entry.to_dict())
            existing[key] = len(episodes)

            if name_lower not in seen_names:
                stats["new_animes"] += 1
                seen_names.add(name_lower)

            stats["new_seasons"] += 1
            stats["new_episodes"] += len(episodes)
            success(
                f"  New: {name_lower} S{season} {lang} - {len(episodes)} episodes"
            )

    # Merge and save
    if new_entries or stats["updated"]:
        data["media"].extend(new_entries)
        save_data(data, data_path, args.dry_run)

    # Summary
    print()
    print(f"{Fore.MAGENTA}{'=' * 50}{Style.RESET_ALL}")
    print(f"{Fore.MAGENTA}Summary{Style.RESET_ALL}")
    print(f"{Fore.MAGENTA}{'=' * 50}{Style.RESET_ALL}")
    print(f"  New animes:   {Fore.GREEN}{stats['new_animes']}{Style.RESET_ALL}")
    print(f"  New seasons:  {Fore.GREEN}{stats['new_seasons']}{Style.RESET_ALL}")
    print(f"  New episodes: {Fore.GREEN}{stats['new_episodes']}{Style.RESET_ALL}")
    if args.check_updates:
        print(f"  Updated:      {Fore.YELLOW}{stats['updated']}{Style.RESET_ALL}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Scrape anime-sama.to and update anime_data.json"
    )
    parser.add_argument(
        "--data-path",
        type=str,
        default=None,
        help="Path to anime_data.json (default: platformdirs location)",
    )
    parser.add_argument(
        "--check-updates",
        action="store_true",
        help="Re-check existing entries for new episodes",
    )
    parser.add_argument(
        "--include-films",
        action="store_true",
        help="Include films in addition to anime series",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be added without modifying the file",
    )
    parser.add_argument(
        "--delay",
        type=float,
        default=1.0,
        help="Delay between requests in seconds (default: 1.0)",
    )
    parser.add_argument(
        "--pages",
        type=int,
        default=None,
        help="Limit number of catalogue pages to scrape (default: all)",
    )

    args = parser.parse_args()
    run(args)


if __name__ == "__main__":
    main()
