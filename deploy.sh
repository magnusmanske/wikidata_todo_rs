#!/usr/bin/env bash
# Delopy and restart toolforge services
ssh login.toolforge.org 'become wikidata-todo bash -lc "cd ~/wikidata_todo_rs/ && git pull && ./build.sh && webservice restart && echo \"OK\""'
