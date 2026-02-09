# Feature Gap Analysis: Internal vs Public Version

**Analysis Date:** 2026-02-09
**Analyzer:** gap-analyzer agent
**Internal Version Path:** `/Users/jason/work/longbridge-terminal-internal/`
**Public Version Path:** `/Users/jason/work/longbridge-terminal/`

---

## Executive Summary

The public version of longbridge-terminal successfully reimplements the core functionality using Longport OpenAPI SDK instead of the internal `engine` library. However, there are several significant feature gaps and implementation differences that need attention.

### Key Findings

- **Core Architecture Change:** Internal → `engine` SDK | Public → `longport` OpenAPI SDK
- **Missing Critical Feature:** QR Code Login (internal only)
- **Code Quality:** Public version has 5.8x more documentation comments (180 vs 31)
- **Dependency Count:** Internal version has 2 private dependencies (`engine`, `engine-signal`)
- **API Call Complexity:** Internal version: 77 `engine::` calls | Public version: 45 `longport::` calls

---

## 1. Critical Feature Gaps

### 1.1 QR Code Login System ⭐⭐⭐⭐⭐ (CRITICAL)

> Skip

**Status:** Missing in public version
**Impact:** High - affects user onboarding experience

**Internal Implementation:**

- File: `src/api/login.rs` (49 lines)
- AppState: `QrCode` state in app.rs
- API endpoints:
  - `/v1/auth/request_scan_login` - Generate QR token
  - `/v1/auth/scan_login` - Complete login after scan
- System: `pub fn qr_code()` in system.rs (line 116)

**Public Implementation:**

- File: `src/api/login.rs` (2 lines - placeholder only)
- Comment: "OpenAPI uses Access Token authentication, no QR code login needed"

**Gap Analysis:**

- Internal version supports mobile app scan-to-login flow
- Public version requires manual Access Token configuration
- User experience significantly degraded for onboarding

**Recommendation:**

```
Priority: HIGH
Effort: Medium (3-5 days)
Action: Investigate if Longport OpenAPI supports QR login flow
Alternative: Provide better token onboarding guide/wizard
```

---

### 1.2 Enhanced WebSocket Integration ⭐⭐⭐⭐

**Status:** Simplified in public version
**Impact:** Medium - affects real-time data reliability

**Internal Implementation:**

- Uses `engine::ws::{BinaryWs, TextWs, ReadyState}`
- Complex topic subscription system via `engine::ws::text::packets::topic`
- Automatic reconnection handling
- Binary protocol support

**Public Implementation:**

- Uses `longport::quote::PushEvent` stream
- File: `src/data/ws.rs` (16 lines - minimal ReadyState enum only)
- Simpler subscription via `QuoteContext::subscribe()`

**Gap Analysis:**

- Internal version has more granular WebSocket state management
- Public version relies entirely on SDK's connection handling
- Less visibility into connection health in public version

**Recommendation:**

```
Priority: MEDIUM
Effort: Low (1-2 days)
Action: Add WebSocket health monitoring UI indicators
Consider: Expose more connection state from longport SDK
```

---

### 1.3 Select Widget Component ⭐⭐⭐

**Status:** Present in internal, used in public
**Impact:** Low - UI component

**Internal Implementation:**

- `Select<T>` widget in `src/widgets/gadget.rs`
- Found in both versions (confirmed via grep)

**Public Implementation:**

- Also present in `src/widgets/gadget.rs`

**Gap Analysis:**

- No gap - both versions have this component
- May have implementation differences (not critical)

---

## 2. Architecture & Dependency Differences

### 2.1 Core SDK Dependency ⭐⭐⭐⭐⭐

| Aspect                  | Internal Version         | Public Version              |
| ----------------------- | ------------------------ | --------------------------- |
| **Primary SDK**         | `engine` (private)       | `longport` (v3.0.7, public) |
| **Secondary SDK**       | `engine-signal`          | None                        |
| **SDK Calls**           | 77 `engine::` references | 45 `longport::` references  |
| **Authentication**      | Multi-method (QR, Token) | Access Token only           |
| **Environment Support** | Canary, Pre, Prod        | Config-based (via .env)     |

**Key Differences:**

```rust
// Internal: main.rs (lines 60-78)
let app = engine::app::AppInfo {
    app_id: engine::app::AppID::Tui,
    env,  // Supports Canary/Pre/Prod
    version: env!("CARGO_PKG_VERSION").into(),
    build: engine::version(),
    // ...
};
engine::HTTP.store(Arc::new(http));

// Public: main.rs (lines 42-48)
let quote_receiver = match openapi::init_contexts().await {
    Ok(receiver) => receiver,
    Err(e) => {
        openapi::print_config_guide();  // User-friendly error
        return;
    }
};
```

---

### 2.2 Data Model Architecture ⭐⭐⭐⭐

**Internal Version:**

- Uses `engine::components::*` modules:
  - `engine::components::stock::{Counter, SubTypes, STOCKS}`
  - `engine::components::user::User`
  - `engine::components::watchlist::api::WatchlistGroup`
  - `engine::components::markets::TradeStatus`
  - `engine::components::quote::LineData`

**Public Version:**

- Custom `src/data/` module (1,292 lines total):
  - `data/stock.rs` (149 lines)
  - `data/types.rs` (730 lines) - extensive type definitions
  - `data/stocks.rs` (76 lines) - DashMap-based cache
  - `data/user.rs` (170 lines)
  - `data/watchlist.rs` (138 lines)
  - `data/ws.rs` (16 lines)

**Gap Analysis:**

- Public version has fully reimplemented data models
- Uses `DashMap` for global stock cache instead of engine's `STOCKS`
- More explicit type definitions (may be more maintainable)

**Code Comparison:**

```rust
// Internal: uses engine's built-in cache
use engine::components::stock::STOCKS;
let stock = STOCKS.get(&counter);

// Public: custom DashMap implementation
use crate::data::STOCKS;
pub static STOCKS: Lazy<DashMap<Counter, RwLock<Stock>>> = ...;
let stock = STOCKS.get(&counter);
```

---

### 2.3 Dependencies Analysis

**Internal Version (Cargo.toml):**

```toml
engine = { path = "../engine/engine", features = [...] }
engine-signal = { path = "../engine/engine-signal" }
tracing-appender = { git = "https://git.5th.im/...", features = ["brotli"] }
ansi-to-tui = "3.1.0"
```

**Public Version (Cargo.toml):**

```toml
longport = "3.0.7"
dashmap = "6.1"
tokio-stream = "0.1"
dirs = "5.0"
arrayvec = "0.7"
ansi-parser = "0.9.1"
ansi-to-tui = "8.0.1"  # Newer version
tracing-appender = { version = "0.2.4" }  # Public version
```

**Key Differences:**

- Public version uses **newer versions** of some crates
- Added `dashmap` for concurrent HashMap (replaces engine's cache)
- Added `dirs` for cross-platform directory paths
- Upgraded `ansi-to-tui` from 3.1.0 → 8.0.1
- Uses public `tracing-appender` instead of custom fork

---

## 3. API & Implementation Differences

### 3.1 Account Management ⭐⭐⭐⭐

**Internal Version (`api/account.rs`):**

- 52 lines
- Direct HTTP API calls to internal endpoints
- Full currency list API: `/v1/portfolio/currency/list`
- Precise data structures matching backend

**Public Version (`api/account.rs`):**

- 306 lines (5.9x larger!)
- Uses Longport SDK: `ctx.account_balance(None).await`
- **Hardcoded currencies** (HKD, USD, CNY) - no live API
- Extensive mapping/conversion layer
- Error handling for missing trading permissions

**Gap:**

```rust
// Internal: Live API call
pub async fn currencies(
    transport: &HttpClient,
    account_channel: &str,
) -> Result<Vec<CurrencyInfo>, ApiError> {
    let response = transport
        .get("/v1/portfolio/currency/list", ...)
        .await?;
    // Returns live data
}

// Public: Hardcoded fallback
pub async fn currencies(_account_channel: &str) -> Result<Vec<CurrencyInfo>> {
    Ok(vec![
        CurrencyInfo { currency: "HKD", ... },
        CurrencyInfo { currency: "USD", ... },
        CurrencyInfo { currency: "CNY", ... },
    ])
}
```

**Impact:** Public version cannot show user's actual supported currencies dynamically.

---

### 3.2 Stock Search ⭐⭐⭐

**Internal Version (`api/search.rs`):**

- 66 lines
- Uses internal search API
- Returns structured `ProductList`

**Public Version (`api/search.rs`):**

- 83 lines (26% larger)
- Uses Longport OpenAPI `symbol_search`
- More defensive error handling

**Gap:** Minor - both work, public version is slightly more verbose.

---

### 3.3 Quote Data Handling ⭐⭐⭐⭐

**Internal Version (`api/quote.rs`):**

- 22 lines
- Simple wrapper around engine API
- Direct access to internal endpoints

**Public Version (`api/quote.rs`):**

- 19 lines
- Uses `longport::quote::QuoteContext`
- Simpler due to SDK abstraction

**Code Comparison:**

```rust
// Internal
pub async fn stock_quote(transport: &HttpClient, counter: &Counter) -> Result<StockQuote> {
    let response = transport
        .get("/v2/market/quote", ...)
        .await?;
    // Manual deserialization
}

// Public
pub async fn fetch_quote(symbol: &str) -> Result<SecurityQuote> {
    let ctx = crate::openapi::quote();
    let quotes = ctx.quote([symbol]).await?;
    Ok(quotes.into_iter().next()...)
}
```

**Impact:** Public version is cleaner but has less control over raw data.

---

## 4. UI/UX Differences

### 4.1 ANSI Rendering ⭐⭐⭐

**Internal Version:**

- Uses `ansi-to-tui = "3.1.0"`
- Simple `logo.into_text().unwrap()` conversion

**Public Version:**

- Uses `ansi-parser = "0.9.1"` + `ansi-to-tui = "8.0.1"`
- Custom `center_ansi()` function for better text centering
- More sophisticated ANSI handling (110 lines vs 34 lines in logo.rs)

**Gap:** Public version has **better ANSI rendering** - this is an improvement!

---

### 4.2 Assets ⭐⭐

**Internal Version:**

- `logo.ascii` (12KB)
- `logo.png` (563 bytes) - ⚠️ PNG file present
- `banner.txt` (119 bytes)

**Public Version:**

- `logo.ascii` (12KB)
- `banner.txt` (118 bytes)
- Missing `logo.png`

**Gap:** PNG logo missing in public version (likely unused, low priority).

---

### 4.3 Locale Differences ⭐⭐⭐

**Internal Version:**

- 186 lines (en.yml)
- Stock index format: `IX/US/.DJI`, `IX/HK/HSI`, etc.
- Missing `TradeStatus` translations
- Missing `watchlist_group` translations

**Public Version:**

- 208 lines (en.yml) - **22 more lines**
- Stock index format: `.DJI.US`, `HSI.HK`, etc. (different convention)
- **Added TradeStatus translations:**
  ```yaml
  TradeStatus.TRADING: Trading
  TradeStatus.US_PREV: Pre-Market
  TradeStatus.US_AFTER: After Hours
  TradeStatus.NOON_CLOSING: Lunch Break
  TradeStatus.Halted: Halted
  # ... 10+ more states
  ```
- **Added watchlist_group translations:**
  ```yaml
  watchlist_group.all: "ALL"
  watchlist_group.holdings: "HOLDINGS"
  watchlist_group.us: "US"
  # ... 8 more markets
  ```

**Impact:** Public version has **better i18n coverage** - this is an improvement!

---

## 5. Code Quality & Documentation

### 5.1 Documentation ⭐⭐⭐⭐⭐

| Metric                   | Internal | Public    | Improvement |
| ------------------------ | -------- | --------- | ----------- |
| **Doc Comments (`///`)** | 31       | 180       | **+481%**   |
| **README.md**            | ❌ None  | ✅ 1.4KB  | ✅          |
| **CLAUDE.md**            | ❌ None  | ✅ 5.9KB  | ✅          |
| **Code Comments**        | Minimal  | Extensive | ✅          |

**Examples of Public Version's Better Docs:**

```rust
// Public: src/openapi/context.rs
/// Initialize Longport SDK contexts (QuoteContext and TradeContext)
///
/// This function:
/// 1. Loads credentials from environment variables or .env file
/// 2. Creates QuoteContext (for market data) and TradeContext (for trading)
/// 3. Returns a receiver for real-time quote push events
///
/// # Errors
/// Returns error if credentials are missing or SDK initialization fails
pub async fn init_contexts() -> Result<impl Stream<Item = PushEvent>> {
    // ...
}
```

**Impact:** Public version is significantly **more maintainable** and **open-source friendly**.

---

### 5.2 Error Handling ⭐⭐⭐⭐

**Public Version Improvements:**

- User-friendly config guide when credentials missing
- Graceful handling of missing trading permissions
- Non-blocking error handling (warns instead of crashing)

**Example:**

```rust
// Public: src/api/account.rs (lines 17-25)
match ctx.account_balance(None).await {
    Ok(_balance) => {
        tracing::info!("Successfully fetched account balance");
    }
    Err(e) => {
        tracing::warn!("Failed to fetch account balance (may lack trading permission): {}", e);
        // Continue execution, do not block app startup
    }
}
```

**Impact:** Public version is **more robust** for different user scenarios.

---

## 6. System Logic Differences

### 6.1 App State Machine ⭐⭐⭐⭐

**Internal Version (`AppState` enum):**

```rust
pub enum AppState {
    Error,
    Loading,
    QrCode,        // ⬅️ Unique to internal
    TradeToken,
    Portfolio,
    Stock,
    Watchlist,
    WatchlistStock,
}
```

**Public Version (`AppState` enum):**

```rust
pub enum AppState {
    Error,
    Loading,
    // QrCode state removed
    TradeToken,
    Portfolio,
    Stock,
    Watchlist,
    WatchlistStock,
}
```

**Impact:** Removal of `QrCode` state reflects simplified authentication flow.

---

### 6.2 System Functions ⭐⭐⭐

**File Sizes:**

- Internal: `system.rs` - 1,767 lines
- Public: `system.rs` - 1,979 lines (+212 lines, +12%)

**Key Differences:**

- Public version has more inline documentation
- Public version has expanded `TradeStatus` rendering
- Both have similar function count and structure

**Function Count Comparison:**

- Internal: ~25 major functions
- Public: ~25 major functions
- Similar complexity, public has more comments

---

## 7. Performance & Optimization

### 7.1 Dependency Version Updates ⭐⭐⭐

Public version uses **newer, potentially faster** crates:

- `ansi-to-tui`: 3.1.0 → 8.0.1 (major upgrade, likely bug fixes)
- `tracing-appender`: git fork → 0.2.4 (stable release)

**Impact:** Public version may have better performance and stability.

---

### 7.2 Data Structures ⭐⭐⭐

**Internal:** Uses `engine`'s built-in `STOCKS` cache (opaque implementation)

**Public:** Uses `DashMap<Counter, RwLock<Stock>>` - explicit concurrent hashmap

```rust
pub static STOCKS: Lazy<DashMap<Counter, RwLock<Stock>>> =
    Lazy::new(DashMap::new);
```

**Impact:**

- Public version is more transparent
- `DashMap` is battle-tested for concurrent access
- May have different performance characteristics

---

## 8. Missing Features Summary

### High Priority (Implement Soon)

1. **QR Code Login** ⭐⭐⭐⭐⭐
   - Effort: Medium (3-5 days)
   - Impact: High (user onboarding)
   - Action: Research Longport OpenAPI capabilities or provide alternative

2. **Dynamic Currency List** ⭐⭐⭐⭐
   - Effort: Low (1-2 days)
   - Impact: Medium (accuracy)
   - Action: Find equivalent API in Longport SDK or document limitation

3. **WebSocket Health Monitoring** ⭐⭐⭐⭐
   - Effort: Low (1 day)
   - Impact: Medium (UX)
   - Action: Add visual indicator for connection state

### Medium Priority (Consider for v2)

4. **Multi-Environment Support** ⭐⭐⭐
   - Effort: Medium (2-3 days)
   - Impact: Low (dev experience only)
   - Action: Add `--env` flag for Canary/Pre environments

5. **Enhanced User Session** ⭐⭐⭐
   - Effort: Medium (2-3 days)
   - Impact: Medium (feature parity)
   - Action: Implement persistent session using `dirs` crate

### Low Priority (Nice to Have)

6. **Logo PNG Asset** ⭐⭐
   - Effort: Trivial
   - Impact: None (unused)
   - Action: Add if needed for branding

7. **Binary WebSocket Protocol** ⭐⭐
   - Effort: High (5+ days)
   - Impact: Low (SDK handles this)
   - Action: Only if performance issues arise

---

## 9. Improvements in Public Version ✅

### What Public Version Does Better:

1. **Documentation** ⭐⭐⭐⭐⭐
   - 5.8x more doc comments
   - Comprehensive README and CLAUDE.md
   - Better code comments

2. **Error Handling** ⭐⭐⭐⭐
   - User-friendly error messages
   - Non-blocking permission failures
   - Config guide on startup errors

3. **Locale Coverage** ⭐⭐⭐⭐
   - 22 more translation strings
   - Better TradeStatus translations
   - Watchlist group labels

4. **ANSI Rendering** ⭐⭐⭐
   - Better text centering
   - More sophisticated color handling
   - Custom center_ansi() function

5. **Type Safety** ⭐⭐⭐
   - Explicit data structures (730 lines in types.rs)
   - More granular error types
   - Better separation of concerns

6. **Open Source Readiness** ⭐⭐⭐⭐⭐
   - No proprietary dependencies
   - Public SDK (longport 3.0.7)
   - MIT-friendly licenses

---

## 10. Implementation Roadmap

### Phase 1: Critical Gaps (Week 1-2)

```markdown
[ ] Task 1.1: Research Longport QR Login Support - Contact Longport to inquire about OAuth/QR flow - If supported: implement QR login (3 days) - If not: create interactive token setup wizard (2 days)

[ ] Task 1.2: Implement WebSocket Health Indicator - Add ReadyState monitoring UI (1 day) - Add reconnection toast notifications (1 day)

[ ] Task 1.3: Fix Currency List API - Research Longport SDK for currency endpoint (0.5 day) - Implement or document limitation (0.5 day)
```

### Phase 2: Feature Parity (Week 3-4)

```markdown
[ ] Task 2.1: Add Environment Switching - Add --env CLI flag (1 day) - Document usage in README (0.5 day)

[ ] Task 2.2: Enhance Session Management - Use `dirs` crate for config storage (1 day) - Implement persistent token cache (1 day)

[ ] Task 2.3: Add Login State Persistence - Save/load user session (1 day) - Implement automatic token refresh (1 day)
```

### Phase 3: Polish (Week 5)

```markdown
[ ] Task 3.1: Add Missing Assets - Export logo.png if needed (0.25 day) - Verify all assets render correctly (0.25 day)

[ ] Task 3.2: Performance Testing - Benchmark DashMap vs engine STOCKS (0.5 day) - Profile WebSocket handling (0.5 day)

[ ] Task 3.3: Documentation Review - Update README with all features (0.5 day) - Add troubleshooting guide (0.5 day)
```

---

## 11. Risk Assessment

### High Risk Items

1. **QR Login Unavailable**
   - Risk: Longport OpenAPI may not support QR flow
   - Mitigation: Prepare excellent onboarding docs for token setup
   - Contingency: Build web-based token management UI

2. **Currency API Missing**
   - Risk: No equivalent endpoint in public API
   - Mitigation: Hardcoded list may be sufficient for MVP
   - Contingency: Add manual currency configuration

### Medium Risk Items

3. **WebSocket Reliability**
   - Risk: Less control over connection vs internal SDK
   - Mitigation: Trust Longport SDK's reconnection logic
   - Contingency: Add manual reconnect button

4. **Performance Difference**
   - Risk: DashMap may perform differently than engine cache
   - Mitigation: Both are well-optimized
   - Contingency: Profile and optimize if needed

---

## 12. Recommendations

### Immediate Actions (This Sprint)

1. ✅ **Accept Trade-offs:**
   - QR login → Token-based auth is acceptable for dev-focused tool
   - Hardcoded currencies → Document as known limitation
   - Simplified WebSocket → Trust SDK's implementation

2. 🔧 **Quick Wins:**
   - Add WebSocket status indicator (1 day)
   - Improve token setup UX with better guide (1 day)
   - Document all known limitations in README (0.5 day)

3. 📚 **Documentation:**
   - Create MIGRATION.md for internal users (1 day)
   - Add LIMITATIONS.md explaining API differences (0.5 day)
   - Expand troubleshooting section (0.5 day)

### Long-term Strategy

1. **Partner with Longport:**
   - Request feature parity for QR login
   - Ask for currency list API access
   - Propose enhancements to public SDK

2. **Community Engagement:**
   - Open-source the project on GitHub
   - Accept contributions for missing features
   - Build plugin system for custom data sources

3. **Maintain Feature Parity:**
   - Regular sync reviews with internal version
   - Automated diff checking (CI/CD)
   - Version compatibility matrix

---

## 13. Conclusion

### Overall Assessment: ✅ **SUCCESSFUL MIGRATION**

**Strengths of Public Version:**

- ✅ Successfully decoupled from proprietary `engine` SDK
- ✅ Significantly better documentation and code quality
- ✅ More robust error handling
- ✅ Better internationalization
- ✅ Open-source ready with public dependencies

**Acceptable Trade-offs:**

- ⚠️ QR login → Token auth (reasonable for dev tool)
- ⚠️ Hardcoded currencies → Low impact for MVP
- ⚠️ Less WebSocket control → SDK is reliable

**Action Items:**

- 🔴 HIGH: Add WebSocket health indicator
- 🔴 HIGH: Improve token onboarding UX
- 🟡 MEDIUM: Document known limitations
- 🟢 LOW: Research future Longport API features

### Recommendation: **APPROVE FOR RELEASE**

The public version achieves the primary goal of removing internal dependencies while **improving** code quality and maintainability. The missing features are either low-impact or have acceptable workarounds. With the quick wins implemented, this is ready for public release.

---

**Generated by:** gap-analyzer agent
**Analysis Duration:** ~15 minutes
**Files Compared:** 150+ source files
**Lines Analyzed:** ~8,000 lines of code
