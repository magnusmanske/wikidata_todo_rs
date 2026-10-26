# wikidata_todo_rs

The web service and background jobs of [wikidata-todo](https://wikidata-todo.toolforge.org/),
a [Toolforge](https://toolforge.org/) tool by Magnus Manske: a collection of
tools for finding and doing work on Wikidata, the APIs of its apps (Duplicity,
Relator, …), the Distributed Game APIs, and the OAuth editing
service (Widar) they share.

This replaces the PHP version ([wikidata-todo](https://codeberg.org/magnusmanske/wikidata-todo)).
The apps with their own (Vue) interface now live here, in `public/`: Duplicity,
Relator, Relator2, Graveyard shift and Papertrail. The PHP checkout
stays in use for the other static files in `public_html` (pages, scripts, data;
served from there by this service), the shared `resources/`, and the
Wikispecies bot.

## Tools

Each tool lives at `/<name>/`; its old address, `/<name>.php`, redirects there
(permanently, keeping the query string and, for POST, the body). Tools that
other tools call with `format=json` and such (`get_item_names`,
`pagepile2kml`, `related_properties`, `resolver`, `sparql_rc`, `user_edits`,
`wikidata_cache`) also answer at their old address.

| Path | What it does |
|---|---|
| `/` | Things to do (items lacking gender, author, administrative unit; per country), all tools |
| `/add_name_labels/` | QuickStatements to copy labels of a SPARQL query's items into another language |
| `/artwork_images/` | Artworks without image, and Commons files of the same creators without item |
| `/awarder/` | Awards a person received, or the recipients of an award, as QuickStatements |
| `/beacon/` | BEACON files from external ID properties |
| `/best_cats/` | Categories of a wiki's articles whose items lack "instance of" (a sample) |
| `/bioacoustica/` | BioAcoustica recordings on Commons, matched to taxon items |
| `/cloudy_concept/` | Items linked from an item's articles that lack a label in a language |
| `/country_stats/` | Historical per-country statistics |
| `/creator_from_wikidata/` | A Commons Creator template for an item |
| `/dupe_finder/` | Possible duplicates of items unique to a wiki |
| `/get_item_names/` | Labels of items (HTML, JSON, TSV, wiki list) |
| `/images_in_categories_used_on_commons/` | How many files of a Commons category tree Wikidata uses |
| `/images_near_items/` | Commons files taken near items without image |
| `/important_blank_items/` | Items without statements, most sitelinks first |
| `/label_no_instance/` | Items with a label pattern but no "instance of" |
| `/linked_items/` | The items of the pages a page links to |
| `/missing_wp_animal_audio/` | Animal sounds on Wikidata but not on a Wikipedia |
| `/multibeacon/` | A table of property values for a list of IDs |
| `/new_items/` | The newest items |
| `/pagepile2kml/` | KML of a PagePile's items |
| `/personendaten/` | Search for people by name, dates, places (German) |
| `/project_stats/` | Sitelinks per project, labels per language |
| `/property_string_matcher/` | Items for string values of a property |
| `/relabel/` | Items whose label or alias is one of some strings |
| `/related_properties/` | Properties used by the items of a query |
| `/resolver/` | Redirects to the item (or its article, or a tool) with a property value |
| `/reverse_tree/` | All categories above a page |
| `/sourcerer_game/` | Pick sources for statements |
| `/sparql_rc/` | Recent changes of the items of a SPARQL query |
| `/stats/` | Statement statistics from the dumps (until 2021) |
| `/subclass_dates/` | Birth (or other date) histograms of a class tree |
| `/test_descriptions/` | Automatic vs. manual descriptions of random items |
| `/translate_items_with_property/` | Labels and aliases of items with a property, as TSV |
| `/unused_images_of_people/` | Commons images of people that no wiki uses |
| `/user_edits/` | A user's edits to some items, by summary pattern |
| `/wd_commons/` | Commons files linking to items |
| `/wd_edit_stats/` | Edits per month and user to some items |
| `/wdq2graph/` | Graph (DOT, TGF) of an item tree |
| `/wdq_image_feed/` | RSS feed of a query's images |
| `/wikidata_cache/` | Items with all of some properties, from the tool's cache |

APIs, at their old addresses: `api.php` (Widar; also `/widar/` and
`/widar/index.php`), `canned_queries.php`, `commons2json.php`,
`coords2json.php`, `creator.php`, `description_api.php`, `duplicity.php` (into
the app), `duplicity/api.php`, `duplicity_game_api.php`,
`findagrave_recent_celeb_deaths_xml.php`, `gnd.php`, `last_changed_items.php`,
`merge_candidates_by_shared_identifier.php`, `odnb_redirect.php`,
`pasleim_projectmerge_game_api.php`, `relator/api.php`, `relator2/api.php`,
`sourcery_proxy.php`, `suggestor.php`, `wp_no_image/api.php`.

WikiFlix is a tool of its own now: `/wikiflix/…` redirects to the same path on
https://wikiflix.toolforge.org/.

Where a parameter took WDQ (the long gone Wikidata Query), it now takes SPARQL,
or WDQ for the common forms (`src/wdq.rs` translates them).

The apps' files in `public/` are built into the binary (`src/web/apps.rs`).
Anything else is a static file from the PHP checkout's `public_html`, if its
type is on an allowlist (`src/web/static_files.rs`); retired pages redirect or
answer 410 (`src/web/legacy.rs`).

Uses of the tools are counted per day in the usage log of magnustools
(tool `wikidata-todo`, the tool's name as method).

## Editing (Widar)

Edits on Wikidata go through `/api.php` (Widar), with the user's OAuth login
in an encrypted cookie shared by all apps on the site. Editing actions are
only accepted from pages of this site.

## Code

- `src/tools/`: one module per tool; `templates/tools/` their pages.
- `public/`: the apps' HTML, JavaScript and CSS (their APIs are in `src/tools/`).
- `src/jobs/`: background jobs.
- `src/web/`: routing, errors, parameters, static files. `templates/base.html`
  and `assets/style.css` give all pages one look.
- `src/wiki.rs`, `src/mediawiki.rs`, `src/wikidata.rs`, `src/replicas.rs`,
  `src/terms.rs` (labels from the term store), `src/sparql.rs`,
  `src/pagepile.rs`, `src/tool_db.rs`, `src/oauth.rs`: wikis, their API and
  databases, the query service, PagePile, the tool's databases, OAuth.

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## Run locally

```bash
cp config.json.template config.json   # point public_html at a wikidata-todo checkout
cargo run -- --port 8000
./smoke_test.sh http://localhost:8000
```

Database tools need a `replica.my.cnf` next to `config.json` (or wherever
`replica_credentials` points), with `local='true'` and a `[tunnels]` section
giving each tunnel a local port (see the `toolforge` crate), including
`termstore.wikidatawiki` and `toolsdb`. Open the tunnels with:

```bash
TOOL_DATA_DIR=$PWD toolforge-tunnel
```

The tool's ToolsDB databases are readable by other tools' credentials, too:
set `"tool_db_owner": "s51211"` to use them (read-only).

## Jobs

The same binary runs the background jobs (`wikidata_todo_rs --help` lists
them; each has `--dry-run`), and `jobs.yaml` schedules them on Toolforge.

## Deploy

On Toolforge, as the `wikidata-todo` tool:

```bash
./build.sh                                              # build the image from GitHub
toolforge webservice buildservice restart --mount=all   # or start, the first time
toolforge jobs load jobs.yaml
```

The service reads `$TOOL_DATA_DIR/wikidata_todo_rs.json` (see
`config.json.template` and the `Procfile`).

## License

GPL-2.0-or-later
