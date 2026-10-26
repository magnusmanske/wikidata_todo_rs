#!/usr/bin/bash
# Builds the image on Toolforge. Does not (re)start anything.
set -euo pipefail
toolforge build start https://github.com/magnusmanske/wikidata_todo_rs
