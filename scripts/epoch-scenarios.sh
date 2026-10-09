#!/usr/bin/env bash
# epoch-scenarios.sh - regtest assertions for vnprc/hashpool#18 (automatic
# reward trigger). Run against a live stack started separately with
# HASHPOOL_MINER=off and poll_interval_secs=1. Reads confirmation_depth from
# config/mint.config.toml (default 6) rather than assuming a fixed value.
#
# Usage: scripts/epoch-scenarios.sh <1|3|4|5>
#
# Scenario 2 (full-stack) is driven by hand: it needs a pool config edit and
# the miner, not scripted here.
#
# Prints one "PASS: <what>" or "FAIL: <what> (expected X, got Y)" line per
# assertion and exits non-zero if any assertion failed.

set -euo pipefail

FAILED=0

pass() {
  echo "PASS: $1"
}

fail() {
  echo "FAIL: $1 (expected $2, got $3)"
  FAILED=1
}

assert_eq() {
  local what="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    pass "$what"
  else
    fail "$what" "$expected" "$actual"
  fi
}

# --- helpers (brief section H) ---

cli() {
  bitcoin-cli -datadir=.devenv/state/bitcoind -conf="$PWD/config/bitcoin.conf" \
    -rpcuser=username -rpcpassword=password -regtest "$@"
}

mint_addr() {
  grep -E '^[[:space:]]*receive_address[[:space:]]*=' config/mint.config.toml \
    | tail -n1 | cut -d= -f2- | xargs | tr -d '"'
}

pool_pubkey() {
  grep -E '^[[:space:]]*pool_pubkey[[:space:]]*=' config/mint.config.toml \
    | tail -n1 | cut -d= -f2- | xargs | tr -d '"'
}

confirmation_depth() {
  local value
  value=$(grep -E '^[[:space:]]*confirmation_depth[[:space:]]*=' config/mint.config.toml \
    | tail -n1 | cut -d= -f2- | xargs || true)
  if [ -z "$value" ]; then
    echo 6
  else
    echo "$value"
  fi
}

other_addr() {
  cli -rpcwallet=regtest getnewaddress
}

# The epoch records now live in the mint database's cdk key-value store
# (primary namespace "hashpool", secondary namespace "epochs", key
# "records"), not a JSON file; the stored value is a bare JSON array, so it
# is re-wrapped as `{records: ...}` to keep every existing ".records"
# filter below unchanged.
epochs_records_json() {
  sqlite3 .devenv/state/mint/mint.sqlite \
    "SELECT CAST(value AS TEXT) FROM kv_store WHERE primary_namespace='hashpool' AND secondary_namespace='epochs' AND key='records';"
}

epochs() {
  epochs_records_json | jq -r "{records: .} | $1"
}

quotes_in_unit() {
  local unit="$1"
  sqlite3 .devenv/state/mint/mint.sqlite \
    "SELECT COUNT(*) FROM mint_quote WHERE unit = '$unit';"
}

paid_in_unit() {
  local unit="$1"
  sqlite3 .devenv/state/mint/mint.sqlite \
    "SELECT COUNT(*) FROM mint_quote WHERE unit = '$unit' AND amount_paid > 0;"
}

# Polls `predicate ...args` once a second for up to `timeout` seconds.
wait_for() {
  local timeout="$1"
  shift
  local waited=0
  while [ "$waited" -lt "$timeout" ]; do
    if "$@"; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

log_offset() {
  if [ -f logs/mint.log ]; then
    wc -l < logs/mint.log
  else
    echo 0
  fi
}

log_since() {
  local offset="$1"
  tail -n "+$((offset + 1))" logs/mint.log
}

# 50 BTC, halving every 150 regtest blocks, in sats.
subsidy_sats() {
  local height="$1"
  local halvings=$((height / 150))
  echo $((5000000000 >> halvings))
}

# --- predicates for wait_for ---

record_is_provisional_reward_at_height() {
  local height="$1"
  local state
  state=$(epochs ".records[] | select(.height == $height and .source == \"reward\") | .state")
  [ "$state" = "provisional" ]
}

record_is_final() {
  local unit="$1"
  local state
  state=$(epochs ".records[] | select(.unit == \"$unit\") | .state")
  [ "$state" = "final" ]
}

record_is_dissolved() {
  local unit="$1"
  local state
  state=$(epochs ".records[] | select(.unit == \"$unit\") | .state")
  [ "$state" = "dissolved" ]
}

record_remined_at_height() {
  local height="$1" old_hash="$2"
  local hash state
  hash=$(epochs ".records[] | select(.height == $height) | .block_hash")
  state=$(epochs ".records[] | select(.height == $height) | .state")
  if [ "$hash" = "$old_hash" ]; then
    return 1
  fi
  if [ "$state" != "provisional" ]; then
    return 1
  fi
  return 0
}

mint_port_open() {
  nc -z localhost 3338
}

# Assumes no earlier reward in this run shares these three heights (true for
# a scenario run against a fresh stack, as the brief describes). Checks that
# every wanted height is among the reward records' heights; a string-joined
# comma pattern is not safe here because adjacent wanted heights each need
# their own leading and trailing comma, which a single match cannot supply
# for both at once (",17,37,57,59,62,65," vs. "*,59,*,62,*,65,*").
three_reward_records_present() {
  local h1="$1" h2="$2" h3="$3"
  epochs_records_json | jq -e --argjson want "[$h1, $h2, $h3]" \
    '[.[] | select(.source == "reward") | .height] as $have
     | all($want[]; . as $w | $have | index($w) != null)' \
    > /dev/null
}

current_unit() {
  epochs '[.records[] | select(.state != "dissolved")][-1].unit'
}

nut04_has_unit() {
  local unit="$1"
  # The nut key is the numeric string "4", not the name "nut04".
  curl -s http://localhost:3338/v1/info | jq -r '.nuts."4".methods[].unit' | grep -qx "$unit"
}

# --- scenarios ---

scenario_1() {
  echo "--- scenario 1: deterministic trigger ---"

  local before_count
  before_count=$(epochs '.records | length')

  cli generatetoaddress 1 "$(mint_addr)" > /dev/null
  local tip
  tip=$(cli getblockcount)
  local unit="hash_$(pool_pubkey)_${tip}"

  if wait_for 30 record_is_provisional_reward_at_height "$tip"; then
    pass "scenario 1: a provisional reward record appears at height $tip"
  else
    fail "scenario 1: a provisional reward record appears at height $tip" "provisional" "absent after 30s"
    return
  fi

  local after_count expected_count
  after_count=$(epochs '.records | length')
  expected_count=$((before_count + 1))
  assert_eq "scenario 1: exactly one new record" "$expected_count" "$after_count"

  local actual_unit
  actual_unit=$(epochs ".records[] | select(.height == $tip and .source == \"reward\") | .unit")
  assert_eq "scenario 1: the new record's unit name" "$unit" "$actual_unit"

  local actual_sats expected_sats
  actual_sats=$(epochs ".records[] | select(.unit == \"$unit\") | .reward_sats")
  expected_sats=$(subsidy_sats "$tip")
  assert_eq "scenario 1: reward_sats matches the regtest subsidy at height $tip" "$expected_sats" "$actual_sats"

  cli generatetoaddress "$((DEPTH - 1))" "$(other_addr)" > /dev/null

  if wait_for 30 record_is_final "$unit"; then
    pass "scenario 1: $unit reaches final at D=$DEPTH"
  else
    fail "scenario 1: $unit reaches final at D=$DEPTH" "final" "still not final after 30s"
    return
  fi

  if nut04_has_unit "$unit"; then
    pass "scenario 1: the new unit is present in /v1/info nut04"
  else
    fail "scenario 1: the new unit is present in /v1/info nut04" "present" "absent"
  fi
}

scenario_3() {
  echo "--- scenario 3: catch-up ---"
  echo "(the overseer must have stopped the mint process by hand already)"

  local heights=()
  local i
  for i in 1 2 3; do
    cli generatetoaddress 1 "$(mint_addr)" > /dev/null
    local h
    h=$(cli getblockcount)
    heights+=("$h")
    echo "reward mined at height $h"
    if [ "$i" -lt 3 ]; then
      cli generatetoaddress 2 "$(other_addr)" > /dev/null
    fi
  done

  echo "waiting for the mint to be restarted by hand..."
  if wait_for 300 mint_port_open; then
    pass "scenario 3: the mint's port reopens after restart"
  else
    fail "scenario 3: the mint's port reopens after restart" "open within 300s" "still closed"
    return
  fi

  if wait_for 60 three_reward_records_present "${heights[0]}" "${heights[1]}" "${heights[2]}"; then
    pass "scenario 3: three reward records appear at ${heights[*]}, in order"
  else
    fail "scenario 3: three reward records appear at ${heights[*]}, in order" "all three present" "not all present after 60s"
    return
  fi

  cli generatetoaddress "$((DEPTH - 1))" "$(other_addr)" > /dev/null

  local last_unit="hash_$(pool_pubkey)_${heights[2]}"
  if wait_for 30 record_is_final "$last_unit"; then
    pass "scenario 3: the last reward epoch reaches final"
  else
    fail "scenario 3: the last reward epoch reaches final" "final" "still not final after 30s"
  fi

  assert_eq "scenario 3: the current unit is the last mined epoch" "$last_unit" "$(current_unit)"
}

scenario_4() {
  echo "--- scenario 4: dissolve ---"

  local offset
  offset=$(log_offset)
  local prev_unit
  prev_unit=$(current_unit)

  cli generatetoaddress 1 "$(mint_addr)" > /dev/null
  local h
  h=$(cli getblockcount)
  local unit="hash_$(pool_pubkey)_${h}"

  if wait_for 30 record_is_provisional_reward_at_height "$h"; then
    pass "scenario 4: a provisional record appears at height $h"
  else
    fail "scenario 4: a provisional record appears at height $h" "provisional" "absent after 30s"
    return
  fi

  local block_hash
  block_hash=$(epochs ".records[] | select(.unit == \"$unit\") | .block_hash")

  cli invalidateblock "$block_hash" > /dev/null
  cli generatetoaddress 2 "$(other_addr)" > /dev/null

  if wait_for 30 record_is_dissolved "$unit"; then
    pass "scenario 4: $unit becomes dissolved"
  else
    fail "scenario 4: $unit becomes dissolved" "dissolved" "still not dissolved after 30s"
  fi

  assert_eq "scenario 4: the current unit rolls back to the previous epoch" "$prev_unit" "$(current_unit)"

  if nut04_has_unit "$prev_unit"; then
    pass "scenario 4: the previous unit still has nut04 settings"
  else
    fail "scenario 4: the previous unit still has nut04 settings" "present" "absent"
  fi

  # Strip ANSI colour codes first (tracing's pretty output puts them between
  # the field name and its value), then treat "the line says epoch
  # dissolved" and "the line names this unit" as two separate greps rather
  # than one combined "unit=<value>" pattern that colour codes can break.
  local dissolve_lines
  dissolve_lines=$(log_since "$offset" | sed 's/\x1b\[[0-9;]*m//g' | grep "epoch dissolved" | grep -c "$unit" || true)
  if [ "$dissolve_lines" -ge 1 ]; then
    pass "scenario 4: the mint log records the dissolve of $unit"
  else
    fail "scenario 4: the mint log records the dissolve of $unit" "at least one log line" "none"
  fi
}

scenario_5() {
  echo "--- scenario 5: same-height re-mine ---"

  cli generatetoaddress 1 "$(mint_addr)" > /dev/null
  local h
  h=$(cli getblockcount)
  local unit="hash_$(pool_pubkey)_${h}"

  if wait_for 30 record_is_provisional_reward_at_height "$h"; then
    pass "scenario 5: a provisional record appears at height $h"
  else
    fail "scenario 5: a provisional record appears at height $h" "provisional" "absent after 30s"
    return
  fi

  local old_hash
  old_hash=$(epochs ".records[] | select(.unit == \"$unit\") | .block_hash")

  cli invalidateblock "$old_hash" > /dev/null
  cli generatetoaddress 1 "$(mint_addr)" > /dev/null

  if wait_for 30 record_remined_at_height "$h" "$old_hash"; then
    pass "scenario 5: the record at height $h gets a new hash and stays provisional"
  else
    fail "scenario 5: the record at height $h gets a new hash and stays provisional" "new hash, provisional" "unchanged after 30s"
  fi

  local record_count_at_h
  record_count_at_h=$(epochs ".records[] | select(.height == $h) | .unit" | wc -l)
  assert_eq "scenario 5: no second record exists at height $h" "1" "$record_count_at_h"

  cli generatetoaddress "$((DEPTH - 1))" "$(other_addr)" > /dev/null

  if wait_for 30 record_is_final "$unit"; then
    pass "scenario 5: $unit reaches final after the re-mine"
  else
    fail "scenario 5: $unit reaches final after the re-mine" "final" "still not final after 30s"
  fi
}

main() {
  local scenario="${1:-}"
  DEPTH="$(confirmation_depth)"
  case "$scenario" in
    1) scenario_1 ;;
    3) scenario_3 ;;
    4) scenario_4 ;;
    5) scenario_5 ;;
    *)
      echo "usage: $0 <1|3|4|5>" >&2
      echo "(scenario 2 is driven by hand; see docs/EPOCH_DESIGN.md)" >&2
      exit 2
      ;;
  esac
  exit "$FAILED"
}

main "$@"
