# Plan

This codebase replaces the PHP web service and the jobs of wikidata-todo
(`~/php/wikidata-todo`, https://wikidata-todo.toolforge.org/) with one Rust
binary, built like `magnustools_rs`: axum, `wikimisc`/`toolforge` for all
database connections, askama templates, one stylesheet.

## Decisions
- URLs and parameters stay; pages moved to `/<name>/` with redirects from `.php`, APIs stay where they are.
- Server-rendered tools share the new look. The Vue apps (Duplicity, Relator, Relator2, Graveyard shift, Papertrail) keep their interface; their files moved into this repository (`public/`, built into the binary), their APIs are ported. Other static files (pages, scripts, data) are served from the PHP checkout as they are.
- Tools that broke (WDQ, `wb_terms`, `revision_compat`, removed columns) are fixed where still useful, else retired.
- Jobs that feed live tools are ported; dead ones retired. The Wikispecies bot is ported too; it runs as a dry run beside the PHP one until its log has been checked.
- Labels come from the term store (`wbt_*`, own server), never the frozen copies in the main database; the tools' own SPARQL avoids what WDQS's successor (QLever) lacks. Heavy job queries use QLever: directly where WDQS cannot finish them (the noitems list), and as a fallback on WDQS timeouts.
- Logins (Widar) live in an encrypted cookie; users log in once more after the switch. JSONP for Widar is gone (it was announced as removed), edits need a same-site request.

## Endpoint dispositions

Ported: every `public_html/*.php` page and API, `duplicity/`, `relator/`,
`relator2/` and `wp_no_image/` APIs (see README). WikiFlix became its own tool: `/wikiflix/…` redirects to https://wikiflix.toolforge.org/.

Retired (`src/web/legacy.rs`):

| Path | Fate | Why |
|---|---|---|
| `wdq_feed.php`, `sexer.php`, `random_item_without_instance.php` | 410 | Deactivated in PHP |
| `no_statements.php` | → `/important_blank_items/` | Deactivated; same purpose |
| `firstnamebasis.php` | 410 | Needs all humans at once |
| `true_duplicates.php` | 410 | Wikidata prevents duplicate sitelinks |
| `viaf_game.php` | 410 | Never finished |
| `wikidata_cache.php` | 410 | Its data stopped being updated in 2022 |
| `autolist2.php`, `download_query.php` | → PetScan | AutoList became PetScan |
| `quick_statements.php` | → quickstatements.toolforge.org | Has its own tool |

Kept, but without data: `findagrave_recent_celeb_deaths_xml.php` answers an
empty RSS feed (Find a Grave has no feed any more), so `recent_deaths.html` is
empty. `canned_queries.html`, `relations.html` and other static pages that call
WDQ from the browser stay broken.

## Jobs

| PHP job | Now |
|---|---|
| update-duplicity | `duplicity-update` |
| add-candidates | `duplicity-candidates` |
| update-game-merge-by-id | `merge-by-id-update` |
| pasleim-projectmerge-update | `pasleim-update` |
| update-noitems | `noitems-update` (daily) |
| update-user-scripts | `user-scripts-update` |
| process-specieswiki | `specieswiki` (`--dry-run` for now; the PHP job still runs) |
| auto-add-dewiki, moon-bot, top-items-no-statement, update-dumps, update-photos-by-employer | retired: broken for months or years (missing libraries, removed tables, dead WDQ, dumps no longer reachable) |

## Graveyard shift

The app keeps its own Vue/jQuery/Leaflet code; only its page shell and styles
follow the tools' look. It lists people buried at a cemetery (P119), their
grave image (P1442) and grave coordinates (a P625 qualifier on the P119
statement), from WDQS. The map zooms past the tile service's level 18 by
enlarging the level-18 tiles.

Next, in this order:

1. **Place a grave on the map.** A "place grave" mode rather than a bare map
   click, which would fight panning and is awkward on touch. The next click is
   the coordinate; the user then picks one of the cemetery's people who has no
   coordinates yet, and confirms person, cemetery and position. If that person
   already has a P119 statement for the cemetery, add a P625 qualifier to it,
   else add the P119 statement with that qualifier. The query should return the
   statement ID (`?statement` in the `p:P119` part) so the edit needs no second
   lookup. Show an unconfirmed marker until the edit succeeds; on failure the
   table and map stay as they were. This brings Widar login back to the tools
   row (the app edits again), with the same-origin check Duplicity uses.
2. **Attach an image.** The file belongs on Commons; the person gets P1442.
   First pick an existing Commons file (by title, or via the geosearch call
   already sketched in `main-page.html`) and set P1442. Uploading is its own
   step: Commons' `action=upload` needs an OAuth consumer allowed to upload
   (the Wikidata one may not be), a CSRF token, a multipart request, and a
   description with licence and source; only a successful upload leads to the
   Wikidata edit. Keep uploads in the browser unless rate limits or file sizes
   force them through the tool's API.
3. **Use the table as the work queue.** People without coordinates are already
   marked; a "place grave" button per person (or in the toolbar, then pick the
   person) ties 1 and 2 together. The mobile table is a card list, so the
   button has room there.

Tests must not edit live Wikidata. Any writes go through Widar directly, as
the PHP tools did; the app itself stays client-side.

## Cutover
1. Push this repository to GitHub; `./build.sh` builds the image.
2. `$TOOL_DATA_DIR/wikidata_todo_rs.json` from `config.json.template`, with a `cookie_key` (`openssl rand -hex 64`); `oauth.ini` is already in the tool home. Check that the OAuth consumer accepts the callbacks `https://wikidata-todo.toolforge.org/api.php?…`, `/duplicity/api.php?…`, `/relator/api.php`, `/relator2/api.php` (the PHP used the same).
3. ~~`duplicity_p.candidates` had used up its IDs~~: `id` is BIGINT now (done 2026-10-26).
4. `toolforge webservice stop && toolforge webservice buildservice start --mount=all`, then `./smoke_test.sh`.
5. Delete the PHP jobs replaced or retired above (`toolforge jobs delete <name>`), then `toolforge jobs load jobs.yaml`.

Rollback: `toolforge webservice stop && toolforge webservice php8.4 start`; PHP jobs from `tjs`.

## Open
- `stats` shows dumps until 2021.
- `project_stats` estimates labels per language from a sample: exact counts take hours.
- `beacon` on the largest properties (GND, VIAF) is cut off by WDQS's 60 s limit (marked in the output); `subclass_dates` and `best_cats` could use QLever too.
- geni.com blocks bots, so Relator's Geni import finds nothing.
- The apps' copies in the PHP repository (`public_html/duplicity/` etc.) are no longer served; changes to the apps belong here now. They can be removed there after the switch.
