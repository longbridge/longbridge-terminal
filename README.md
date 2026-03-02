# Longbridge Terminal

An _experimental_ terminal-based stock trading app built with [Longbridge OpenAPI](https://open.longbridge.com).

A Rust-based TUI (Terminal User Interface) for monitoring market data and managing stock portfolios. Built to showcase the capabilities of the Longbridge OpenAPI SDK.

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## Features

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

## API Rate Limits

The Longbridge OpenAPI has rate limiting:

- Maximum 10 API calls per second
- Access tokens are automatically refreshed when expired

## Documentation

- [Longbridge OpenAPI Documentation](https://open.longbridge.com)
- [Rust SDK Documentation](https://longportapp.github.io/openapi/rust/longport/)

## License

MIT
