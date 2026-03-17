# Watchlist Commands Reference

Watchlists are organized as named groups, each containing a list of securities.

## List All Watchlists

```bash
longbridge watchlist [--format json]
```

Returns all groups with their contained securities.

---

## Show a Specific Watchlist Group

```bash
longbridge watchlist show <ID|NAME> [--format json]
```

Filter by numeric group ID or group name (case-insensitive).

```bash
longbridge watchlist show 123
longbridge watchlist show "Tech Stocks"
longbridge watchlist show tech stocks   # case-insensitive name match
```

Output: the group header and its securities table (or JSON object).

---

## Create a Watchlist Group

```bash
longbridge watchlist create <NAME>
```

```bash
longbridge watchlist create "My Portfolio"
longbridge watchlist create "Tech Stocks"
```

Output: `Created watchlist group 'NAME' with ID: {id}`

Use the returned ID for subsequent update/delete operations.

---

## Update a Watchlist Group

```bash
longbridge watchlist update <ID> [--name <NAME>] [--add <SYMBOL>] [--remove <SYMBOL>] [--mode <MODE>]
```

- `--name`: Rename the group
- `--add`: Add a security (repeatable)
- `--remove`: Remove a security (repeatable)
- `--mode`: `add` (default), `remove`, or `replace` (replace all securities with `--add` list)

```bash
# Add securities
longbridge watchlist update 123 --add TSLA.US --add AAPL.US --add NVDA.US

# Remove a security
longbridge watchlist update 123 --remove 700.HK

# Rename
longbridge watchlist update 123 --name "US Tech"

# Replace entire list
longbridge watchlist update 123 --add TSLA.US --add AAPL.US --mode replace

# Rename and add in one command
longbridge watchlist update 123 --name "Core Holdings" --add MSFT.US
```

---

## Delete a Watchlist Group

```bash
longbridge watchlist delete <ID> [--purge]
```

- `--purge`: Also delete all securities in the group
- Prompts for confirmation before deleting

```bash
longbridge watchlist delete 123
longbridge watchlist delete 123 --purge
```

---

## Typical Workflow

```bash
# 1. See existing watchlists
longbridge watchlist

# 2. Create a new group
longbridge watchlist create "AI Stocks"
# → Created watchlist group 'AI Stocks' with ID: 456

# 3. Add securities
longbridge watchlist update 456 --add NVDA.US --add MSFT.US --add GOOGL.US

# 4. Verify
longbridge watchlist

# 5. Remove a stock
longbridge watchlist update 456 --remove GOOGL.US

# 6. Clean up
longbridge watchlist delete 456 --purge
```
