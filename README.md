# Longbridge Terminal

An _experimental_ terminal-based stock trading app built with [Longbridge OpenAPI](https://open.longbridge.com).

A Rust-based TUI (Terminal User Interface) for monitoring market data and managing stock portfolios. Built to showcase the capabilities of the Longbridge OpenAPI SDK.

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## Features

- TUI with real-time market data and portfolio management
- CLI wrapper for all Longbridge OpenAPI endpoints for AI-native integration.
- Real-time watchlist with live market data
- Portfolio management
- Stock search and quotes
- Candlestick charts
- Multi-market support (Hong Kong, US, China A-share)
- Built on Rust + Ratatui
- Vim-like keybindings

## System Requirements

- macOS or Linux
- Internet connection and browser access (for OAuth authentication)
- Longbridge account (free to register at [open.longbridge.com](https://open.longbridge.com))

## Installation

### From Binary

If you're on macOS or Linux, run the following command in your terminal:

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

This will install the `longbridge` command in your terminal.

## Configuration

The app uses **OAuth2.1** for authentication. No manual configuration is required!

### First Time Setup

1. **Create a Longbridge Account**: If you don't have one, register at [Longbridge Open Platform](https://open.longbridge.com)

2. **Run the App**:

   ```bash
   longbridge
   ```

3. **Automatic OAuth Flow**:
   - The app will automatically register an OAuth client with Longbridge
   - Your default browser will open for authorization
   - After you approve, the app will receive an access token
   - The token is securely saved to your system keychain

That's it! On subsequent runs, the app will automatically use the saved token.

### Token Storage

Access tokens are stored securely in your system's credential manager:

- **macOS**: Keychain Access
- **Windows**: Credential Manager
- **Linux**: Secret Service (libsecret)

Service name: `com.longbridge.terminal`

### Token Refresh

Access tokens are automatically refreshed when they expire. No manual intervention needed.

### Troubleshooting

If you encounter authentication issues:

```bash
# View detailed OAuth flow logs
RUST_LOG=debug longbridge

# The app listens on localhost:8877 for OAuth callback
# If this port is in use, it will try ports 8878-8880
```

**Requirements:**

- Internet connection
- Browser access
- Active Longbridge account

## CLI

In addition to the TUI, `longbridge` exposes every Longbridge OpenAPI endpoint as a CLI command. The same OAuth token is shared between TUI and CLI — run `longbridge login` once and both modes work.

### Authentication

```bash
longbridge login    # OAuth browser flow, saves token
longbridge logout   # Clear saved token
```

### Use Cases

```bash
$ longbridge quote TSLA.US
+---------+---------+------------+---------+---------+---------+----------+-----------------+--------+
| Symbol  | Last    | Prev Close | Open    | High    | Low     | Volume   | Turnover        | Status |
+====================================================================================================+
| TSLA.US | 395.560 | 391.200    | 396.220 | 403.730 | 394.420 | 58068343 | 23138752546.000 | Normal |
+---------+---------+------------+---------+---------+---------+----------+-----------------+--------+
```

```bash
$ longbridge kline AAPL.US --period day
+---------------------+---------+---------+---------+---------+-----------+-----------------+
| Time                | Open    | High    | Low     | Close   | Volume    | Turnover        |
+===========================================================================================+
| 2025-10-21 04:00:00 | 261.880 | 265.290 | 261.830 | 262.770 | 46695948  | 12300536141.000 |
|---------------------+---------+---------+---------+---------+-----------+-----------------|
| 2025-10-22 04:00:00 | 262.650 | 262.850 | 255.430 | 258.450 | 45015254  | 11644676589.000 |
|---------------------+---------+---------+---------+---------+-----------+-----------------|
| 2025-10-23 04:00:00 | 259.940 | 260.620 | 258.010 | 259.580 | 32754941  | 8502427792.000  |
|---------------------+---------+---------+---------+---------+-----------+-----------------|
| 2025-10-24 04:00:00 | 261.190 | 264.130 | 259.180 | 262.820 | 38253717  | 10043092739.000 |
|---------------------+---------+---------+---------+---------+-----------+-----------------|
| 2025-10-27 04:00:00 | 264.880 | 269.120 | 264.650 | 268.810 | 44888152  | 11985374919.000 |
|---------------------+---------+---------+---------+---------+-----------+-----------------|
| 2025-10-28 04:00:00 | 268.985 | 269.890 | 268.150 | 269.000 | 41534759  | 11172288927.000 |
|---------------------+---------+---------+---------+---------+-----------+-----------------|
| 2025-10-29 04:00:00 | 269.275 | 271.410 | 267.110 | 269.700 | 51086742  | 13768515898.000 |
...
```

### Output Format

All commands support `--format table` (default) or `--format json` for AI-agent / pipe use:

```bash
longbridge static TSLA.US
longbridge quote TSLA.US AAPL.US --format json | jq '.[] | {symbol, last}'
longbridge positions --format json
longbridge orders --format csv > orders.csv
```

### Commands

Use `longbridge -h` or `longbridge <command> -h` for detailed usage of each command.

**Quotes**

| Command                                                                    | Description                                           |
| -------------------------------------------------------------------------- | ----------------------------------------------------- |
| `longbridge quote TSLA.US 700.HK`                                          | Real-time quotes                                      |
| `longbridge depth TSLA.US`                                                 | Order book depth                                      |
| `longbridge brokers TSLA.US`                                               | Broker queue                                          |
| `longbridge trades TSLA.US [--count 50]`                                   | Recent trades                                         |
| `longbridge intraday TSLA.US`                                              | Intraday lines                                        |
| `longbridge kline TSLA.US [--period day]`                                  | Candlesticks (`1m 5m 15m 30m 1h day week month year`) |
| `longbridge kline-history TSLA.US [--start 2024-01-01] [--end 2024-12-31]` | History candlesticks                                  |
| `longbridge static TSLA.US`                                                | Static info (name, lot size, currency)                |
| `longbridge calc-index TSLA.US`                                            | Calculated indexes (PE, PB, EPS…)                     |
| `longbridge capital-flow TSLA.US`                                          | Capital flow                                          |
| `longbridge capital-dist TSLA.US`                                          | Capital distribution                                  |
| `longbridge market-temp [HK\|US\|CN\|SG]`                                  | Market temperature                                    |
| `longbridge trading-session`                                               | Trading sessions                                      |
| `longbridge trading-days HK`                                               | Trading calendar                                      |
| `longbridge security-list HK`                                              | Security list                                         |
| `longbridge participants`                                                  | Market maker list                                     |

**Options & Warrants**

| Command                                          | Description               |
| ------------------------------------------------ | ------------------------- |
| `longbridge option-quote AAPL240119C190000`      | Option quotes             |
| `longbridge option-chain AAPL`                   | Option chain expiry dates |
| `longbridge option-chain AAPL --date 2024-01-19` | Strike prices for a date  |
| `longbridge warrant-quote 12345.HK`              | Warrant quotes            |
| `longbridge warrant-list 700.HK`                 | Warrants for a security   |
| `longbridge warrant-issuers`                     | Warrant issuer list       |

**Watchlist**

| Command                                                           | Description     |
| ----------------------------------------------------------------- | --------------- |
| `longbridge watchlist`                                            | List all groups |
| `longbridge watchlist create "My Portfolio"`                      | Create group    |
| `longbridge watchlist update <id> --add TSLA.US --remove AAPL.US` | Update group    |
| `longbridge watchlist delete <id>`                                | Delete group    |

**Trading**

| Command                                               | Description                                 |
| ----------------------------------------------------- | ------------------------------------------- |
| `longbridge orders`                                   | Today's orders                              |
| `longbridge orders --history [--start 2024-01-01]`    | History orders                              |
| `longbridge order <order_id>`                         | Order detail                                |
| `longbridge executions`                               | Today's executions                          |
| `longbridge buy TSLA.US 100 --price 250`              | Submit buy order (prompts for confirmation) |
| `longbridge sell TSLA.US 100 --price 260`             | Submit sell order                           |
| `longbridge cancel <order_id>`                        | Cancel order                                |
| `longbridge replace <order_id> --qty 200 --price 255` | Modify order                                |
| `longbridge balance`                                  | Account balance                             |
| `longbridge cash-flow [--start 2024-01-01]`           | Cash flow records                           |
| `longbridge positions`                                | Stock positions                             |
| `longbridge fund-positions`                           | Fund positions                              |
| `longbridge margin-ratio TSLA.US`                     | Margin ratio                                |
| `longbridge max-qty TSLA.US --side buy --price 250`   | Max purchase quantity                       |

## API Rate Limits

The Longbridge OpenAPI has rate limiting:

- Maximum 10 API calls per second
- Access tokens are automatically refreshed when expired

## Documentation

- [Longbridge OpenAPI Documentation](https://open.longbridge.com)
- [Rust SDK Documentation](https://longbridge.github.io/openapi/rust/longbridge/)

## License

MIT
