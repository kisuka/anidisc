# Contributing discs

## Layout

```
data/
  binetsu-shoukougun-1997/          one folder per series: title and year
    series.json
    dvd-r2-de/                      one folder per release: media, region, edition
      release.json
      abc-123-611c8af2.json         one file per disc: catalog number + start of contentHash
      2b19bf36.json                 ...or just the start of contentHash if it has no catalog number
```

Folder and file names are slugs: lowercase letters, digits and single hyphens. They only need to
be unique and readable; nothing depends on them. A disc is identified by its `contentHash`.

## Adding discs

```
anidisc contribute "D:\Rips\Some Show" -r --anidb 12345
```

writes everything `anidisc` can read from the discs, and with an AniDB id also the series'
titles and episodes. Instead of `--anidb`, a disc's series can be given by an
`anidisc.json` or an `[anidb-12345]` folder tag; see "Which series a disc belongs to" in the README. Check the result, then fill in what only you know: the release details, each
disc's `label`, and each title's `type` and `episodes`.

## series.json

```json
{
  "title": "Binetsu Shoukougun",
  "originalTitle": "微熱症候群",
  "year": 1997,
  "anidb": [1234],
  "aliases": ["Fever Syndrome"],
  "episodes": [
    { "number": 1, "title": "Welcome to the School Infirmary", "romajiTitle": "Hokenshitsu e Youkoso", "originalTitle": "保健室へようこそ" },
    { "number": 2, "title": "The Medical Checkup Has Just Started", "romajiTitle": "Tadaima Shinsatsuchuu", "originalTitle": "ただいま診察中" },
    { "number": "S1", "title": "Making of" },
    { "number": "T1", "title": "PV" }
  ]
}
```

- `title` is the romanized title used for folder names; `year` the year of the first release.
- `anidb` lists the AniDB anime ids the series covers, and is required. Usually one; several when the publisher
  numbers a sequel's discs as continuing volumes. An episode belonging to a later id says so
  with `"anidb": <id>`; without it, an episode belongs to the first id.
- `episodes` follows AniDB's numbering. Regular episodes are plain numbers (`1`); AniDB's other
  kinds are strings with its prefix: `"S1"` special, `"C1"` credits (opening/ending), `"T1"`
  trailer/promo, `"P1"` parody, `"O1"` other. `anidisc contribute --anidb` fills in all of them.
  The list is needed to map disc titles to episodes. Each episode's `title` is AniDB's English
  title, `romajiTitle` the romanized Japanese one and `originalTitle` the one in Japanese script;
  `title` falls back to the romaji or original title when AniDB has no English one, and a title
  equal to `title` is not repeated.

## release.json

```json
{
  "region": "R2-DE",
  "media": "DVD",
  "publisher": "Example Media",
  "releaseDate": "2003-05",
  "edition": "Limited Edition",
  "upc": "4012345678901"
}
```

- `region` is the tag used in file names: region code and country, such as `R2-JP`, `R1-US`, `A-JP`.
- `media` is `DVD`, `BD` or `UHD`.
- Everything else is optional. `name` gives the release a display name.

## Disc files

```json
{
  "contentHash": "611C8AF29F0C6E1EC6D8448DB14A33BE",
  "format": "dvd",
  "label": "Volume 1",
  "catalogNumber": "ABC-123",
  "volumeLabel": "BINETSU_1",
  "titles": [
    { "title": 1, "duration": "0:29:58", "chapters": 6, "type": "episode", "episodes": [1] },
    { "title": 2, "duration": "0:01:30", "chapters": 1, "type": "trailer", "episodes": ["T1"] },
    { "title": 3, "duration": "0:12:40", "chapters": 3, "type": "extra", "name": "Making of", "episodes": ["S1"] },
    { "title": 4, "duration": "0:00:12", "chapters": 1, "type": "menu" }
  ]
}
```

- `contentHash`, `format`, `volumeLabel`, `files` and the `title`/`playlist`, `duration`,
  `chapters` and `sizeBytes` of each title come from `anidisc contribute`. Don't edit them by hand.
- `files` is every file on the disc (`{ "path": "VIDEO_TS/VIDEO_TS.IFO", "size": 24576 }`), taken
  from a clean, unmodified copy. It is how `anidisc` tells a faithful copy from one with junk
  added or files removed, so contribute from the original disc or an untouched ISO of it, keeping
  files the pressing itself carries (`.DS_Store`, PC/Mac extras). `validate` checks that the
  listing produces the disc's `contentHash`.
- `pressing` tells re-pressings apart when they share a catalog number (`"2nd pressing, 2009"`).
  Each pressing with a different layout is its own disc file, in the same release folder.
- `label` is how the disc is named in its release: `Volume 1`, `Disc 2`, `Complete Edition`,
  `Specials`. Leave it out for a single-disc release.
- For each title, set `type` (`episode`, `extra`, `trailer`, `menu`, `other`) and, when it is one
  of the series' AniDB entries, the `episodes` it contains: `[1]`, `[1, 2]` for a title holding
  both, `["S1"]` for a special, `["T1"]` for a trailer. An episode may appear in several titles
  (a "play all" title and one per episode). Extras AniDB doesn't list just get a `name`. Titles you can't
  identify can keep no `type`; `validate` warns about them but accepts them.
- Blu-ray titles use `"playlist": "00800.mpls"` instead of `"title"`.

## Checking

```
anidisc validate
anidisc identify <your disc> --db .
```

`validate` must report 0 errors. Pull requests are validated automatically.
