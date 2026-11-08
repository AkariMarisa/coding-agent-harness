#!/usr/bin/env bash
# Official list prices, USD per 1M tokens, as of 2026-11-08.
#
# Sources:
#   deepseek-flash  api-docs.deepseek.com/quick_start/pricing
#                   (DeepSeek-V4.1-Flash-1017; OFF-PEAK rates — peak is 2x
#                   during 01:00-04:00 & 06:00-10:00 UTC Mon-Fri)
#   gpt-6.1-sol     developers.openai.com/api/docs/models/gpt-6.1-sol
#                   (Standard, short context <=272K)
#
# Relay discounts vs official (user's relay billing): deepseek relay 0.09x,
# gpt relay 0.32x. Create a separate [[models]] alias with those multipliers
# when measuring relay spend; official endpoints use these numbers directly.
#
# Sourced by scripts; safe under `set -euo pipefail`.

price_input() {
    case "$1" in
        deepseek-flash) echo "0.15" ;;
        gpt-6.1-sol) echo "2.0" ;;
    esac
}

price_cached() {
    case "$1" in
        deepseek-flash) echo "0.003" ;;
        gpt-6.1-sol) echo "0.1" ;;
    esac
}

price_cache_write() {
    case "$1" in
        gpt-6.1-sol) echo "2.5" ;;
    esac
}

price_output() {
    case "$1" in
        deepseek-flash) echo "0.6" ;;
        gpt-6.1-sol) echo "10.0" ;;
    esac
}

price_version() {
    case "$1" in
        deepseek-flash) echo "2026-11-08 deepseek offpeak (peak2x)" ;;
        gpt-6.1-sol) echo "2026-11-08 openai standard short-context" ;;
    esac
}

# Print the [[models]] price fields for a model id; nothing if unknown (the
# ledger then honestly records cost_source=unknown instead of guessing).
price_toml() {
    local input cached output version cache_write
    input=$(price_input "$1")
    if [ -z "$input" ]; then
        return 0
    fi
    cached=$(price_cached "$1")
    output=$(price_output "$1")
    version=$(price_version "$1")
    printf 'input_usd_per_mtok = %s\n' "$input"
    printf 'cached_input_usd_per_mtok = %s\n' "$cached"
    printf 'output_usd_per_mtok = %s\n' "$output"
    cache_write=$(price_cache_write "$1")
    if [ -n "$cache_write" ]; then
        printf 'cache_write_usd_per_mtok = %s\n' "$cache_write"
    fi
    printf 'pricing_version = "%s"\n' "$version"
}
