# Privacy

**English** · [简体中文](PRIVACY.zh-CN.md)

The short version: MTC Calculator runs on your PC and looks only at the Roblox window. Nothing
about you leaves your PC unless you turn on statistics, or send a problem report yourself.

## What it looks at

- **The Roblox window.** It reads the picture of the game window while it runs and draws its own
  windows on top. Frames are processed in memory and not saved; the only exception is a problem
  report with *Screenshots* ticked (see below).
- **Player names.** The seat menu shows the names of your crew. The calculator reads them to find
  which seat is yours. They stay in memory and are never written to disk or sent anywhere.
- **Keyboard and mouse.** Only while Roblox is the window in front, and only the keys it uses: M, X,
  F, the number keys 1 to 6, and the mouse buttons and wheel. It never sees what you type in chat,
  never blocks a key and never sends any input to the game. Its log notes when those keys were
  pressed.

## Logs

Every run writes a log to `%LOCALAPPDATA%\MTC Calculator\logs`: what it recognised and how long
things took. The newest 10 logs are kept, at most 50 MB and 30 days; older ones are deleted. Open the folder from Settings, *Log files*, *Open folder*, or from the menu of the calculator's icon in the
notification area.

## Downloads

On first use, and later to check for updates, the calculator downloads:

- map pictures and heightmaps from `maps.artillery-calculator.com`, the map service of the original
  artillery calculator;
- weapon and ammunition data from `data.multicrew.wiki`, the MTC wiki's data service;
- terrain layers from `data.mtc-calc.dev`, run by this project.

These requests carry nothing about you or your games. Like any website, the services see your IP
address.

## Statistics (optional)

Statistics help us tune the calculator for guns, maps and PCs we have not tested ourselves.

- **Off until you agree.** At start the calculator asks whether to send them, until you choose
  *Send statistics* or tick *Don't ask again*. Change it any time in Settings, *DIAGNOSTICS*,
  *Share statistics*.
- **What is sent**, every 10 minutes and when you quit, to `stats.mtc-calc.dev`:
  - one record per shot: the map, weapon, shell and charge, the angles you fired at, the gun's
    and the impact's positions on the map, the predicted and the actual landing, and how sure the
    calculator was of each reading;
  - one record per run: the calculator and Windows versions, the graphics card's maker and whether it
    is dedicated or integrated, the number of CPU threads, the window size and display scaling, how well it found things on screen, its CPU
    and memory use, how often it could read the angles, the weapons and maps it recognised, weapon
    names it could not match (the game's text), counts of loads, shots and seat menu reads, your
    settings, and how many times it crashed.
- **What is never sent:** screenshots, any player's name, chat, key presses, file paths, your
  computer's or your account's name, or the time of day.
- **Records cannot be traced to you.** Each kind of record carries its own random number made at
  start, so shots cannot be matched to runs, runs cannot be matched to each other, and nothing
  connects them to you. The only date is the day.
- **On the server** (a Cloudflare Worker with a database, run by this project) each record is kept
  with its day only. Your IP address is used briefly to limit how often requests are accepted and is
  not stored, nor is the time a record arrived. Records older than 12 months are deleted
  automatically.
- **See it yourself.** Settings, *DIAGNOSTICS*, *View data* opens exactly what is waiting to be
  sent.
- Because the records carry no identity, we cannot find or delete one person's records. Turning
  sharing off stops new ones. If what is collected ever grows, the calculator asks you again.

## Problem reports

Settings, *Problem report*, *Create report* makes a folder on your PC with:

- `report.txt`: the calculator's version, the window size and scaling, the weapon, your settings,
  and room for you to describe the problem;
- `records.jsonl`: this run's statistics records, whether or not sharing is on;
- `log.txt`: the last 10 minutes of the log, with folder paths replaced by placeholders;
- only if *Screenshots* is ticked: pictures of the parts of the screen the calculator reads, with
  the names in the seat menu blacked out.

Nothing is sent. You decide whether and where to share the folder, for example in a GitHub issue.

## Questions

Open an issue on [GitHub](https://github.com/Akureyri-i/mtc-calc/issues).
