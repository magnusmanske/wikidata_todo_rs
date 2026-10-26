# Plan

This codebase replaces the PHP web service and the jobs of wikidata-todo
(`~/php/wikidata-todo`, https://wikidata-todo.toolforge.org/) with one Rust
binary, built like `magnustools_rs`: axum, `wikimisc`/`toolforge` for all
database connections, askama templates, one stylesheet.

## Decisions
- URLs and parameters stay; pages moved to `/<name>/` with redirects from `.php`, APIs stay where they are.
- Server-rendered tools share the new look. The Vue apps (Duplicity, Relator, Relator2, WikiFlix, Graveyard shift, Papertrail) keep their interface; their files moved into this repository (`public/`, built into the binary), their APIs are ported. Other static files (pages, scripts, data, and WikiFlix's generated `config.js`) are served from the PHP checkout as they are.
- Tools that broke (WDQ, `wb_terms`, `revision_compat`, removed columns) are fixed where still useful, else retired.
- Jobs that feed live tools are ported; dead ones retired; the Wikispecies bot stays PHP.
- Labels come from the term store (`wbt_*`, own server), never the frozen copies in the main database; the tools' own SPARQL avoids what WDQS's successor (QLever) lacks. Heavy job queries fall back to QLever when WDQS times out.
- Logins (Widar) live in an encrypted cookie; users log in once more after the switch. JSONP for Widar is gone (it was announced as removed), edits need a same-site request.

## Endpoint dispositions

Ported: every `public_html/*.php` page and API, `duplicity/`, `relator/`,
`relator2/`, `wikiflix/` and `wp_no_image/` APIs (see README).

Retired (`src/web/legacy.rs`):

| Path | Fate | Why |
|---|---|---|
| `wdq_feed.php`, `sexer.php`, `random_item_without_instance.php` | 410 | Deactivated in PHP |
| `no_statements.php` | → `/important_blank_items/` | Deactivated; same purpose |
| `firstnamebasis.php` | 410 | Needs all humans at once |
| `true_duplicates.php` | 410 | Wikidata prevents duplicate sitelinks |
| `viaf_game.php` | 410 | Never finished |
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
| process-specieswiki | stays PHP (in `jobs.yaml`) |
| auto-add-dewiki, moon-bot, top-items-no-statement, update-dumps, update-photos-by-employer | retired: broken for months or years (missing libraries, removed tables, dead WDQ, dumps no longer reachable) |

## Cutover
1. Push this repository to GitHub; `./build.sh` builds the image.
2. `$TOOL_DATA_DIR/wikidata_todo_rs.json` from `config.json.template`, with a `cookie_key` (`openssl rand -hex 64`); `oauth.ini` is already in the tool home. Check that the OAuth consumer accepts the callbacks `https://wikidata-todo.toolforge.org/api.php?…`, `/duplicity/api.php?…`, `/relator/api.php`, `/relator2/api.php` (the PHP used the same).
3. ~~`duplicity_p.candidates` had used up its IDs~~: `id` is BIGINT now (done 2026-10-26).
4. `toolforge webservice stop && toolforge webservice buildservice start --mount=all`, then `./smoke_test.sh`.
5. Delete the PHP jobs replaced or retired above (`toolforge jobs delete <name>`), then `toolforge jobs load jobs.yaml`.

Rollback: `toolforge webservice stop && toolforge webservice php8.4 start`; PHP jobs from `tjs`.

## Open
- WikiFlix and the Wikidata cache read databases whose update scripts have not run for years (`wikiflix_p`, `wikidata_cache_p`, last updated 2022); `stats` shows dumps until 2021.
- `project_stats` estimates labels per language from a sample: exact counts take hours.
- `beacon` on the largest properties (GND, VIAF) is cut off by WDQS's 60 s limit (marked in the output); `subclass_dates` and `best_cats` could use QLever too.
- geni.com blocks bots, so Relator's Geni import finds nothing.
- The apps' copies in the PHP repository (`public_html/duplicity/` etc.) are no longer served; changes to the apps belong here now. They can be removed there after the switch.
