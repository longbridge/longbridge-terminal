# Longbridge Terminal

An _experimental_ terminal-based stock trading app built with [Longport OpenAPI](https://open.longbridge.com).

A Rust-based TUI (Terminal User Interface) for monitoring market data and managing stock portfolios. Built to showcase the capabilities of the Longport OpenAPI SDK.

<img width="1601" height="1155" alt="image" src="https://github.com/user-attachments/assets/3b853eba-658c-4ee8-a626-ba03170e6b28" />

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
- Longport OpenAPI credentials (free to obtain)

## Installation

### From Binary

If you're on macOS or Linux, run the following command in your terminal:

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

This will install the `longbridge` command in your terminal.

## Configuration

Before running the app, you need to configure your Longport OpenAPI credentials:

1. **Get API Credentials**: Visit [Longport Open Platform](https://open.longbridge.com) to create an application and obtain:
   - `APP_KEY`
   - `APP_SECRET`
   - `ACCESS_TOKEN`

2. **Configure Environment Variables**:

   Create a `.env` file in the project root:

   ```bash
   cp .env.example .env
   ```

   Edit `.env` and add your credentials:

   ```bash
   LONGPORT_APP_KEY=your_app_key
   LONGPORT_APP_SECRET=your_app_secret
   LONGPORT_ACCESS_TOKEN=your_access_token
   ```

   Alternatively, export them as environment variables:

   ```bash
   export LONGPORT_APP_KEY=your_app_key
   export LONGPORT_APP_SECRET=your_app_secret
   export LONGPORT_ACCESS_TOKEN=your_access_token
   ```

3. **Run the App**:

   ```bash
   longbridge
   ```

## API Rate Limits

The Longport OpenAPI has rate limiting:

- Maximum 10 API calls per second
- Access tokens expire every 3 months and need to be renewed

## Documentation

- [Longport OpenAPI Documentation](https://open.longbridge.com)
- [Rust SDK Documentation](https://longportapp.github.io/openapi/rust/longport/)

## License

MIT
