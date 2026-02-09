# 渲染优化方案

## 当前问题

### 现状分析

```rust
// 当前实现 (src/app.rs:249-263)
let mut needs_render = true;

loop {
    tokio::select! {
        _ = render_tick.tick() => {
            if needs_render {
                app.update();  // 全屏重绘
                needs_render = false;
            }
        }
        Some(mut cmd) = update_rx.recv() => {
            cmd.apply(&mut app.world);
            needs_render = true;  // 任何更新都触发渲染
        }
        Some(push_event) = quote_receiver.next() => {
            // 更新股票数据
            needs_render = true;  // 触发渲染
        }
    }
}
```

**问题：**
1. ❌ **过度渲染**：任何数据更新都触发全屏重绘
2. ❌ **无差异检测**：不知道具体哪些数据变化了
3. ❌ **浪费资源**：即使只更新一个股票也重绘整个界面
4. ❌ **无优先级**：所有更新同等对待

---

## 方案设计

### 方案 A: 脏标记系统（Dirty Flag System）

**核心思想**：追踪具体哪些组件需要更新

```rust
// src/app.rs - 新增脏标记系统
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct DirtyFlags: u32 {
        const WATCHLIST     = 0b0000_0001;  // 自选股列表
        const STOCK_DETAIL  = 0b0000_0010;  // 股票详情
        const PORTFOLIO     = 0b0000_0100;  // 投资组合
        const QUOTE         = 0b0000_1000;  // 实时行情
        const DEPTH         = 0b0001_0000;  // 盘口数据
        const KLINE         = 0b0010_0000;  // K线图
        const TRADES        = 0b0100_0000;  // 成交明细
        const NAVBAR        = 0b1000_0000;  // 导航栏

        const ALL           = 0xFFFF_FFFF;
    }
}

pub struct RenderState {
    dirty: DirtyFlags,
    last_render: std::time::Instant,
}

impl RenderState {
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlags::ALL,  // 初始全部标记为脏
            last_render: std::time::Instant::now(),
        }
    }

    /// Mark specific components as dirty
    pub fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty |= flags;
    }

    /// Check if any component is dirty
    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Check if specific component is dirty
    pub fn is_dirty_component(&self, flags: DirtyFlags) -> bool {
        self.dirty.contains(flags)
    }

    /// Clear all dirty flags
    pub fn clear_dirty(&mut self) {
        self.dirty = DirtyFlags::empty();
        self.last_render = std::time::Instant::now();
    }

    /// Clear specific dirty flags
    pub fn clear_component(&mut self, flags: DirtyFlags) {
        self.dirty.remove(flags);
    }
}
```

**使用示例：**

```rust
// 主循环改造
let mut render_state = RenderState::new();

loop {
    tokio::select! {
        _ = render_tick.tick() => {
            if render_state.is_dirty() {
                // 只渲染脏组件
                app.update_selective(&render_state);
                render_state.clear_dirty();
            }
        }
        Some(mut cmd) = update_rx.recv() => {
            cmd.apply(&mut app.world);
            // 精确标记哪些组件需要更新
            match cmd_type {
                CommandType::StateChange => render_state.mark_dirty(DirtyFlags::ALL),
                CommandType::StockSelect => render_state.mark_dirty(DirtyFlags::STOCK_DETAIL),
                CommandType::AccountSwitch => render_state.mark_dirty(DirtyFlags::PORTFOLIO),
            }
        }
        Some(push_event) = quote_receiver.next() => {
            match push_event.detail {
                PushEventDetail::Quote(_) => {
                    // 只标记行情组件为脏
                    render_state.mark_dirty(DirtyFlags::QUOTE | DirtyFlags::WATCHLIST);
                }
                PushEventDetail::Depth(_) => {
                    render_state.mark_dirty(DirtyFlags::DEPTH);
                }
                PushEventDetail::Trade(_) => {
                    render_state.mark_dirty(DirtyFlags::TRADES);
                }
            }
        }
    }
}
```

---

### 方案 B: Ratatui StatefulWidget + 状态对比

**核心思想**：利用 Ratatui 的状态管理和增量更新

```rust
// src/ui/stateful.rs - 新增有状态的组件系统
use ratatui::widgets::StatefulWidget;
use std::hash::{Hash, Hasher};

/// Trait for components that can detect changes
pub trait StatefulComponent {
    type State: Clone + PartialEq;

    /// Get current state snapshot
    fn snapshot(&self) -> Self::State;

    /// Check if state has changed
    fn has_changed(&self, old_state: &Self::State) -> bool {
        self.snapshot() != *old_state
    }
}

/// Component state manager
pub struct ComponentStateManager<T: StatefulComponent> {
    last_state: Option<T::State>,
}

impl<T: StatefulComponent> ComponentStateManager<T> {
    pub fn new() -> Self {
        Self { last_state: None }
    }

    /// Check if component needs rendering
    pub fn should_render(&mut self, component: &T) -> bool {
        let current = component.snapshot();

        match &self.last_state {
            None => {
                self.last_state = Some(current);
                true  // First render
            }
            Some(last) => {
                if current != *last {
                    self.last_state = Some(current);
                    true  // State changed
                } else {
                    false  // No change
                }
            }
        }
    }
}

/// Example: Watchlist state
#[derive(Clone, PartialEq)]
pub struct WatchlistState {
    symbols: Vec<String>,
    quotes: Vec<(Decimal, Decimal)>,  // (price, change)
    selected_index: usize,
}

/// Example: Stock detail state
#[derive(Clone, PartialEq)]
pub struct StockDetailState {
    symbol: String,
    quote: QuoteSnapshot,
    depth: DepthSnapshot,
}
```

**使用示例：**

```rust
// src/app.rs
struct AppStateManagers {
    watchlist: ComponentStateManager<WatchlistComponent>,
    stock_detail: ComponentStateManager<StockDetailComponent>,
    portfolio: ComponentStateManager<PortfolioComponent>,
}

// 在渲染循环中
fn render_selective(app: &mut App, managers: &mut AppStateManagers) {
    let mut frame = terminal.get_frame();

    // 只渲染变化的组件
    if managers.watchlist.should_render(&watchlist_component) {
        render_watchlist(&mut frame, &watchlist_component);
    }

    if managers.stock_detail.should_render(&stock_detail_component) {
        render_stock_detail(&mut frame, &stock_detail_component);
    }
}
```

---

### 方案 C: 增量更新队列（推荐综合方案）

**核心思想**：结合脏标记和状态对比，使用更新队列

```rust
// src/render/mod.rs - 新的渲染模块
pub mod queue;
pub mod state;
pub mod manager;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Update event types
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    /// Quote data updated for specific symbols
    Quote { symbols: Vec<String> },

    /// Depth data updated
    Depth { symbol: String },

    /// Trade data updated
    Trades { symbol: String },

    /// State changed (require full redraw)
    StateChange { new_state: AppState },

    /// User input (high priority)
    UserInput,

    /// Periodic refresh (low priority)
    PeriodicRefresh,
}

impl UpdateEvent {
    /// Get priority (higher = more urgent)
    pub fn priority(&self) -> u8 {
        match self {
            UpdateEvent::UserInput => 100,
            UpdateEvent::StateChange { .. } => 80,
            UpdateEvent::Quote { .. } => 50,
            UpdateEvent::Depth { .. } => 40,
            UpdateEvent::Trades { .. } => 30,
            UpdateEvent::PeriodicRefresh => 10,
        }
    }
}

/// Render manager with update queue
pub struct RenderManager {
    /// Pending update events
    update_queue: VecDeque<UpdateEvent>,

    /// Dirty flags for components
    dirty: DirtyFlags,

    /// Last render time
    last_render: Instant,

    /// Minimum render interval (avoid excessive redraws)
    min_interval: Duration,

    /// Component state managers
    states: ComponentStates,
}

impl RenderManager {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            update_queue: VecDeque::new(),
            dirty: DirtyFlags::ALL,
            last_render: Instant::now(),
            min_interval,
            states: ComponentStates::default(),
        }
    }

    /// Push an update event
    pub fn push_update(&mut self, event: UpdateEvent) {
        // Map event to dirty flags
        match &event {
            UpdateEvent::Quote { .. } => {
                self.dirty |= DirtyFlags::QUOTE | DirtyFlags::WATCHLIST;
            }
            UpdateEvent::Depth { .. } => {
                self.dirty |= DirtyFlags::DEPTH;
            }
            UpdateEvent::Trades { .. } => {
                self.dirty |= DirtyFlags::TRADES;
            }
            UpdateEvent::StateChange { .. } => {
                self.dirty = DirtyFlags::ALL;
            }
            UpdateEvent::UserInput => {
                self.dirty = DirtyFlags::ALL;
            }
            UpdateEvent::PeriodicRefresh => {
                // Low priority, don't mark as dirty yet
            }
        }

        // Insert based on priority
        let priority = event.priority();
        let pos = self.update_queue
            .iter()
            .position(|e| e.priority() < priority)
            .unwrap_or(self.update_queue.len());
        self.update_queue.insert(pos, event);
    }

    /// Check if should render now
    pub fn should_render(&self) -> bool {
        if !self.dirty.is_empty() {
            // Has dirty components
            if self.last_render.elapsed() >= self.min_interval {
                return true;
            }
        }
        false
    }

    /// Process update queue and render if needed
    pub fn process_updates(&mut self, app: &mut App, terminal: &mut Terminal) -> Result<()> {
        if !self.should_render() {
            return Ok(());
        }

        // Batch process updates
        let mut processed = 0;
        const MAX_BATCH: usize = 100;  // Process at most 100 updates per frame

        while processed < MAX_BATCH && !self.update_queue.is_empty() {
            if let Some(_event) = self.update_queue.pop_front() {
                // Event already updated dirty flags when pushed
                processed += 1;
            }
        }

        // Render only dirty components
        terminal.draw(|frame| {
            self.render_selective(app, frame);
        })?;

        self.dirty = DirtyFlags::empty();
        self.last_render = Instant::now();

        Ok(())
    }

    /// Selective rendering based on dirty flags
    fn render_selective(&mut self, app: &App, frame: &mut Frame) {
        let rect = frame.area();

        // Always render navbar if anything is dirty
        if !self.dirty.is_empty() {
            self.render_navbar(app, frame, rect);
        }

        // Render specific components based on dirty flags
        if self.dirty.intersects(DirtyFlags::WATCHLIST) {
            if self.states.watchlist.should_render(&app.watchlist) {
                self.render_watchlist(app, frame, rect);
            }
        }

        if self.dirty.intersects(DirtyFlags::STOCK_DETAIL) {
            if self.states.stock_detail.should_render(&app.stock_detail) {
                self.render_stock_detail(app, frame, rect);
            }
        }

        // ... other components
    }
}
```

**主循环改造：**

```rust
// src/app.rs
pub async fn run() {
    let mut render_manager = RenderManager::new(
        Duration::from_millis(16)  // ~60 FPS max
    );

    // No more fixed render tick - render on demand
    let mut throttle_tick = tokio::time::interval(Duration::from_millis(16));

    loop {
        tokio::select! {
            _ = throttle_tick.tick() => {
                // Check if render is needed (throttled)
                if let Err(e) = render_manager.process_updates(&mut app, &mut terminal) {
                    tracing::error!("Render error: {}", e);
                }
            }

            Some(mut cmd) = update_rx.recv() => {
                cmd.apply(&mut app.world);
                render_manager.push_update(UpdateEvent::StateChange {
                    new_state: app.world.resource::<State<AppState>>().get().clone()
                });
            }

            Some(push_event) = quote_receiver.next() => {
                let symbol = push_event.symbol.clone();

                match push_event.detail {
                    PushEventDetail::Quote(quote) => {
                        // Update data
                        update_quote_data(&symbol, quote);
                        // Queue update
                        render_manager.push_update(UpdateEvent::Quote {
                            symbols: vec![symbol]
                        });
                    }
                    PushEventDetail::Depth(depth) => {
                        update_depth_data(&symbol, depth);
                        render_manager.push_update(UpdateEvent::Depth { symbol });
                    }
                    PushEventDetail::Trade(trade) => {
                        update_trade_data(&symbol, trade);
                        render_manager.push_update(UpdateEvent::Trades { symbol });
                    }
                }
            }
        }
    }
}
```

---

## 对比总结

| 方案 | 优点 | 缺点 | 复杂度 |
|------|------|------|--------|
| **方案 A: 脏标记** | ✅ 简单直接<br>✅ 易于集成 | ⚠️ 仍可能过度渲染<br>⚠️ 需手动维护标记 | ⭐⭐ |
| **方案 B: 状态对比** | ✅ 精确检测变化<br>✅ 自动化 | ⚠️ 需实现 PartialEq<br>⚠️ 状态克隆开销 | ⭐⭐⭐ |
| **方案 C: 更新队列** | ✅ 最优化<br>✅ 支持优先级<br>✅ 批量处理 | ⚠️ 实现复杂<br>⚠️ 需重构较多 | ⭐⭐⭐⭐ |

---

## 推荐实施路线

### Phase 1: 简单脏标记（1-2天）

实现方案 A 的基础版本：
- 添加 `DirtyFlags` 系统
- 替换 `bool needs_render` 为 `RenderState`
- 在数据更新时精确标记脏组件

**预期收益：** 减少 30-40% 不必要的渲染

### Phase 2: 状态对比（2-3天）

在 Phase 1 基础上添加状态对比：
- 实现 `ComponentStateManager`
- 为主要组件添加状态快照
- 结合脏标记 + 状态对比

**预期收益：** 再减少 20-30% 渲染，总计减少 50-60%

### Phase 3: 完整优化（3-5天）

实现完整的方案 C：
- 添加更新队列和优先级系统
- 批量处理更新
- 性能监控和调优

**预期收益：** 最优化渲染，减少 70-80% 不必要渲染

---

## 性能指标

**当前状态（粗糙 needs_render）：**
- 每秒渲染次数：60 FPS（固定）
- 实际需要渲染：~10-15 FPS（估计）
- 浪费率：~75-83%

**优化后（Phase 2）：**
- 按需渲染：10-20 FPS
- 浪费率：<20%
- CPU 使用率：降低 40-50%

**优化后（Phase 3）：**
- 按需渲染：5-15 FPS
- 浪费率：<10%
- CPU 使用率：降低 60-70%
- 支持高频数据更新（>100 updates/sec）

---

## Ratatui 增量更新能力

Ratatui 本身的优化机制：

1. **Buffer Diffing**：Ratatui 内部已经实现了缓冲区差异对比
   ```rust
   // Ratatui 会自动比较前后两帧的差异
   terminal.draw(|frame| {
       // 即使你每帧都调用，Ratatui 只会更新变化的部分
       frame.render_widget(widget, area);
   })?;
   ```

2. **Double Buffering**：自动双缓冲，减少闪烁

3. **Lazy Evaluation**：Widget 只在实际绘制时计算

**我们的优化重点：**
- ✅ 减少 `terminal.draw()` 调用次数
- ✅ 避免不必要的数据更新
- ✅ 优化组件状态管理

---

## 建议

**优先实施 Phase 1（脏标记系统）**，原因：
1. ✅ 投入产出比最高
2. ✅ 实现简单，风险低
3. ✅ 可以快速看到效果
4. ✅ 为后续优化打基础

**如果时间充裕，继续实施 Phase 2（状态对比）**，可获得接近最优的性能。

Phase 3 可以作为长期目标，在实际遇到性能瓶颈时再考虑。
