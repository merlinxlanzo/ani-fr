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
from concurrent.futures import ThreadPoolExecutor, as_completed
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

LANG_MAP = {"vostfr": "vostfr", "vf": "vf"}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def normalize_name(name: str) -> str:
    return re.sub(r"[^a-z0-9 ]+", "", name.strip().lower()).strip()


def info(msg: str) -> None:
    print(f"{Fore.CYAN}[INFO]{Style.RESET_ALL} {msg}")


def success(msg: str) -> None:
    print(f"{Fore.GREEN}[OK]{Style.RESET_ALL} {msg}")


def warn(msg: str) -> None:
    print(f"{Fore.YELLOW}[WARN]{Style.RESET_ALL} {msg}")


def error(msg: str) -> None:
    print(f"{Fore.RED}[ERROR]{Style.RESET_ALL} {msg}")


def strip_js_comments(text: str) -> str:
    """Remove JS block comments (/* ... */) and line comments (// ...)."""
    # Block comments first (non-greedy, handles multiline)
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    # Line comments (but not URLs like https://)
    text = re.sub(r"(?<!:)//.*", "", text)
    return text


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

@dataclass
class SeasonInfo:
    """A discovered season/film/OAV from an anime page."""
    label: str          # e.g. "Saison 1", "Films", "OAV"
    path: str           # e.g. "saison1/vostfr"
    media_type: str     # "anime", "film", "oav"
    season: int         # season number (1 for films/oav)
    lang: str           # "vostfr" or "vf"


@dataclass
class AnimeEntry:
    name: str
    lang: str
    media_type: str
    season: int
    episodes: list[str] = field(default_factory=list)

    def key(self) -> tuple:
        return (normalize_name(self.name), self.lang, self.season, self.media_type)

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


# ---------------------------------------------------------------------------
# Network
# ---------------------------------------------------------------------------

session = requests.Session()
session.headers.update(HEADERS)


def fetch(url: str, delay: float) -> Optional[requests.Response]:
    time.sleep(delay)
    try:
        resp = session.get(url, timeout=30)
        resp.raise_for_status()
        return resp
    except requests.RequestException as exc:
        warn(f"Failed to fetch {url}: {exc}")
        return None


# ---------------------------------------------------------------------------
# Catalogue scraping
# ---------------------------------------------------------------------------

def scrape_catalogue_page(page: int, delay: float) -> list[CatalogueItem]:
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
            name=name, slug=slug, media_type=media_type, languages=languages,
        ))

    return items


def scrape_catalogue(max_pages: Optional[int], delay: float) -> list[CatalogueItem]:
    all_items: list[CatalogueItem] = []
    page = 1
    limit = max_pages or 999

    while page <= limit:
        info(f"Scraping catalogue page {page}...")
        items = scrape_catalogue_page(page, delay)
        if not items:
            break
        all_items.extend(items)
        page += 1

    return all_items


# ---------------------------------------------------------------------------
# Season discovery
# ---------------------------------------------------------------------------

def classify_path(path: str) -> Optional[tuple[str, int]]:
    """Classify a panneauAnime path into (media_type, season) or None to skip.

    Returns None for kai/hs editions (duplicate content).
    """
    # Skip kai editions (recuts)
    if re.match(r"kai\d*/", path):
        return None
    # Skip hors-série / filler-free versions
    if re.match(r"saison\d+hs/", path):
        return None

    # Film
    if path.startswith("film/"):
        return ("film", 1)
    # OAV
    if path.startswith("oav/"):
        return ("oav", 1)
    # Regular season
    m = re.match(r"saison(\d+)/", path)
    if m:
        return ("anime", int(m.group(1)))

    return None


def discover_seasons(slug: str, delay: float, include_kai: bool = False,
                     include_hs: bool = False) -> list[SeasonInfo]:
    """Fetch anime page and find available seasons/films/OAVs.

    Strips JS comments before parsing to avoid phantom seasons from
    commented-out panneauAnime() calls.
    """
    url = f"{BASE_URL}/catalogue/{slug}/"
    resp = fetch(url, delay)
    if resp is None:
        return [SeasonInfo("Saison 1", "saison1/vostfr", "anime", 1, "vostfr")]

    clean_text = strip_js_comments(resp.text)

    results: list[SeasonInfo] = []
    seen: set[tuple[str, str]] = set()  # (path_prefix, lang) dedup

    for m in re.finditer(
        r'panneauAnime\s*\(\s*"([^"]*)"\s*,\s*"([^"]+)"', clean_text
    ):
        label = m.group(1)
        full_path = m.group(2)  # e.g. "saison1/vostfr"

        # Extract lang from end of path
        parts = full_path.rsplit("/", 1)
        if len(parts) != 2:
            continue
        path_prefix, lang_str = parts[0] + "/", parts[1].lower()

        if lang_str not in LANG_MAP:
            continue
        lang = LANG_MAP[lang_str]

        # Classify
        if re.match(r"kai\d*/", path_prefix) and not include_kai:
            continue
        if re.match(r"saison\d+hs/", path_prefix) and not include_hs:
            continue

        classification = classify_path(path_prefix)
        if classification is None:
            continue

        media_type, season = classification
        dedup_key = (path_prefix, lang)
        if dedup_key in seen:
            continue
        seen.add(dedup_key)

        results.append(SeasonInfo(
            label=label,
            path=full_path,
            media_type=media_type,
            season=season,
            lang=lang,
        ))

    if not results:
        return [SeasonInfo("Saison 1", "saison1/vostfr", "anime", 1, "vostfr")]

    all_langs = set(LANG_MAP.values())
    extra: list[SeasonInfo] = []
    for si in results:
        for lang in all_langs:
            if lang == si.lang:
                continue
            path_prefix = si.path.rsplit("/", 1)[0]
            dedup_key = (path_prefix + "/", lang)
            if dedup_key in seen:
                continue
            probe_path = f"{path_prefix}/{lang}"
            probe_url = f"{BASE_URL}/catalogue/{slug}/{probe_path}/episodes.js"
            probe_resp = fetch(probe_url, 0.3)
            if probe_resp is not None and probe_resp.status_code == 200 and "eps" in probe_resp.text:
                seen.add(dedup_key)
                extra.append(SeasonInfo(
                    label=si.label,
                    path=probe_path,
                    media_type=si.media_type,
                    season=si.season,
                    lang=lang,
                ))

    results.extend(extra)
    return results


# ---------------------------------------------------------------------------
# Episode scraping
# ---------------------------------------------------------------------------

def _is_valid_url(u: str) -> bool:
    if u.endswith("=") or u.endswith("embed-.html") or u.endswith("/embed/"):
        return False
    return len(u) > 10


def fetch_episodes(slug: str, season_info: SeasonInfo, delay: float) -> list[str]:
    """Fetch episodes.js and extract episode URLs. Prefers sibnet.ru sources."""
    url = f"{BASE_URL}/catalogue/{slug}/{season_info.path}/episodes.js"
    resp = fetch(url, delay)
    if resp is None:
        return []

    js_text = resp.text
    all_sources: list[list[str]] = []
    sibnet_source: list[str] = []

    for m in re.finditer(r"(?:var\s+)?(eps\w+)\s*=\s*\[([^\]]*)\]", js_text):
        urls = re.findall(r"""['"]([^'"]+)['"]""", m.group(2))
        urls = [u for u in urls if _is_valid_url(u)]
        if not urls:
            continue
        all_sources.append(urls)
        if not sibnet_source and any("sibnet.ru" in u for u in urls):
            sibnet_source = [u for u in urls if "sibnet.ru" in u]

    if sibnet_source:
        return sibnet_source
    if all_sources:
        return max(all_sources, key=len)

    warn(f"No episode URLs found in {url}")
    return []


# ---------------------------------------------------------------------------
# Data management
# ---------------------------------------------------------------------------

def load_data(path: Path) -> dict:
    if path.exists():
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    return {"media": []}


def build_existing_keys(data: dict) -> dict[tuple, int]:
    """Map (normalized_name, lang, season, media_type) -> episode count."""
    keys: dict[tuple, int] = {}
    for entry in data.get("media", []):
        norm = normalize_name(entry["name"])
        mt = entry.get("media_type", "anime")
        key = (norm, entry["lang"], entry.get("season", 1), mt)
        keys[key] = len(entry.get("episodes", []))
    return keys


def save_data(data: dict, path: Path, dry_run: bool) -> None:
    if dry_run:
        info("Dry run: not saving changes.")
        return

    if path.exists():
        backup = path.with_name("anime_data_backup.json")
        shutil.copy2(path, backup)

    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
    success(f"Saved {path}")


def dedup_data(data: dict) -> None:
    """Deduplicate entries in-place, keeping the one with more episodes."""
    seen: dict[tuple, int] = {}
    deduped: list[dict] = []
    for entry in data["media"]:
        mt = entry.get("media_type", "anime")
        key = (normalize_name(entry["name"]), entry["lang"], entry.get("season", 1), mt)
        if key in seen:
            idx = seen[key]
            if len(entry.get("episodes", [])) > len(deduped[idx].get("episodes", [])):
                deduped[idx] = entry
        else:
            seen[key] = len(deduped)
            deduped.append(entry)
    data["media"] = deduped


def cleanup_data(data: dict, dry_run: bool) -> int:
    """Fix entries with duplicate episode blocks from the old scraper's blind merge.

    Detects when the second half of an episode list is a repeat of the first half
    (caused by merging identical phantom seasons) and trims to the unique portion.
    Returns the number of entries fixed.
    """
    fixed = 0
    for entry in data["media"]:
        eps = entry.get("episodes", [])
        n = len(eps)
        if n < 4:
            continue

        # Try splitting at every divisor to find repeated blocks
        best_unique = None
        for chunk_size in range(1, n // 2 + 1):
            if n % chunk_size != 0:
                continue
            chunk = eps[:chunk_size]
            # Check if all subsequent chunks are identical
            all_same = True
            for start in range(chunk_size, n, chunk_size):
                if eps[start:start + chunk_size] != chunk:
                    all_same = False
                    break
            if all_same and n // chunk_size >= 2:
                best_unique = chunk
                break  # smallest repeating unit found

        if best_unique and len(best_unique) < n:
            name = entry["name"]
            lang = entry["lang"]
            old_n = n
            new_n = len(best_unique)
            repeats = old_n // new_n
            if dry_run:
                info(f"  Would fix: {name} {lang} S{entry.get('season', 1)} "
                     f"({old_n} -> {new_n} eps, was repeated {repeats}x)")
            else:
                entry["episodes"] = best_unique
                success(f"  Fixed: {name} {lang} S{entry.get('season', 1)} "
                        f"({old_n} -> {new_n} eps, was repeated {repeats}x)")
            fixed += 1

    return fixed


# ---------------------------------------------------------------------------
# Main logic
# ---------------------------------------------------------------------------

def resolve_data_path(cli_path: Optional[str]) -> Path:
    if cli_path:
        return Path(cli_path)
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA", os.path.expanduser("~\\AppData\\Roaming"))
        return Path(appdata) / "B0SE" / "ani-fr" / "data" / "anime_data.json"
    data_dir = user_data_dir("ani-fr", "B0SE")
    return Path(data_dir) / "anime_data.json"


def process_anime(item: CatalogueItem, existing: dict[tuple, int],
                  args: argparse.Namespace,
                  seen_episodes: dict[str, str],
                  force_rescrape: bool = False) -> list[AnimeEntry]:
    """Process a single anime: discover seasons, fetch episodes. Returns new entries."""
    name_lower = item.name.strip().lower()
    entries: list[AnimeEntry] = []

    seasons = discover_seasons(
        item.slug, args.delay,
        include_kai=not args.skip_kai,
        include_hs=not args.skip_hs,
    )

    # Override media_type from catalogue if it's a film
    catalogue_is_film = "film" in item.media_type.lower()

    for si in seasons:
        media_type = si.media_type
        if catalogue_is_film and media_type == "anime":
            media_type = "film"

        key = (normalize_name(item.name), si.lang, si.season, media_type)

        if key in existing and not force_rescrape:
            if not args.check_updates:
                continue
            old_count = existing[key]
            episodes = fetch_episodes(item.slug, si, args.delay)
            if len(episodes) > old_count:
                info(f"  Updated: {name_lower} {si.label} {si.lang}: {old_count} -> {len(episodes)}")
                entries.append(AnimeEntry(
                    name=name_lower, lang=si.lang, media_type=media_type,
                    season=si.season, episodes=episodes,
                ))
            continue

        episodes = fetch_episodes(item.slug, si, args.delay)
        if not episodes:
            continue

        # Duplicate detection: if first episode URL matches another season, skip
        first_ep = episodes[0]
        dup_key = f"{normalize_name(item.name)}:{si.lang}:{first_ep}"
        if dup_key in seen_episodes:
            prev = seen_episodes[dup_key]
            warn(f"  Skipping {name_lower} {si.label} {si.lang} (duplicate of {prev})")
            continue
        seen_episodes[dup_key] = si.label

        entries.append(AnimeEntry(
            name=name_lower, lang=si.lang, media_type=media_type,
            season=si.season, episodes=episodes,
        ))

    return entries


def run(args: argparse.Namespace) -> None:
    colorama_init()

    data_path = resolve_data_path(args.data_path)
    info(f"Data file: {data_path}")

    data = load_data(data_path)
    existing = build_existing_keys(data)
    known_names = {normalize_name(e["name"]) for e in data.get("media", [])}
    info(f"Loaded {len(existing)} existing entries ({len(known_names)} unique animes)")

    # Cleanup mode: fix duplicate episode blocks from old scraper
    if args.cleanup:
        if args.name:
            search = normalize_name(args.name)
            target = [e for e in data.get("media", []) if search in normalize_name(e["name"])]
            if not target:
                error(f"No anime found matching '{args.name}'")
                return
            # Build a subset dict for cleanup, then apply fixes back
            subset = {"media": target}
            info(f"Running cleanup on {len(target)} entries matching '{args.name}'...")
            fixed = cleanup_data(subset, args.dry_run)
        else:
            info("Running cleanup on existing data...")
            fixed = cleanup_data(data, args.dry_run)
        if fixed:
            info(f"Fixed {fixed} entries with duplicate episode blocks")
            save_data(data, data_path, args.dry_run)
        else:
            info("No duplicate episode blocks found")
        return

    catalogue = scrape_catalogue(args.pages, args.delay)
    info(f"Found {len(catalogue)} item(s) in catalogue")

    if args.name:
        search = normalize_name(args.name)
        catalogue = [it for it in catalogue if search in normalize_name(it.name)]
        if not catalogue:
            error(f"No anime found matching '{args.name}'")
            return
        info(f"Matched {len(catalogue)}: {', '.join(it.name for it in catalogue)}")

        matched_norms = {normalize_name(it.name) for it in catalogue}
        old_count = len(data["media"])
        data["media"] = [e for e in data["media"]
                         if normalize_name(e["name"]) not in matched_norms]
        removed = old_count - len(data["media"])
        if removed:
            info(f"Removed {removed} old entries for re-scrape")
            existing = build_existing_keys(data)

    # Skip already-scraped unless checking updates or targeting by name
    if not args.check_updates and not args.name:
        all_langs = set(LANG_MAP.values())
        def _fully_scraped(name_norm: str) -> bool:
            langs_found = {e["lang"] for e in data.get("media", [])
                           if normalize_name(e["name"]) == name_norm}
            return langs_found >= all_langs
        before = len(catalogue)
        catalogue = [it for it in catalogue if not _fully_scraped(normalize_name(it.name))]
        skipped = before - len(catalogue)
        if skipped:
            info(f"Skipped {skipped} fully scraped, {len(catalogue)} remaining")

    stats = {"new_animes": 0, "new_seasons": 0, "new_episodes": 0,
             "updated": 0, "new_films": 0, "new_oavs": 0}
    seen_names: set[str] = set()
    seen_episodes: dict[str, str] = {}  # "name:lang:first_url" -> label
    new_entries: list[dict] = []
    last_save_count = 0

    def _save_progress():
        nonlocal last_save_count
        if len(new_entries) <= last_save_count:
            return
        data["media"].extend(new_entries[last_save_count:])
        last_save_count = len(new_entries)
        dedup_data(data)
        save_data(data, data_path, args.dry_run)

    try:
        for i, item in enumerate(catalogue):
            info(f"[{i + 1}/{len(catalogue)}] Processing: {item.name}")

            if len(new_entries) - last_save_count >= 50:
                _save_progress()

            entries = process_anime(item, existing, args, seen_episodes,
                                    force_rescrape=bool(args.name))

            for entry in entries:
                d = entry.to_dict()
                new_entries.append(d)
                existing[entry.key()] = len(entry.episodes)

                name_lower = entry.name
                if name_lower not in seen_names:
                    if entry.media_type == "film":
                        stats["new_films"] += 1
                    elif entry.media_type == "oav":
                        stats["new_oavs"] += 1
                    else:
                        stats["new_animes"] += 1
                    seen_names.add(name_lower)

                if entry.media_type == "film":
                    stats["new_films"] += 0  # counted above
                elif entry.media_type == "oav":
                    stats["new_oavs"] += 0
                else:
                    stats["new_seasons"] += 1

                stats["new_episodes"] += len(entry.episodes)
                success(f"  {entry.media_type.upper()}: {name_lower} S{entry.season} {entry.lang} - {len(entry.episodes)} ep")

    except KeyboardInterrupt:
        warn("Interrupted! Saving progress...")

    _save_progress()

    # Summary
    print()
    print(f"{Fore.MAGENTA}{'=' * 50}{Style.RESET_ALL}")
    print(f"{Fore.MAGENTA}Summary{Style.RESET_ALL}")
    print(f"{Fore.MAGENTA}{'=' * 50}{Style.RESET_ALL}")
    print(f"  New animes:   {Fore.GREEN}{stats['new_animes']}{Style.RESET_ALL}")
    print(f"  New films:    {Fore.GREEN}{stats['new_films']}{Style.RESET_ALL}")
    print(f"  New OAVs:     {Fore.GREEN}{stats['new_oavs']}{Style.RESET_ALL}")
    print(f"  New seasons:  {Fore.GREEN}{stats['new_seasons']}{Style.RESET_ALL}")
    print(f"  New episodes: {Fore.GREEN}{stats['new_episodes']}{Style.RESET_ALL}")
    if args.check_updates:
        print(f"  Updated:      {Fore.YELLOW}{stats['updated']}{Style.RESET_ALL}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Scrape anime-sama.to and update anime_data.json"
    )
    parser.add_argument("--data-path", type=str, default=None,
                        help="Path to anime_data.json (default: platformdirs location)")
    parser.add_argument("--check-updates", action="store_true",
                        help="Re-check existing entries for new episodes")
    parser.add_argument("--dry-run", action="store_true",
                        help="Show what would be added without modifying the file")
    parser.add_argument("--delay", type=float, default=1.0,
                        help="Delay between requests in seconds (default: 1.0)")
    parser.add_argument("--pages", type=int, default=None,
                        help="Limit number of catalogue pages to scrape (default: all)")
    parser.add_argument("--name", type=str, default=None,
                        help="Scrape a specific anime by name")
    parser.add_argument("--skip-kai", action="store_true", default=True,
                        help="Skip Kai editions (default: true)")
    parser.add_argument("--no-skip-kai", action="store_false", dest="skip_kai",
                        help="Include Kai editions")
    parser.add_argument("--skip-hs", action="store_true", default=True,
                        help="Skip hors-série/filler-free versions (default: true)")
    parser.add_argument("--no-skip-hs", action="store_false", dest="skip_hs",
                        help="Include hors-série/filler-free versions")
    parser.add_argument("--cleanup", action="store_true",
                        help="Fix duplicate episode blocks from old scraper's merge")

    args = parser.parse_args()
    run(args)


if __name__ == "__main__":
    main()
