# AniDiscDB

An open catalog of information about anime DVD and Blu-ray releases: which series, release and
volume each disc is, and which episodes and extras are on it. Discs are recognized by a
fingerprint of their file sizes. The command-line tool `anidisc` fingerprints the discs and disc
images you already own, looks them up, and names and files them for you.

AniDiscDB holds only descriptive data. It contains no video, disc images or download links.

- **No server.** The data is plain JSON files in this repository. A build step turns them into
  a static site plus a lookup API (one small JSON file per disc), hosted on GitHub Pages at
  [anidisc.com](https://anidisc.com).
- **Discs are identified by content hash**: an MD5 of the sizes of the disc's video files
  (DVD: every `VIDEO_TS/*.VOB/IFO/BUP`; Blu-ray: every `BDMV/STREAM/*.m2ts`), taken in file-name
  order as 64-bit little-endian numbers. No video data is read, and the value is identical for a
  physical disc, an ISO and a `VIDEO_TS`/`BDMV` folder ripped from it.
- **Altered copies are caught.** Each disc also records every file on the original pressing.
  A copy with junk added or files removed is still identified, but reported as altered with the
  exact files that differ, and `organize` leaves it alone unless told otherwise.
- **Each disc maps its titles** (DVD titles or Blu-ray playlists) to episodes and extras.

## Download

`anidisc` is a single small executable (about 1–2 MB) with nothing to install. Get the one for your system from the
[latest release](../../releases/latest):

| System | File |
| --- | --- |
| Windows (most PCs) | `anidisc-win-x64.zip` |
| Windows on ARM | `anidisc-win-arm64.zip` |
| macOS, Apple Silicon (M1 and later) | `anidisc-osx-arm64.tar.gz` |
| macOS, Intel | `anidisc-osx-x64.tar.gz` |
| Linux | `anidisc-linux-x64.tar.gz` |
| Linux on ARM (Raspberry Pi 4/5, etc.) | `anidisc-linux-arm64.tar.gz` |

Unpack it and run it from a terminal (`anidisc.exe identify D:\ISOs` on Windows,
`./anidisc identify ~/ISOs` elsewhere). `SHA256SUMS.txt` in the release lists checksums.

The builds are not code-signed, so the first run shows a warning:
- **Windows** SmartScreen: *More info* → *Run anyway*.
- **macOS** Gatekeeper: run `xattr -d com.apple.quarantine anidisc` once in the folder you
  unpacked it to.

## The CLI

```
anidisc identify <paths> [-r]                 hash discs and look them up
anidisc organize <paths> -d <folder> [-r]     plan moves into Series (Year)/Label [DVD] [Region] [Catalog].iso
anidisc organize ... --apply                  ...and do them (writes an undo log)
anidisc undo <log>                            put everything back
anidisc contribute <paths> [-r]               write pre-filled data files for unknown discs
anidisc validate                              check data/
anidisc build                                 build the static site into _site/
```

Inputs can be `.iso` files (read directly, no mounting), disc folders containing `VIDEO_TS` or
`BDMV`, or folders to search (`-r` for subfolders).

`contribute`, `validate` and `build` work on a checkout's `data/` folder: the checkout containing
the current folder, otherwise `--data <folder>` or the `ANIDISC_DATA` environment variable. Run
anywhere else, they stop rather than create a `data` folder.

`identify` and `organize` look discs up on [anidisc.com](https://anidisc.com) by default. Only
each disc's hash is sent, one small request per disc, so nothing else needs downloading. `--db`
(or the `ANIDISC_DB` environment variable) looks somewhere else:

| `--db` value | Looks in |
| --- | --- |
| `https://anidisc.com` (default) | the published site |
| a built `_site` folder | `api/hash/*.json` |
| a checkout of this repository | the `data/` files directly (no build needed) |

### Which series a disc belongs to

`identify` and `organize` don't need to know: they look discs up by hash. `contribute` does, and
every series is identified by its AniDB id, given explicitly; folder names are never guessed from.
It uses, in order:

1. `--anidb <id>` for every disc in the run;
2. an `anidisc.json` containing `{ "anidb": 12345 }` in the folder holding the disc, or a parent;
3. an `[anidb-12345]` or `{anidb-12345}` tag in the disc's or a folder's name (the Plex HAMA convention).

Discs without an id are skipped and listed, with these options.

### AniDB records

Given an AniDB id, `contribute` fills in the series' titles, year and episode list from that
anime's AniDB record. Nothing needs setting up: `anidisc` downloads each record it needs once and
keeps it in its own cache (`anidb/` in the [app data folder](#where-files-go), not in this
repository), so later runs, and other discs of the same series, use the saved copy. Records are
looked for in this order:

1. the cache;
2. folders of `<id>.xml` files listed in `ANIDISC_ANIDB_XML` (`;`-separated), if you already
   have AniDB records saved elsewhere;
3. by default, [Kometa's AniDB cache](https://utilities.kometa.wiki/anidb-service), a public mirror of AniDB
   records that needs no account;
4. or AniDB itself, with your own AniDB API client (below).

`--offline` uses only the first two and downloads nothing.

**Using your own AniDB client.** Register an HTTP API client on your AniDB account (it gets a
name, lowercase letters and digits such as `myanidisc`, and a version number such as `1`; see
AniDB's HTTP API documentation on its wiki) and pass it
as `--anidb-client myanidisc/1`, or set `ANIDISC_ANIDB_CLIENT=myanidisc/1`. Records then come
from AniDB's HTTP API instead of Kometa. `anidisc` keeps well inside AniDB's limits on its own:

- it waits at least 4 seconds between requests (AniDB asks for 2), across separate runs too;
- it never requests a record it has already saved;
- if AniDB answers with an error, it makes no more requests that run, and if the error is a
  ban, none for 24 hours.

### Organizing

```
anidisc organize "D:\Unsorted" -r -d "D:\Anime"
```

prints the plan and changes nothing. Add `--apply` to move (or `--copy`). Discs that are not in
the database, altered copies (`--include-altered` to move them anyway), conflicts (target exists, or two copies of one disc), and unreadable images are
left alone and listed. Each applied run writes a log under
`logs/` in the [app data folder](#where-files-go) (or `--log`), which `anidisc undo <log>` reverses.

The target path comes from `--template`; the default is

```
{series} ({year})/{label} [{media}] [{region}] [{catalog}]
```

which gives e.g. `Binetsu Shoukougun (1997)/Volume 1 [DVD] [R2-DE] [ABC-123].iso`. Empty `[]`
and `()` are dropped, so a single-disc release without a label becomes `[DVD] [R2-JP] [ABC-123].iso`.
Other placeholders: `{original} {edition} {pressing} {publisher} {release} {volumelabel} {anidb} {hash}`.
If you keep two pressings of one disc, add `{pressing}` so they get different names.

### Where files go

`anidisc` creates and manages a per-user app data folder for the AniDB records it has downloaded
(`anidb/`) and organize logs (`logs/`). `anidb/` is safe to delete (records are downloaded again
when needed); `logs/` holds what `anidisc undo` needs.

| System | Folder |
| --- | --- |
| Windows | `%LOCALAPPDATA%\anidisc` |
| macOS | `~/Library/Application Support/anidisc` |
| Linux | `$XDG_DATA_HOME/anidisc`, or `~/.local/share/anidisc` |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). In short:

```
anidisc contribute "D:\Anime\Some Show (2003)" -r --db https://anidisc.com
```

writes `data/<series>/<release>/<disc>.json` for each disc the database does not have, with the
hash, titles, lengths and chapter counts read from the disc and the rest guessed from the file
names. Fill in the `TODO`s and the episode numbers, run `anidisc validate`, and open a pull
request.
