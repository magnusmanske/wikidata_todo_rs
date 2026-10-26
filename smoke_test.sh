#!/usr/bin/env bash
# Requests every tool once and checks status and content.
# Usage: ./smoke_test.sh [base URL]   (default https://wikidata-todo.toolforge.org)
set -uo pipefail
BASE="${1:-https://wikidata-todo.toolforge.org}"
pass=0
fail=0

# check <expected status> <path> [text the body must contain]
check() {
  local expected="$1" path="$2" needle="${3:-}"
  local body status
  body=$(mktemp)
  status=$(curl -s -o "$body" -w '%{http_code}' --max-time 300 "$BASE$path")
  if [[ "$status" == "$expected" ]] && { [[ -z "$needle" ]] || grep -qF -- "$needle" "$body"; }; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    echo "FAIL $status (want $expected${needle:+, \"$needle\"}): $path"
  fi
  rm -f "$body"
}

# The main page, every tool's form, and old addresses.
check 200 "/" "Wikidata Todo"
check 200 "/?action=nogender&max=5" "wikidata.org/wiki/Q"
check 200 "/index.php?action=noauthor&bot=1&max=3&callback=cb" "cb("
for tool in add_name_labels artwork_images awarder beacon best_cats bioacoustica cloudy_concept country_stats \
  creator_from_wikidata dupe_finder get_item_names images_in_categories_used_on_commons images_near_items \
  important_blank_items label_no_instance linked_items missing_wp_animal_audio multibeacon new_items pagepile2kml \
  personendaten project_stats property_string_matcher relabel related_properties resolver reverse_tree \
  sourcerer_game sparql_rc stats subclass_dates test_descriptions translate_items_with_property \
  unused_images_of_people user_edits wd_commons wd_edit_stats wdq2graph wdq_image_feed wikidata_cache; do
  check 200 "/$tool/" "<h1>"
done
check 308 "/reverse_tree.php?title=Douglas_Adams&doit=1"
check 200 "/get_item_names.php?items=Q42&format=json" '"i2n"'

# Pages with results.
check 200 "/reverse_tree/?language=en&project=wikipedia&title=Douglas_Adams&namespace=0&doit=1" "Category"
check 200 "/relabel/?labels=Douglas+Adams" "Q42"
check 200 "/creator_from_wikidata/?q=Q42" "{{Creator"
check 200 "/sparql_rc.php?sparql=SELECT+%3Fq+%7B+VALUES+%3Fq+%7B+wd%3AQ42+%7D+%7D&start=last+week&format=json" '"status":"OK"'
check 200 "/related_properties.php?q=claim%5B31%3A2334719%5D&botmode=1" '"property"'

# APIs, at their old addresses.
check 200 "/description_api.php?q=Q42" '"short_desc"'
check 200 "/creator.php?action=check_titles_aliases&title=Douglas+Adams" '"Q42"'
check 200 "/canned_queries.php?items=42&params=%7B%22skip_if_proplink%22%3A%5B18%5D%7D" '"items"'
check 200 "/coords2json.php?sparql=SELECT+%3Fitem+%3Fcoord+%7B+wd%3AQ64+wdt%3AP625+%3Fcoord+BIND(wd%3AQ64+AS+%3Fitem)+%7D" "FeatureCollection"
check 200 "/last_changed_items.php?date=$(date -u -v-1H +%Y%m%d%H%M%S 2>/dev/null || date -u -d '1 hour ago' +%Y%m%d%H%M%S)" "["
check 200 "/duplicity_game_api.php?action=desc&callback=cb" "cb("
check 200 "/merge_candidates_by_shared_identifier.php?action=desc" '"label"'
check 200 "/pasleim_projectmerge_game_api.php?action=desc" '"label"'
check 200 "/duplicity/api.php?action=wikis" '"wikis"'
check 200 "/relator2/api.php?action=random_person" '"data":"Q'
check 200 "/wikiflix/api.php?action=get_all_sections" '"status":"OK"'
check 200 "/api.php?action=get_rights" '"error"'
check 303 "/resolver.php?prop=P214&value=113230702"
check 303 "/odnb_redirect.php?id=10101010"
check 302 "/duplicity.php?wiki=enwiki&page=X"
check 403 "/sourcery_proxy.php?url=https://example.org/"

# Static files of the PHP checkout, and what must never be served.
check 200 "/duplicity/" "duplicity"
check 200 "/wikidata_useful.js"
check 404 "/sparql_rc.php.bak"
check 404 "/php/common.php"

# Retired pages.
check 410 "/sexer.php"
check 308 "/quick_statements.php"

echo "$pass passed, $fail failed"
[[ $fail -eq 0 ]]
