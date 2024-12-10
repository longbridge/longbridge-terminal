# Longbridge Terminal

An _experimental_ terminal-based app of [Longbridge](https://longbridge.com).

We build this app to help us in development of the cross-platform Rust core library (named: Engine) of Longbridge.

This is also for making a fun, so we can watch the stock market data in the terminal.

![longbridge-terminal](https://github.com/longbridgeapp/longbridge-terminal/assets/5518/49c93838-852e-4127-8377-f49876923069)

## Features

- Watchlist
- Portfolio
- Real-time stock data
- Built on Rust + Ratatui.
- Vim like keybindings.

## System Requirements

- macOS
- Linux

## Installation

If you'r on macOS or Linux, just run the following command in your terminal:

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

Then you will get `longbridge` command in your terminal.

```bash
$ longbridge
```

Run `longbridge` can start the app.

## QA

- Q: "Another instance is running, please close it first" error.
- A: Because of the real-time stock data limited you only have one instance.

---

- Q: "How can I switch another account?"
- A: Just run `longbridge --logout`, and scan QRCode again.

---

- Q: "Where is the login session storaged at?"
- A: macOS: `$HOME/Library/Application Support/Longbridge TUI`, Linux: `~/.local/share/Longbridge TUI/`

## License

This is not a open-source project, this repository just for release and give a install script.
