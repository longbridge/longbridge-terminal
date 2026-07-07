# Longbridge Terminal

<p align="center">
  <strong>简体中文</strong> | <a href="./README.md">English</a> 
</p>

[长桥（Longbridge）](https://longbridge.com)交易平台的 AI 原生命令行工具（CLI）——提供实时行情、投资组合与交易功能，同时附带一个全屏终端界面（**TUI**，Terminal User Interface），可用于交互式监控。

支持长桥 OpenAPI 的全部接口：行情方面，包括实时报价、深度、K 线和期权、权证数据；投资组合方面，包括账户余额、股票和基金持仓；交易方面，包括订单提交、修改、撤单和成交历史。专为脚本开发、AI Agent 工具调用以及终端内的日常交易工作流而设计。

```bash
$ longbridge static TSLA.US NVDA.US
| Symbol | Last | Prev Close | Open | High | Low | Volume | Turnover | Status |
|---------|---------|------------|---------|---------|---------|-----------|-----------------|--------|
| TSLA.US | 395.560 | 391.200 | 396.220 | 403.730 | 394.420 | 58068343 | 23138752546.000 | Normal |
| NVDA.US | 183.220 | 180.250 | 182.970 | 188.880 | 181.410 | 217307380 | 40023702698.000 | Normal |

$ longbridge quote TSLA.US NVDA.US --format json
[
 {
 "high": "403.730",
 "last": "395.560",
 "low": "394.420",
 "open": "396.220",
 "prev_close": "391.200",
 "status": "Normal",
 "symbol": "TSLA.US",
 "turnover": "23138752546.000",
 "volume": "58068343"
 },
 {
 "high": "188.880",
 "last": "183.220",
 "low": "181.410",
 "open": "182.970",
 "prev_close": "180.250",
 "status": "Normal",
 "symbol": "NVDA.US",
 "turnover": "40023702698.000",
 "volume": "217307380"
 }
]
```

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## 安装

**Homebrew（macOS / Linux）**

```bash
brew install --cask longbridge/tap/longbridge-terminal
```

**Windows**（[Scoop](https://scoop.sh)）

```powershell
scoop install https://github.com/longbridge/longbridge-terminal/raw/refs/heads/main/.scoop/longbridge.json
```

**Windows**（PowerShell）

```powershell
iwr https://github.com/longbridge/longbridge-terminal/raw/main/install.ps1 | iex
```

**安装脚本（macOS / Linux）**

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

将 `longbridge` 二进制文件安装到 `/usr/local/bin`（macOS/Linux）或 `%LOCALAPPDATA%\Programs\longbridge`（Windows）。

## 认证

通过长桥 SDK 使用 **OAuth 2.0** 认证——无需手动管理令牌。

```bash
longbridge auth login # 打开浏览器进行 OAuth 认证并保存令牌（由 SDK 管理）
longbridge auth logout # 清除已保存的令牌
longbridge check # 验证令牌有效性、区域和 API 接口连通性
```

令牌在命令行工具和终端界面（TUI）之间共享。`login` 后，所有命令无需重新认证即可使用。

命令行工具在每次启动时会在后台探测 `geotest.lbkrs.com` 来自动检测是否位于中国大陆，并缓存结果。若检测到，下次运行时将自动使用中国大陆 API 接口。

## Shell 补全

在 Shell 中启用 `longbridge` 命令与参数的 Tab 补全：

**Bash** —— 添加到 `~/.bashrc` 或 `~/.bash_profile`：

```bash
source <(longbridge completion bash)
```

**Zsh** —— 添加到 `~/.zshrc`：

```zsh
source <(longbridge completion zsh)
```

**Fish** —— 添加到 `~/.config/fish/config.fish`：

```fish
longbridge completion fish | source
```

重新加载 Shell 后，输入 `longbridge <TAB>` 即可提示子命令、参数和可选值。

## 命令行用法

```
longbridge <command> [options]
```

所有命令均支持 `--format json` 以输出 JSON 格式，便于程序处理。接受 `--count` 的命令也可使用 `--limit` 作为别名（便于 AI Agent 兼容）：

```bash
longbridge quote TSLA.US --format json
longbridge positions --format json | jq '.[] | {symbol, quantity}'
```

### 诊断

```bash
longbridge check # 检查令牌有效性和 API 连通性
```

### 行情

```bash
longbridge quote TSLA.US 700.HK # 获取一个或多个股票的实时报价
longbridge depth TSLA.US # Level 2（二档行情）深度行情（买盘/卖盘阶梯）
longbridge brokers 700.HK # 各价位经纪队列（港股市场）
longbridge trades TSLA.US [--count 50] # 近期逐笔成交记录
longbridge intraday TSLA.US # 当日日内每分钟价格和成交量明细
longbridge kline TSLA.US [--period day] # OHLCV K 线数据 [--adjust none|forward]
longbridge kline history TSLA.US --start 2024-01-01 # 指定日期范围内的历史 OHLCV K线数据
longbridge static TSLA.US # 获取一个或多个股票的基本信息
longbridge calc-index TSLA.US --fields pe,pb,eps # 计算财务指标（PE / PB / EPS、换手率等）
longbridge capital TSLA.US # 资金分布橄榄（大/中/小单资金流入流出）
longbridge capital TSLA.US --flow # 日内资金流向时间序列（大/中/小单资金进出）
longbridge market-temp [HK|US|CN|SG] # 市场情绪指数（0–100，数值越高表示越看多）
longbridge constituent .SPX.US [--sort market-cap] # 指数成分股（美股指数代码需要以点号（`.`）开头，如 .DJI.US、.SPX.US）
longbridge constituent IVV.US [--limit 0] # 美股 ETF 完整持仓，数据来源于 SEC N-PORT（--limit 0 = 全部）；当 SEC 数据不可用时（如 SPY），回退至平台资产配置数据
longbridge trading session # 各市场交易时段（开盘/收盘时间）安排
longbridge trading days HK # 某市场的交易日和半日交易日
longbridge security-list HK # 某市场的全部可交易证券列表
longbridge participants # 做市商（市场参与者）经纪商 ID 和名称
longbridge subscriptions # 当前会话中的实时 WebSocket 订阅
```

### 资讯

```bash
longbridge news TSLA.US [--count 20] # 获取指定股票的最新资讯
longbridge news detail <news-id> # 某篇资讯文章的完整 Markdown 内容
longbridge filing list AAPL.US [--count 20] # 某股票的监管申报和公告列表
longbridge filing detail AAPL.US <filing-id> # 某份申报的完整 Markdown 内容；对于多文件申报（如 8-K 附件），使用 --file-index N
longbridge topic list TSLA.US [--count 20] # 某股票的社区讨论话题列表
longbridge topic detail <topic-id> # 某社区话题的详情（正文、作者、关联代码、统计数据、URL）
longbridge topic replies <topic-id> [--page 1] # 话题的回复列表（分页，--size 1–50）
longbridge topic mine [--type article] # 当前登录用户创建的话题
longbridge topic create --body "…" # 发布新的社区讨论话题（--title 为可选参数）
longbridge topic create-reply <topic-id> --body "…" # 对话题发表回复（--reply-to <reply-id> 用于嵌套回复）
```

### 期权与权证

```bash
longbridge option quote AAPL240119C190000 # 期权合约实时报价
longbridge option chain AAPL.US # 期权链：列出所有到期日
longbridge option chain AAPL.US --date 2024-01-19 # 期权链：指定到期日的行权价列表
longbridge option volume AAPL.US # 实时期权看涨/看跌成交量及看跌/看涨比率
longbridge option volume daily AAPL.US # 每日期权看涨/看跌成交量及未平仓合约历史
longbridge option volume daily AAPL.US --count 60 # 返回最近 60 个交易日的数据
longbridge warrant quote 12345.HK # 权证合约实时报价
longbridge warrant 700.HK # 与某标的证券关联的权证
longbridge warrant issuers # 权证发行人列表（港股市场）
```

### 基本面

```bash
longbridge financial-report AAPL.US [--kind IS|BS|CF] # 多期财务报表（利润表 / 资产负债表 / 现金流量表）
longbridge financial-report AAPL.US --latest # 最新财务报告摘要
longbridge financial-report snapshot AAPL.US --report qf --year N --period N # 盈利摘要、预测 vs 实际（营收/EBIT/EPS 超预期/低于预期）、财务比率
longbridge financial-statement AAPL.US [--kind IS|BS|CF|ALL] [--report af|saf|qf|cumul] # 详细财务报表（v3 接口）
longbridge institution-rating AAPL.US # 分析师评级分布和一致预期目标价
longbridge institution-rating AAPL.US --history # 评级和目标价变动历史
longbridge institution-rating AAPL.US --industry-rank [--page 1] [--limit 20] # 全行业机构评级排名
longbridge institution-rating AAPL.US --views # 月度买入/持有/卖出分布时间线（机构观点）
longbridge institution-rating detail AAPL.US # 月度评级趋势和分析师准确率历史
longbridge dividend AAPL.US # 历史股息记录
longbridge dividend detail AAPL.US # 股息分配方案详情
longbridge forecast-eps AAPL.US # 分析师 EPS 一致预期
longbridge consensus AAPL.US # 营收 / 净利润 / EPS 多期对比（含超预期/低于预期标记）
longbridge valuation AAPL.US [--indicator pe|pb|ps|dvd_yld] # 当前估值概览与同行对比
longbridge valuation AAPL.US --history [--indicator pe] [--range 5] # 历史估值时间序列（1 / 3 / 5 / 10 年）
longbridge valuation-rank AAPL.US [--start 20240101] [--end 20241231] # 行业估值百分位排名（默认：最近 30 天）
longbridge analyst-estimates AAPL.US # 分析师一致预期 EPS 预测
longbridge fund-holder AAPL.US [--count 20] # 持有该股票的基金和 ETF
longbridge shareholder AAPL.US [--range all|inc|dec] [--sort chg] # 机构股东及环比变动追踪
longbridge shareholder AAPL.US --top # 前 20 大股东（含个人和内部人士，多期数据）
longbridge shareholder AAPL.US --object-id <id> # 特定股东的持仓和交易详情（使用 --top 输出中的 ID）
longbridge compare AAPL.US # 多股票估值对比（与服务器选取的行业同行对比）
longbridge compare 9988.HK 700.HK 9999.HK [--currency HKD] # 并排对比指定股票（价格、市值、PE/PB/PS、ROE、ROA、股息率等）
longbridge corp-action 700.HK [--all] # 公司行动（拆股、分红、配股等）—— 默认 30 条，--all 获取全部历史
longbridge business-segments AAPL.US [--history] [--report qf|saf|af] [--cate <cate>] # 营收分部构成（当前概览或历史趋势）
longbridge industry-rank --market US|HK|CN|SG [--indicator leading-gainer|...|net-profit-growth] # 行业排名列表；返回的行业代码可直接用于 industry-peers 命令
longbridge industry-peers IN00446.US # 某行业指数成分股的同行分层树（来自 industry-rank）
longbridge macrodata [--country US] [--page 1] [--limit 20] # 列出宏观经济指标（每页 20 条）；名称语言跟随 --lang
longbridge macrodata US00175 [--start 2024-01-01] [--end 2024-12-31] # 某指标的历史发布数据（实际值 / 预测值 / 前值）
```

### 出入金

```bash
longbridge bank-cards # 列出已绑定的银行卡
longbridge withdrawals [--page 1] [--limit 20] # 出金历史
longbridge deposits [--page 1] [--limit 20] [--states 0,1,2] [--currencies HKD,USD] # 入金历史
```

### 搜索

```bash
longbridge search TSLA [--tab market|news|posts|hashtags|help|share-lists|users|institutions] # 跨多种内容类型搜索
longbridge search-hot # 热门搜索关键词
```

### IPO

```bash
longbridge ipo subscriptions # 当前处于申报或认购阶段的 IPO 股票
longbridge ipo wait-listing # 处于暗盘市场（等待挂牌）阶段的 IPO 股票
longbridge ipo listed [--page 1] [--limit 20] # 近期已上市的 IPO 股票
longbridge ipo calendar # IPO 日历（所有即将到来和近期的 IPO）
longbridge ipo detail <symbol> [--market HK|US] # 某 IPO 的概况、时间线、申购资格和持仓
longbridge ipo orders [--market HK] [--status 0] [--page 1] # 当前账户的 IPO 订单（活跃 + 历史）
longbridge ipo orders detail <order-id> # 单笔 IPO 订单的完整详情
longbridge ipo profit-loss [--period all|1m|3m|6m|1y] [--page 1] # IPO 盈亏（P&L）汇总和明细列表
longbridge ipo us-subscriptions # 当前处于认购阶段的美国 IPO 股票
longbridge ipo us-wait-listing # 处于等待挂牌阶段的美国 IPO 股票
longbridge ipo us-listed [--page 1] [--limit 20] # 近期已上市的美国 IPO 股票
longbridge ipo submit TSLA.US --qty 200 --amount 1000 [--method 2] # 提交 IPO 认购（需确认）
longbridge ipo withdraw <order-id> # 撤回 IPO 认购（需确认）
```

### 市场数据

```bash
longbridge rank # 列出可用的热度排行分类键
longbridge rank --key ib_hot_all-us [--count 20] # 按综合热度评分排名的股票（交易活跃度、媒体、社区、波动性）
longbridge top-movers [--market HK|US|CN|SG] [--sort hot|time|chg] # 出现异常价格波动的股票，附带关联资讯和原因摘要
longbridge exchange-rate # 各市场汇率
longbridge finance-calendar financial [--symbol AAPL.US] # 当天及未来的业绩指引公告
longbridge finance-calendar report [--symbol AAPL.US] # 当天及未来的财报发布日期
longbridge finance-calendar dividend [--symbol AAPL.US] # 当天及未来的股息除权日 / 派发事件
longbridge finance-calendar ipo [--market US] # 当天及未来的 IPO 上市时间线
longbridge finance-calendar macrodata [--star 3] # 宏观经济事件（--star 1–3 按重要性筛选）
longbridge finance-calendar closed [--market HK] # 市场假期和缩短交易日
```

### 自选列表

```bash
longbridge watchlist # 列出所有自选列表分组及其股票（置顶的优先显示）
longbridge watchlist show <group-id> # 查看指定分组中的股票（置顶标记）
longbridge watchlist create "My Portfolio" # 创建新的自选列表分组
longbridge watchlist update <group-id> --add TSLA.US # 向分组中添加股票
longbridge watchlist update <group-id> --remove AAPL.US # 从分组中移除股票
longbridge watchlist delete <group-id> # 删除自选列表分组
longbridge watchlist pin TSLA.US AAPL.US # 将股票置顶到所在分组的顶部
longbridge watchlist pin --remove 700.HK # 取消置顶
```

### 分享列表

```bash
longbridge sharelist # 列出自己创建的和已订阅的分享列表
longbridge sharelist [--count 50] # 自定义每页数量
longbridge sharelist detail <list-id> # 查看详情和成分股
longbridge sharelist create --name "My Picks" [--description "…"] # 创建新的分享列表
longbridge sharelist delete <list-id> # 删除分享列表
longbridge sharelist add <list-id> TSLA.US AAPL.US 700.HK # 添加股票到分享列表
longbridge sharelist remove <list-id> TSLA.US # 从分享列表中移除股票
longbridge sharelist sort <list-id> TSLA.US AAPL.US 700.HK # 重排分享列表中的股票
longbridge sharelist popular [--count 10] # 获取热门（趋势）分享列表
```

### 交易

```bash
longbridge order # 今日订单，或通过 --history 查看历史订单
longbridge order --history [--start 2024-01-01] # 历史订单（使用 --symbol 过滤）
longbridge order detail <order-id> # 单笔订单的完整详情，包含费用和历史
longbridge order executions # 今日成交记录（成交明细），或通过 --history 查看历史
longbridge order buy TSLA.US 100 --price 250.00 # 提交买单（需确认）
longbridge order sell TSLA.US 100 --price 260.00 # 提交卖单（需确认）
longbridge order cancel <order-id> # 撤销待处理订单（需确认）
longbridge order replace <order-id> --qty 200 --price 255.00 # 修改待处理订单的数量或价格
longbridge assets [--currency USD] # 资产概览：净资产、现金、购买力、保证金及按币种划分
longbridge cash-flow [--start 2024-01-01] # 资金流水记录（入金、出金、股息、结算）
longbridge portfolio # 投资组合概览：总资产、盈亏（P&L）、持仓及现金明细
longbridge portfolio short-margin # 卖空保证金详情
longbridge positions # 所有子账户的当前股票（权益类）持仓
longbridge fund-positions # 所有子账户的当前基金（公募基金）持仓
longbridge margin-ratio TSLA.US # 某股票的保证金比例要求
longbridge max-qty TSLA.US --side buy --price 250 # 根据当前账户余额估算最大可买或可卖数量
```

### 盈亏分析

```bash
longbridge profit-analysis # 盈亏（P&L）汇总及按股票划分
longbridge profit-analysis detail 700.HK # 股票盈亏明细 + 交易流水
longbridge profit-analysis detail 700.HK --derivative # 显示衍生品流水
longbridge profit-analysis by-market # 按市场划分的股票盈亏（分页）
longbridge profit-analysis by-market --market HK --size 50 # 按市场过滤
```

### 对账单

```bash
longbridge statement list [--type daily|monthly] # 列出可用的账户对账单（日结单或月结单）
longbridge statement export --file-key <key> --section equity_holdings # 将对账单部分导出为 CSV 或 Markdown
longbridge statement export --file-key <key> --all # 导出所有非空部分
```

### 内部人交易

```bash
longbridge insider-trades TSLA.US # 近期 Form 4 内部人交易（SEC EDGAR，仅美股）
longbridge insider-trades AAPL.US --count 40 # 获取 40 份 Form 4 申报（默认 20 份）
longbridge insider-trades NVDA.US --format json # 导出为 JSON
```

### 投资者

```bash
longbridge investors # 按资产管理规模（AUM）排名的前 50 位活跃基金管理人（实时 SEC 13F 排名；已排除被动指数巨头；使用 --top N 调整数量）
longbridge investors 0001067983 # 按 SEC CIK 编号查看任意申报人的 13F 持仓
longbridge investors 0001067983 --top 20 # 仅显示前 20 大持仓
longbridge investors 0001067983 --format json # 将持仓导出为 JSON
longbridge investors changes 0001067983 # 季度环比变动（新增/加仓/减仓/清仓）
longbridge investors changes 0001067983 --from 2024-12-31 # 对比最新数据与指定期间的差异
```

### 定期投资

```bash
longbridge dca # 列出所有定期投资计划
longbridge dca --status Active # 按状态过滤：Active | Suspended | Finished
longbridge dca --symbol TSLA.US # 按股票代码过滤
longbridge dca create TSLA.US --amount 500 --frequency weekly --day-of-week mon # 创建每周定投计划
longbridge dca create 700.HK --amount 1000 --frequency monthly --day-of-month 15 # 创建每月定投计划
longbridge dca update <plan-id> --amount 800 # 更新计划金额
longbridge dca pause <plan-id> # 暂停定期投资计划
longbridge dca resume <plan-id> # 恢复已暂停的定期投资计划
longbridge dca stop <plan-id> # 永久停止定期投资计划
longbridge dca history <plan-id> # 某计划的交易历史
longbridge dca stats # 定期投资统计汇总
longbridge dca calc-date TSLA.US --frequency weekly --day-of-week fri # 计算下一交易日
longbridge dca check TSLA.US AAPL.US 700.HK # 检查哪些股票支持定期投资
longbridge dca set-reminder 6 # 设置交易前提醒小时数（1 | 6 | 12）
```

### 卖空

```bash
longbridge short-positions AAPL.US # 美股：双周 FINRA 沽空比率（沽空比率、费率、回补天数）
longbridge short-positions 700.HK # 港股：每日港交所（HKEX）披露的沽空持仓（沽空股数、余额、成本、比率）
longbridge short-positions TSLA.US --count 50 # 返回最近 50 条记录
longbridge short-trades AAPL.US # 每日卖空成交量（美股：FINRA/NASDAQ；港股：港交所）
longbridge short-trades 700.HK [--count 50] # 港股：每交易日成交额、余额、总成交额、费率、收盘价
```

### 选股器

```bash
longbridge screener strategies # 列出推荐选股策略
longbridge screener strategies --all # 列出平台全部策略
longbridge screener strategies --mine # 列出用户自创策略
longbridge screener strategies --id <strategy-id> # 查看特定策略的分组和指标
longbridge screener search --strategy-id <strategy-id> # 运行已保存策略并返回匹配股票
longbridge screener search --market HK --filter filter_marketcap:100:1000 --filter filter_divyld:3: # 自定义筛选（键:最小值:最大值，省略某一侧表示不设限）
longbridge screener indicators # 列出所有可用的筛选指标，含 ID、键和默认值范围
```

### 股票代码格式

```
<代码>.<市场>   如  TSLA.US   700.HK   600519.SH
```

市场：`HK`（港股）· `US`（美股）· `CN` / `SH` / `SZ`（A股）· `SG`（新加坡）

## Skill

安装 Skill 后，你的 AI 工具即可全面掌握所有 `longbridge` 命令行命令：

```bash
npx skills add longbridge/developers
```

更多关于长桥 Skill 的信息，请访问：https://open.longbridge.com/skill/

安装后，Claude 可以直接在你的 AI 工作流中查询行情数据、运行技术分析并管理交易。

```
claude> 帮我展示 TSLA 和 NVDA 最近 5 天的表现

● Bash(longbridge kline TSLA.US --period day --count 5 & longbridge kline NVDA.US --period day --count 5 & wait)

● 以下是两只股票最近 5 天的表现：

  TSLA.US — 最近 5 天

  ┌────────┬────────┬────────┬────────┬────────┬──────────┐
  │  Date  │  Open  │  High  │  Low   │ Close  │  Change  │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 10 │ 402.22 │ 406.59 │ 398.19 │ 399.24 │ —        │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 11 │ 402.28 │ 416.38 │ 402.15 │ 407.82 │ ▲ +2.15% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 12 │ 405.18 │ 406.50 │ 394.65 │ 395.01 │ ▼ -3.14% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 13 │ 399.17 │ 400.20 │ 389.95 │ 391.20 │ ▼ -0.96% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 16 │ 396.22 │ 403.73 │ 394.42 │ 395.56 │ ▲ +1.11% │
  └────────┴────────┴────────┴────────┴────────┴──────────┘

  5 日回报：-0.92% | 区间：$389.95 – $416.38

  NVDA.US — 最近 5 天

  ┌────────┬────────┬────────┬────────┬────────┬──────────┐
  │  Date  │  Open  │  High  │  Low   │ Close  │  Change  │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 10 │ 112.34 │ 115.20 │ 111.80 │ 114.50 │ —        │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 11 │ 114.80 │ 117.60 │ 114.20 │ 116.90 │ ▲ +2.10% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 12 │ 116.50 │ 118.30 │ 115.40 │ 115.80 │ ▼ -0.94% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 13 │ 115.20 │ 116.80 │ 113.90 │ 114.60 │ ▼ -1.04% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 16 │ 114.90 │ 117.50 │ 114.30 │ 116.80 │ ▲ +1.92% │
  └────────┴────────┴────────┴────────┴────────┴──────────┘

  5 日回报：+2.01% | 区间：$111.80 – $118.30
```

## 终端界面（TUI）

```bash
longbridge tui
```

功能：实时自选列表、K 线图、投资组合视图、股票搜索、类 Vim 快捷键。

## 输出格式

```bash
--format table   # 人类可读的 ASCII 表格（默认）
--format json    # 机器可读的 JSON 格式，适合 AI Agent 和管道操作
```

## 速率限制

长桥 OpenAPI：每秒最多 10 次调用。SDK 会自动刷新 OAuth 令牌。

## 要求

- macOS、Linux 或 Windows
- 网络连接和浏览器访问（用于首次 OAuth 认证）
- [长桥账户](https://open.longbridge.com)

## 文档

- [长桥 OpenAPI 文档](https://open.longbridge.com)
- [Rust SDK](https://longbridge.github.io/openapi/rust/longbridge/)

## 许可证

Apache License 2.0。详见 [LICENSE](LICENSE)。
