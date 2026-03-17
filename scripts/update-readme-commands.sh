#!/usr/bin/env bash
# Generate the CLI commands section of README.md from `longbridge --help` output.
#
# Usage:
#   ./scripts/gen-readme-commands.sh                  # print to stdout
#   ./scripts/gen-readme-commands.sh --update         # update README.md in-place
#
# Requirements: the `longbridge` binary must be on PATH (run `cargo build --release` first,
# or set LONGBRIDGE_BIN to the binary path).
#
# The script replaces the block between the markers:
#   <!-- COMMANDS_START -->
#   <!-- COMMANDS_END -->
# in README.md when --update is passed.

set -euo pipefail

BIN="${LONGBRIDGE_BIN:-longbridge}"

if ! command -v "$BIN" &>/dev/null; then
  echo "Error: '$BIN' not found. Build first: cargo build --release" >&2
  echo "Or set LONGBRIDGE_BIN=/path/to/longbridge" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Helper: extract one-line description for a subcommand
# ---------------------------------------------------------------------------
describe() {
  local cmd="$1"
  # Run `longbridge <cmd> --help`, grab the first non-empty line after "Usage:"
  # Clap prints: "<binary> <cmd>" then a blank line then the short description.
  "$BIN" "$cmd" --help 2>/dev/null \
    | awk '
        /^$/ { blank=1; next }
        blank && /^[A-Z]/ { print; exit }
        /^Usage:/ { next }
      ' \
    | head -1
}

# ---------------------------------------------------------------------------
# Build each section
# ---------------------------------------------------------------------------
quotes() {
  cat <<EOF
### Quotes

\`\`\`bash
longbridge quote TSLA.US 700.HK                                       # $(describe quote)
longbridge depth TSLA.US                                              # $(describe depth)
longbridge brokers 700.HK                                             # $(describe brokers)
longbridge trades TSLA.US [--count 50]                                # $(describe trades)
longbridge intraday TSLA.US                                           # $(describe intraday)
longbridge kline TSLA.US [--period day] [--count 100]                 # $(describe kline)
longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31 # $(describe kline-history)
longbridge static TSLA.US                                             # $(describe static)
longbridge calc-index TSLA.US --index pe,pb,eps                       # $(describe calc-index)
longbridge capital-flow TSLA.US                                       # $(describe capital-flow)
longbridge capital-dist TSLA.US                                       # $(describe capital-dist)
longbridge market-temp [HK|US|CN|SG]                                  # $(describe market-temp)
longbridge trading-session                                            # $(describe trading-session)
longbridge trading-days HK                                            # $(describe trading-days)
longbridge security-list HK                                           # $(describe security-list)
longbridge participants                                               # $(describe participants)
longbridge subscriptions                                              # $(describe subscriptions)
\`\`\`
EOF
}

options_warrants() {
  cat <<EOF

### Options & Warrants

\`\`\`bash
longbridge option-quote AAPL240119C190000         # $(describe option-quote)
longbridge option-chain AAPL.US                   # Option chain: list all expiry dates
longbridge option-chain AAPL.US --date 2024-01-19 # Option chain: strike prices for a given expiry
longbridge warrant-quote 12345.HK                 # $(describe warrant-quote)
longbridge warrant-list 700.HK                    # $(describe warrant-list)
longbridge warrant-issuers                        # $(describe warrant-issuers)
\`\`\`
EOF
}

watchlist() {
  cat <<EOF

### Watchlist

\`\`\`bash
longbridge watchlist                                             # $(describe watchlist)
longbridge watchlist create "My Portfolio"                       # Create a new watchlist group
longbridge watchlist update <id> --add TSLA.US --remove AAPL.US  # Add/remove securities in a group, or rename it
longbridge watchlist delete <id>                                 # Delete a watchlist group
\`\`\`
EOF
}

trading() {
  cat <<EOF

### Trading

\`\`\`bash
longbridge orders                                      # $(describe orders)
longbridge orders --history [--start 2024-01-01]       # Historical orders (use --symbol to filter)
longbridge order <order_id>                            # $(describe order)
longbridge executions                                  # $(describe executions)
longbridge buy TSLA.US 100 --price 250.00              # $(describe buy)
longbridge sell TSLA.US 100 --price 260.00             # $(describe sell)
longbridge cancel <order_id>                           # $(describe cancel)
longbridge replace <order_id> --qty 200 --price 255.00 # $(describe replace)
longbridge balance                                     # $(describe balance)
longbridge cash-flow [--start 2024-01-01]              # $(describe cash-flow)
longbridge positions                                   # $(describe positions)
longbridge fund-positions                              # $(describe fund-positions)
longbridge margin-ratio TSLA.US                        # $(describe margin-ratio)
longbridge max-qty TSLA.US --side buy --price 250      # $(describe max-qty)
\`\`\`
EOF
}

# ---------------------------------------------------------------------------
# Combine all sections
# ---------------------------------------------------------------------------
generate() {
  quotes
  options_warrants
  watchlist
  trading
}

# ---------------------------------------------------------------------------
# Print or update README.md
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--update" ]]; then
  readme="$(dirname "$0")/../README.md"
  if [[ ! -f "$readme" ]]; then
    echo "Error: README.md not found at $readme" >&2
    exit 1
  fi

  marker_start="<!-- COMMANDS_START -->"
  marker_end="<!-- COMMANDS_END -->"

  if ! grep -q "$marker_start" "$readme"; then
    echo "Error: marker '$marker_start' not found in README.md" >&2
    echo "Add the markers around the commands section:" >&2
    echo "  $marker_start" >&2
    echo "  $marker_end" >&2
    exit 1
  fi

  # Replace content between markers (inclusive of markers)
  generated="$(generate)"
  awk -v start="$marker_start" -v end="$marker_end" -v content="$generated" '
    $0 == start { print; print content; skip=1; next }
    $0 == end   { skip=0 }
    !skip        { print }
  ' "$readme" > "$readme.tmp" && mv "$readme.tmp" "$readme"

  echo "README.md updated."
else
  generate
fi
