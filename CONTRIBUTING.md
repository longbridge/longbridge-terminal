# Contributing to Longbridge Terminal

Thank you for your interest in contributing to Longbridge Terminal! This document provides guidelines and instructions for contributing.

## Getting Started

### Prerequisites

- Rust toolchain (latest stable version)
- Longbridge OpenAPI credentials ([Get them here](https://open.longbridge.com))
- macOS or Linux

### Setup Development Environment

1. **Clone the repository**:

   ```bash
   git clone https://github.com/longbridge/longbridge-terminal.git
   cd longbridge-terminal
   ```

2. **Build and run**:
   ```bash
   cargo run
   ```

## Code Style and Guidelines

### Language Requirements

**IMPORTANT**: All code comments and documentation MUST be written in English only.

- ❌ **Never** write Chinese or other non-English text in code comments
- ❌ **Never** hardcode Chinese strings directly in code
- ✅ Use `rust-i18n` (`t!` macro) for all user-facing text
- ✅ All locale strings must be defined in `locales/*.yml` files

**Example**:

```rust
// ✅ Good: English comment with i18n
let status = t!("TradeStatus.Normal");

// ❌ Bad: Chinese comment or hardcoded string
// let status = "交易中";
```

### Naming Conventions

- **Types**: `UpperCamelCase` (e.g., `QuoteData`, `TradeStatus`)
- **Functions and variables**: `snake_case` (e.g., `update_from_quote`, `stock_count`)
- **Constants**: `SCREAMING_SNAKE_CASE` (e.g., `STOCKS`, `DEFAULT_TIMEOUT`)

### Clippy Rules

This project uses strict `clippy::pedantic` rules. Run the following before submitting:

```bash
cargo fmt && cargo clippy
```

The following pedantic rules are allowed (you don't need to fix them):

- `cast_possible_truncation`
- `ignored_unit_patterns`
- `implicit_hasher`
- `missing_errors_doc` / `missing_panics_doc`
- `module_name_repetitions`
- `must_use_candidate`
- `needless_pass_by_value`
- `too_many_arguments` / `too_many_lines`

### Code Formatting

Format your code with:

```bash
cargo fmt
```

## Adding Translations

When adding new user-facing text:

1. **Add the translation key to all locale files**:
   - `locales/en.yml` (English)
   - `locales/zh-CN.yml` (Simplified Chinese)
   - `locales/zh-HK.yml` (Traditional Chinese)

2. **Use the `t!` macro in code**:

   ```rust
   use rust_i18n::t;

   let message = t!("your.translation.key");
   ```

**Example**:

```yaml
# locales/en.yml
Portfolio:
  TotalAssets: "Total Assets"

# locales/zh-CN.yml
Portfolio:
  TotalAssets: "总资产"

# locales/zh-HK.yml
Portfolio:
  TotalAssets: "總資產"
```

## Architecture Overview

### Key Components

- **`src/openapi/`**: Longbridge OpenAPI integration layer
  - `context.rs`: Global QuoteContext and TradeContext management
- **`src/data/`**: Data models and global state
  - `stocks.rs`: Global stock cache using DashMap
- **`src/app.rs`**: Main application loop using Bevy ECS
- **`src/system.rs`**: UI rendering and user input handling
- **`src/widgets/`** and **`src/views/`**: UI components

### Data Flow

```
Initialization → Subscribe Quotes → WebSocket Push → Update Cache → Render UI
```

For more details, see [CLAUDE.md](./CLAUDE.md).

## Pull Request Process

1. **Fork the repository** and create a new branch:

   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Make your changes** following the code style guidelines

3. **Run checks**:

   ```bash
   cargo fmt && cargo clippy
   cargo build
   ```

4. **Commit your changes**:
   - Write clear, descriptive commit messages in English
   - Reference issue numbers if applicable

5. **Push and create a Pull Request**:
   - Provide a clear description of the changes
   - Explain why the changes are needed
   - Include screenshots for UI changes

6. **Address review feedback** if requested

## Commit and PR Title Conventions

Use a prefix to indicate the area of change. The word after the colon must be **capitalized**.

- `cli:` — changes to CLI commands (`src/cli/`) or shared infrastructure (`src/openapi/`, `src/region.rs`, `src/auth.rs`, etc.)
- `tui:` — changes that touch TUI-specific code (`src/tui/app.rs`, `src/tui/views/`, `src/tui/widgets/`, `src/tui/systems/`, etc.)
- `chore:` — other changes that don't fit the above (e.g. docs, formatting, refactors that don't modify behavior)

Only use `tui:` when the diff actually modifies TUI files. Changes to shared modules that happen to be triggered by a TUI bug should still use `cli:` or a more specific prefix.

**Examples**: `cli: Add statement export command`, `tui: Fix quit confirmation dialog`

## Development Tips

### Using Ratatui

This project uses [Ratatui](https://ratatui.rs/) for the TUI. For Ratatui-specific questions, refer to:

- [Ratatui Documentation](https://ratatui.rs/)
- [Ratatui Examples](https://github.com/ratatui-org/ratatui/tree/main/examples)

### Longbridge API

- **Rate Limit**: Maximum 10 API calls per second
- **Token Refresh**: The SDK automatically refreshes access tokens — no manual renewal needed
- **Documentation**: [Longbridge OpenAPI Docs](https://open.longbridge.com)
- **Rust SDK**: [SDK Documentation](https://longbridge.github.io/openapi/rust/longbridge/)

### Debugging

Enable debug logging:

```bash
RUST_LOG=debug cargo run
```

## Questions or Issues?

- **Bug Reports**: Open an issue with detailed reproduction steps
- **Feature Requests**: Open an issue describing the feature and use case
- **Questions**: Check existing issues or open a new discussion

## Code of Conduct

- Be respectful and inclusive
- Provide constructive feedback
- Focus on what is best for the community

Thank you for contributing! 🎉
