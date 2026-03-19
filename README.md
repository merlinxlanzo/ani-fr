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

## Thanks

- [ani-cli](https://github.com/pystardust/ani-cli) for the inspiration
- [@S3nda](https://github.com/S3nda) for making the original scraper
- [B0SEmc/ani-dl](https://github.com/B0SEmc/ani-dl) for the original project
