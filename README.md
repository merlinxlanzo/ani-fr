## Table of Contents

- [Install](#install)
  - [From Source](#build)
- [Dependencies](#dependencies)
- [Setup](#setup)
- [Thanks](#thanks)

## Dependencies

- [yt-dlp](https://github.com/yt-dlp/yt-dlp)
- [mpv](https://mpv.io/)

## Setup

### 1. Install mpv

**Windows:**
```bash
winget install mpv-player.mpv
```

**Linux:**
```bash
sudo apt install mpv
```

**macOS:**
```bash
brew install mpv
```

### 2. Install yt-dlp

mpv needs yt-dlp to stream videos. Install it with pip:
```bash
pip install yt-dlp
```

Or with winget (Windows):
```bash
winget install yt-dlp
```

Or with your package manager (Linux):
```bash
sudo apt install yt-dlp
```

### 3. Make sure both are in your PATH

Run these commands to verify:
```bash
mpv --version
yt-dlp --version
```

If either command is not found, add its install location to your system PATH.

## Installation

<details>
  <summary>Cargo</summary>

  ```bash
  cargo install ani-fr
  ```
</details>

## Build
```bash
git clone https://github.com/merlinxlanzo/ani-fr.git
cd ani-fr
cargo build --release
```

## Usage

After installation, run:
```bash
ani-fr
```

## MyAnimeList Integration

ani-fr can connect to your [MyAnimeList](https://myanimelist.net/) account to automatically track your watch progress and skip OP/ED.

### Setting up MAL

1. Go to [https://myanimelist.net/apiconfig](https://myanimelist.net/apiconfig) and create a new API client
2. Fill in the form:
   - **App Name**: anything you want (e.g. `ani-fr`)
   - **App Type**: `web`
   - **App Redirect URL**: `http://localhost:7878/callback`
   - **Homepage URL**: can be left blank or any URL
3. After creating the client, note your **Client ID** and **Client Secret**
4. In ani-fr, select the MAL login option and enter your Client ID and Client Secret when prompted
5. A browser window will open — log in to MAL and authorize the app
6. Once authorized, you'll be redirected back and ani-fr will save your credentials

### Features

- **Automatic episode tracking** — your MAL list updates as you watch episodes
- **OP/ED auto-skip** — opening and ending sequences are automatically skipped using [aniskip](https://aniskip.com/) data linked via MAL IDs
- **Watch history** — your last 10 watched anime are saved locally for quick access with resume timestamps
- **Auto anime matching** — ani-fr searches MAL to match French titles to MAL entries, with manual fallback if needed

## Thanks

- [ani-cli](https://github.com/pystardust/ani-cli) for the inspiration
- [@S3nda](https://github.com/S3nda) for making the original scraper
- [B0SEmc/ani-dl](https://github.com/B0SEmc/ani-dl) for the original project
