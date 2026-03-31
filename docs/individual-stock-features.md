# 个股功能清单 & CLI 规划

> 来源：longbridge-gpui codebase 分析 + 内部 API 网关 scopes.json 比对 + engine WS 协议分析
> 目标：服务端暴露 API + CLI 命令设计

---

## 实施列表

| 状态 | 分类   | 功能名称          | CLI 命令                               | API 接口                                                  | 优先级 | 备注                                                      |
| ---- | ------ | ----------------- | -------------------------------------- | --------------------------------------------------------- | ------ | --------------------------------------------------------- |
| ✅   | 基本面 | 财务报表          | `longbridge financial-report`          | `GET /v1/quote/financial-reports`                         | P0     | IS/BS/CF，transposed 多期对比                             |
| ✅   | 基本面 | 估值分析          | `longbridge valuation`                 | `GET /v1/quote/valuation` `GET /v1/quote/valuation/detail`| P0     | PE/PB/PS/DY 历史 + 同行对比；`detail` 子命令             |
| ✅   | 基本面 | 机构评级          | `longbridge institution-rating`        | `GET /v1/quote/institution-ratings[/detail]`              | P0     | 评级分布 + 目标价；`detail` 子命令查历史                  |
| ✅   | 基本面 | 分红历史          | `longbridge dividend`                  | `GET /v1/quote/dividends[/details]`                       | P0     | 历史派息记录；`detail` 子命令查分配方案                   |
| ✅   | 基本面 | EPS 业绩预测      | `longbridge forecast-eps`              | `GET /v1/quote/forecast-eps`                              | P0     | 分析师 EPS 共识快照序列                                   |
| ✅   | 基本面 | 财务一致预期      | `longbridge consensus`                 | `GET /v1/quote/financial-consensus-detail`                | P0     | 营收/利润/EPS 多期横向对比，含 beat/miss 标记             |
| ✅   | 资讯   | 新闻资讯          | `longbridge news`                      | SDK `ContentContext.news()`                               | P0     | 股票相关新闻；`detail` 子命令看全文                       |
| ✅   | 资讯   | 监管文件          | `longbridge filing`                    | SDK `QuoteContext.filings()`                              | P0     | 监管公告/文件；`detail` 子命令列文件/下载                 |
| ✅   | 行情   | 期权              | `longbridge option`                    | SDK `QuoteContext.option_quote()` / `option_chain_*()`    | P0     | 期权行情 + 期权链                                         |
| ✅   | 行情   | 权证              | `longbridge warrant`                   | SDK `QuoteContext.warrant_quote()` / `warrant_list()`     | P1     | 权证行情 + 权证列表 + 发行人列表                          |
|      | 行情   | 美股深度摆盘      | `longbridge orderbook`                 | WS `TOTAL_VIEW` / `TOTAL_VIEW_BRIEF`                      | P0     | 全量订单簿；`--brief` 取 60 档；引擎已实现，仅需暴露      |
|      | 行情   | 资金流（大笔）    | `longbridge flow`                      | WS `DETAIL.inflow` + REST（待确认）                       | P0     | 大单净流入/流出（超大/大/中/小单分层）；历史 K 线 + 分时  |
|      | 行情   | 做空数据          | `longbridge short`                     | REST（待确认）                                            | P0     | 美股/港股卖空成交 + 做空持仓数量/比例                     |
|      | 行情   | K 线扩展时段      | `longbridge kline --session`           | SDK `QuoteContext.candlesticks()` + `session` 参数        | P0     | 新增 `--session pre-post/all`；引擎已支持，CLI 未透传     |
|      | 行情   | 分时/逐笔扩展时段 | `longbridge intraday/trades --session` | SDK `QuoteContext.intraday_lines()` / `trades()`          | P0     | 盘前盘后/夜盘；引擎已支持                                 |
| ✅   | 基本面 | 投资风格与多维评分 | `longbridge score`                    | `GET /v1/quote/security-ratings`                          | P1     | 投资风格分类 + 盈利/成长/现金/运营/负债五维打分           |
|      | 公司   | 公司概况          | `longbridge company`                   | `GET /stock-info/comp-overview`                           | P1     | 基本信息、员工数、IPO 价格等                              |
|      | 公司   | 公司高管          | `longbridge executives`                | `GET /stock-info/company-professionals`                   | P1     | 高管姓名、职位、biography                                 |
|      | 公司   | 所属行业 & 排名   | `longbridge industry`                  | `GET /v1/stock-info/panorama` + `ranking-in-industry`     | P1     | 行业涨跌统计 + 个股指标排名                               |
|      | 公司   | 股东结构          | `longbridge shareholders`              | `GET /stock-info/company-shareholders`                    | P1     | Top20 股东；`--institutions` 机构；`--insiders` 内部人    |
|      | 公司   | 主营业务拆分      | `longbridge business`                  | `GET /stock-info/business[-historical]`                   | P1     | 按业务线/地区拆分营收占比                                 |
|      | 公司   | 供应链            | `longbridge supply-chain`              | `GET /stock-info/supply_chains[/detail]`                  | P1     | 上下游供应商/客户列表                                     |
|      | 市场   | 指数/ETF 成分股   | `longbridge constituents`              | `GET /v2/discovery/index-constituents`                    | P1     | 指数成分股 + ETF 持仓；统一命令                           |
|      | 市场   | 持有该股的基金    | `longbridge fund-holders`              | `GET /ut/fundamental/reverse/stock`                       | P1     | 哪些基金/ETF 持有该股，含持仓比例                         |
|      | 市场   | 新股日历 & 详情   | `longbridge ipo`                       | `POST /ipo/calendar` + `GET /stock-info/ipo-profile`      | P1     | upcoming/subscribing/listed + 招股详情                    |
|      | 市场   | 财经日历          | `longbridge calendar`                  | `POST /stock_info/finance_calendar`                       | P2     | 财报发布日、IPO 日期、宏观经济事件                        |
|      | 行情   | 期权成交量统计    | `longbridge option volume`             | REST（待确认）                                            | P2     | Call/Put 总成交量比例                                     |
|      | 账户   | 股价提醒          | `longbridge alert`                     | `price-notify` scope（待确认）                            | P1     | 设置/查看/删除价格提醒；多指标                            |
|      | 账户   | 股票备注          | `longbridge note`                      | REST（待确认）                                            | P2     | 对自选股设置/读取个人备注                                 |
|      | 账户   | 内部人士交易      | `longbridge insider`                   | `GET /stock-info/get_company_insider_holding_detail`      | P1     | 高管/大股东增减持明细                                     |
|      | 选股   | 选股器            | `longbridge screener`                  | REST（待确认）                                            | P1     | 多指标条件筛选（估值/技术/基本面）                        |

---

## 零、行情数据能力参考（WebSocket 协议）

> 行情走二进制 WS 长连接，不在 scopes.json 中，来源：`engine/src/ws/binary/`

### 订阅类型（SubTypes）

| SubType            | 说明                                             | 市场                   |
| ------------------ | ------------------------------------------------ | ---------------------- |
| `LIST`             | 列表页轻量价格（last_done, amount, balance）     | 全市场                 |
| `DETAIL`           | 详情页完整行情（见下方字段表）                   | 全市场                 |
| `DEPTH`            | 10 档买卖盘（price, volume, order_num）          | 全市场                 |
| `BROKER`           | 各档经纪商队列                                   | **仅港股**             |
| `TRADE`            | 盘中逐笔成交                                     | 全市场                 |
| `PRE_TRADE`        | 盘前逐笔成交                                     | **美股**               |
| `POST_TRADE`       | 盘后逐笔成交                                     | **美股**               |
| `NIGHT_TRADE`      | 夜盘逐笔成交                                     | 美股/A股夜盘           |
| `TOTAL_VIEW`       | 深度摆盘——全量订单簿，含每笔挂单 id/volume/mp_id | **美股**（ORDER_BOOK） |
| `TOTAL_VIEW_BRIEF` | 深度摆盘简要数据（60档）                         | **美股**               |

> `TOTAL_VIEW` 对应 proto `ORDER_BOOK`，`data_level`：0=默认、1=全量、2=仅60档挂单。Push 命令 `PushOrderBook(109)` / `PushOrderBookBrief(110)`。

### DETAIL 行情字段（StockDetail proto）

| 字段                                                                     | 说明                              |
| ------------------------------------------------------------------------ | --------------------------------- |
| last_done / open / high / low / prev_close                               | 最新价、开盘、最高、最低、昨收    |
| amount / balance                                                         | 成交量 / 成交额                   |
| turnover_rate / volume_rate / depth_rate                                 | 换手率 / 量比 / 委比              |
| year_high / year_low                                                     | 52 周高低                         |
| eps / eps_ttm / eps_forecast                                             | 每股收益（静/TTM/动）             |
| market_cap                                                               | 总市值                            |
| total_shares / circulating_shares                                        | 总股本 / 流通股本                 |
| bps / dps_rate / dividend_yield                                          | 每股净资产 / 股息率 / 股息 TTM    |
| limit_up / limit_down                                                    | 涨停价 / 跌停价（A股）            |
| market_price / market_high / market_low / market_amount / market_balance | 盘前/盘后行情（美股）             |
| ah_premium                                                               | A/H 溢价（AH 两地上市股）         |
| inflow                                                                   | 大单资金净流入                    |
| industry_counter_id / industry_name                                      | 所属行业                          |
| stock_derivatives                                                        | 支持的衍生品（option/warrant 等） |
| tags / available_levels                                                  | 行情标签 / 可用行情等级           |

### K 线类型（KlineType）与参数

| 级别     | 枚举值                                          |
| -------- | ----------------------------------------------- |
| 分钟线   | 1m / 2m / 3m / 5m / 10m / 15m / 20m / 30m / 45m |
| 小时线   | 60m / 120m / 180m / 240m                        |
| 日线以上 | Day / Week / Month / Quarter / Year             |

- `AdjustType`：`NoAdjust`（不复权）/ `ForwardAdjust`（前复权）
- `Session`：`Trading` / `NormalAndPrePost`（含盘前盘后）/ `NormalAndOverNight`（含夜盘）/ `All`

### 分时类型（TimeshareType）

| 类型           | 说明                         |
| -------------- | ---------------------------- |
| `Trading`      | 当日分时（盘中）             |
| `FiveDays`     | 5 日分时                     |
| `Sample`       | 小分时（多股票缩略版）       |
| `PreTrading`   | 美股盘前分时                 |
| `PostTrading`  | 美股盘后分时                 |
| `NightTrading` | 夜盘分时                     |
| `IntraDay`     | 美股全时段（盘前+盘中+盘后） |

---

## 一、财务报表（Financial Statements）✅ 已实现

### 功能说明

| 功能         | 说明                                                         |
| ------------ | ------------------------------------------------------------ |
| 财务报表明细 | 利润表 / 资产负债表 / 现金流量表的完整科目数据，多期横向对比 |

### API Path

```
GET /v1/quote/financial-reports
  ?counter_id=ST/US/TSLA   # symbol_to_counter_id("TSLA.US") 转换
  &kind=IS                  # IS=利润表 BS=资产负债表 CF=现金流量表
  &report=FY2024            # 可选，报告期
```

### 报告期枚举

`Annual`（年报）/ `Interim`（中报）/ `Quarterly`（季报：Q1/Q2/Q3/Q4）

### 财务报表类型枚举

`IS`（Income Statement 利润表）/ `BS`（Balance Sheet 资产负债表）/ `CF`（Cash Flow 现金流量表）

### CLI 用法

```bash
# 默认：利润表（最近 5 期横向对比）
longbridge financial-report TSLA.US

# 指定类型
longbridge financial-report TSLA.US --kind BS   # 资产负债表
longbridge financial-report TSLA.US --kind CF   # 现金流量表

# 指定报告期
longbridge financial-report TSLA.US --report FY2024
longbridge financial-report TSLA.US --kind IS --report Q3FY2024

# JSON 输出
longbridge financial-report TSLA.US --format json
```

---

## 二、估值分析（Valuation）✅ 已实现

### 功能说明

| 功能         | 说明                                              |
| ------------ | ------------------------------------------------- |
| 估值详情     | 当前指标值 + 5年 High/Median/Low + 行业中位数 + 同行对比 |
| 历史估值走势 | 指定指标的历史序列（日度/月度），含区间统计       |

### API Path

```
GET /v1/quote/valuation/detail
  ?counter_id=ST/US/TSLA
  &indicator=pe             # pe / pb / ps / dvd_yld

GET /v1/quote/valuation
  ?counter_id=ST/US/TSLA
  &indicator=pe
  &range=5                  # 1 / 3 / 5 / 10（年）
```

### 估值指标枚举

`pe`（市盈率）/ `pb`（市净率）/ `ps`（市销率）/ `dvd_yld`（股息率）

### CLI 用法

```bash
# 估值详情（当前值 + 历史区间 + 同行对比）
longbridge valuation detail TSLA.US
longbridge valuation detail TSLA.US --indicator pe   # 指定指标

# 历史估值走势
longbridge valuation TSLA.US --indicator pe --range 5   # 5年 PE 历史
longbridge valuation TSLA.US --indicator pb --range 1   # 1年 PB 日度数据

# JSON 输出
longbridge valuation detail TSLA.US --format json
longbridge valuation TSLA.US --indicator pe --range 5 --format json
```

---

## 三、机构评级（Institution Rating）✅ 已实现

### 功能说明

| 功能         | 说明                                                             |
| ------------ | ---------------------------------------------------------------- |
| 评级概况     | 强买/买/持有/卖 评级分布 + 共识目标价 + 目标价区间 + 行业排名   |
| 评级历史详情 | 月度评级分布趋势 + 分析师预测准确率 + 周度目标价历史            |

### API Path

```
GET /v1/quote/institution-ratings
  ?counter_id=ST/US/TSLA

GET /v1/quote/institution-rating-latest
  ?counter_id=ST/US/TSLA

GET /v1/quote/institution-ratings/detail
  ?counter_id=ST/US/TSLA
```

### 评级枚举

`strong_buy` / `buy` / `hold` / `sell` / `under` / `no_opinion`

### CLI 用法

```bash
# 当前评级分布 + 共识目标价
longbridge institution-rating TSLA.US

# 历史评级趋势 + 目标价时序
longbridge institution-rating detail TSLA.US

# JSON 输出
longbridge institution-rating TSLA.US --format json
longbridge institution-rating detail TSLA.US --format json
```

---

## 四、指数/ETF 成分股（Index & ETF Constituents）

### 功能说明

| 功能           | 说明                                                            |
| -------------- | --------------------------------------------------------------- |
| 指数成分股列表 | 成分股清单，含最新价、涨跌幅、资金净流入/流出、总股本、流通股本 |
| 涨跌统计       | 成分股上涨/下跌/平盘数量                                        |
| ETF 成分股     | ETF 的持仓股票列表                                              |
| 概念板块成分股 | 概念板块（如"新能源汽车 CP00013.US"）的龙头/成分股列表          |

### API Path

```
GET /v2/discovery/index-constituents
  ?counter_id=SPY.US     # 指数 symbol（如 SPY.US、000300.SH、HSI.HK）
  &offset=0
  &limit=100
  &indicator=0           # 排序指标字段 id
  &order=0               # 0=降序 1=升序

GET /stock-info/etf-holdings
  ?counter_id=SPY.US

GET /market/concept_stocks
  ?concept_index=CP00013.US
  &offset=0
  &limit=50
  &filter_tag_key=1      # 1=龙头股 3=成分股
```

### CLI 规划

```bash
# 指数/ETF 成分股（统一命令，自动识别类型）
longbridge constituents SPY.US               # S&P 500 成分股
longbridge constituents HSI.HK               # 恒生指数成分股
longbridge constituents 000300.SH            # 沪深300成分股
longbridge constituents CP00013.US           # 新能源汽车概念板块

# 排序
longbridge constituents SPY.US --sort chg    # 按涨跌幅排序
longbridge constituents SPY.US --sort inflow # 按资金净流入

# 只看龙头股（概念板块）
longbridge constituents CP00011.US --leaders

# JSON 输出
longbridge constituents SPY.US --format json
```

---

## 五、新股认购（IPO）

> 注：当前通过 H5 页面处理，以下为规划阶段。

### 功能说明

| 功能     | 说明                                                       |
| -------- | ---------------------------------------------------------- |
| 新股日历 | 即将上市 / 正在认购 / 已上市的新股列表                     |
| 新股详情 | 招股信息：发行价区间、募资规模、行业、股份数量、认购截止日 |
| 我的认购 | 当前用户的认购记录和状态                                   |

### API Path（参考）

```
POST /ipo/calendar
  → 新股日历

GET /stock-info/ipo-profile
  ?counter_id=XXXX.HK

GET /ipo/history
  → 当前账户认购记录
```

### CLI 规划

```bash
# 新股日历
longbridge ipo                           # 近期新股（所有市场）
longbridge ipo --market hk               # 港股新股
longbridge ipo --market us               # 美股 IPO
longbridge ipo --status subscribing      # 正在认购中

# 新股详情
longbridge ipo detail XXXX.HK

# 我的认购记录
longbridge ipo subscriptions

# JSON 输出
longbridge ipo --format json
```

---

## 六、新闻资讯（News）✅ 已实现

命令：`longbridge news`、`longbridge filing`

```bash
# 新闻列表
longbridge news TSLA.US
longbridge news TSLA.US --count 20

# 新闻详情（全文）
longbridge news detail <news_id>

# 监管文件列表
longbridge filing TSLA.US

# 文件详情（列出附件 / 下载）
longbridge filing detail TSLA.US <filing_id>
longbridge filing detail TSLA.US <filing_id> --list-files
longbridge filing detail TSLA.US <filing_id> --file-index 0
```

---

## 七、期权（Option）✅ 已实现

命令：`longbridge option`

```bash
# 期权行情
longbridge option AAPL.US

# 期权链（到期日列表）
longbridge option chain AAPL.US

# 到期日下的行权价列表
longbridge option chain AAPL.US --expiry 2025-06-20

# 期权成交量统计（Call/Put 比）—— 待实现
longbridge option volume AAPL.US

# JSON 输出
longbridge option AAPL.US --format json
```

---

## 八、权证（Warrant）✅ 已实现

命令：`longbridge warrant`

```bash
# 权证行情
longbridge warrant 700.HK

# 权证列表（含溢价率/Delta/IV）
longbridge warrant list 700.HK
longbridge warrant list 700.HK --sort delta
longbridge warrant list 700.HK --type call

# 发行人列表
longbridge warrant issuers

# JSON 输出
longbridge warrant list 700.HK --format json
```

---

## 九、公司概况与高管（Company Overview & Executives）

### 功能说明

| 功能            | 说明                                                                       |
| --------------- | -------------------------------------------------------------------------- |
| 公司概况        | 成立日期、上市日期、员工数、地址、官网、IPO 价格、董事长、审计机构等       |
| 公司高管列表    | 高管姓名、职位、简介（biography）                                          |
| 全景聚合        | 所属行业（含行业涨跌家数、个股排名）+ 相关证券分时                         |
| 行业排名        | 个股关键指标在所属行业中的排名（PE、ROE、营收等）                          |
| 关联公司/供应链 | 与该股相关联的公司列表（上下游、生态伙伴），含 counter_id + 关联产品描述   |

### API Path

```
GET /stock-info/comp-overview
  ?counter_id=ST/US/TSLA

GET /stock-info/company-professionals
  ?counter_ids=ST/US/TSLA

GET /v1/stock-info/panorama
  ?counter_id=ST/US/TSLA

GET /v1/stock-info/ranking-in-industry
  ?counter_id=ST/US/TSLA
```

### CLI 规划

```bash
# 公司基本信息
longbridge company TSLA.US

# 公司高管
longbridge executives TSLA.US
longbridge executives TSLA.US --format json

# 所属行业 + 个股在行业内的指标排名
longbridge industry TSLA.US
longbridge industry TSLA.US --format json
```

---

## 十、分红历史（Dividend History）✅ 已实现

### 功能说明

| 功能         | 说明                                                 |
| ------------ | ---------------------------------------------------- |
| 分红摘要     | 历史派息记录：除权日、金额、到账日、登记日           |
| 分红分配方案 | 完整分配方案明细（`/dividends/details` 独立端点）    |

### API Path

```
GET /v1/quote/dividends
  ?counter_id=ST/US/TSLA

GET /v1/quote/dividends/details
  ?counter_id=ST/US/TSLA
```

### CLI 用法

```bash
# 分红历史摘要
longbridge dividend TSLA.US

# 分红分配方案明细
longbridge dividend detail TSLA.US

# JSON 输出
longbridge dividend TSLA.US --format json
longbridge dividend detail TSLA.US --format json
```

---

## 十一、持有该股的基金（Fund Holders）

### 功能说明

| 功能           | 说明                                                      |
| -------------- | --------------------------------------------------------- |
| 持有该股的基金 | 哪些基金/ETF 持有这只股票，含持仓比例、持仓市值、持仓变化 |

### API Path

```
GET /ut/fundamental/reverse/stock
  ?counter_id=ST/US/TSLA

GET /stock-info/reverse/stock
  ?counter_id=ST/US/TSLA
```

### CLI 规划

```bash
longbridge fund-holders TSLA.US
longbridge fund-holders TSLA.US --count 20
longbridge fund-holders TSLA.US --format json
```

---

## 十二、主营业务拆分（Business Breakdown）

> 来源：`stock-info` scope（scopes.json 确认存在）

### 功能说明

| 功能         | 说明                                  |
| ------------ | ------------------------------------- |
| 主营业务构成 | 按业务线/地区拆分的收入占比（当前期） |
| 主营业务历史 | 各业务线收入的历史序列                |

### API Path

```
GET /stock-info/business
  ?counter_id=ST/US/TSLA

GET /stock-info/business-historical
  ?counter_id=ST/US/TSLA
```

### CLI 规划

```bash
longbridge business TSLA.US               # 当期主营业务收入拆分
longbridge business TSLA.US --history     # 历史趋势
longbridge business TSLA.US --format json
```

---

## 十三、供应链（Supply Chain）

> 来源：`stock-info` scope + `fundamental-app` scope（scopes.json 确认）

### 功能说明

| 功能            | 说明                                    |
| --------------- | --------------------------------------- |
| 核心供应链列表  | 上下游主要供应商/客户，含产品描述       |
| 供应链详情      | 上下游完整列表                          |
| 产业链          | 所属产业链及产业链内个股                |

### API Path

```
GET /stock-info/supply_chains
  ?counter_id=ST/US/TSLA

GET /stock-info/supply-chains-detail
  ?counter_id=ST/US/TSLA
  &offset=0&limit=20

GET /industrial/chain/counterid
  → 查询所属产业链
```

### CLI 规划

```bash
longbridge supply-chain TSLA.US            # 供应链摘要（上下游）
longbridge supply-chain detail TSLA.US     # 完整详情
longbridge supply-chain TSLA.US --format json
```

---

## 十四、股东结构（Shareholders）

> 来源：`stock-info` scope（scopes.json 确认存在）

### 功能说明

| 功能         | 说明                          |
| ------------ | ----------------------------- |
| 股东列表     | Top20 主要股东（机构 + 个人） |
| 机构持仓明细 | 机构持股列表 + 持仓变动       |
| Insider 持仓 | 公司内部人员持股              |

### API Path

```
GET /stock-info/company-shareholders
  ?counter_id=ST/US/TSLA

GET /stock-info/get_company_major_shareholders
GET /stock-info/get_company_institution_holding_detail
GET /stock-info/get_company_insider_holding_detail
```

### CLI 规划

```bash
longbridge shareholders TSLA.US               # 主要股东列表
longbridge shareholders TSLA.US --institutions # 机构持仓明细
longbridge shareholders TSLA.US --insiders    # Insider 持仓
longbridge shareholders TSLA.US --format json
```

---

## 十五、一致预期（Consensus Estimates）✅ 已实现

### 功能说明

| 功能             | 说明                                                |
| ---------------- | --------------------------------------------------- |
| EPS 预测快照序列 | 分析师 EPS 共识随时间收敛的快照（最近 20 条）       |
| 财务一致预期详情 | 营收/EBIT/净利润/EPS 多期横向对比，含 beat/miss 标记 |

### API Path

```
GET /v1/quote/forecast-eps
  ?counter_id=ST/US/TSLA

GET /v1/quote/financial-consensus-detail
  ?counter_id=ST/US/TSLA
```

### CLI 用法

```bash
# EPS 预测共识快照（最近 20 条）
longbridge forecast-eps TSLA.US
longbridge forecast-eps TSLA.US --format json

# 多期财务一致预期横向对比（营收/利润/EPS）
longbridge consensus TSLA.US
longbridge consensus TSLA.US --format json
```

---

## 十六、投资风格与多维评分（Security Ratings）✅ 已实现

### 功能说明

| 功能             | 说明                                                                  |
| ---------------- | --------------------------------------------------------------------- |
| 投资风格分类     | 1-9 格子（价值/平衡/成长 × 小盘/中盘/大盘）+ 风格描述                |
| 多维打分         | 盈利 / 成长 / 现金 / 运营 / 负债 五维评分（分数 + 字母等级 + 趋势）  |
| 行业横向对比     | 行业排名 + 行业均值/中位数评分                                        |

### API Path

```
GET /v1/quote/security-ratings
  ?counter_id=ST/US/TSLA
```

### CLI 用法

```bash
# 投资风格 + 多维评分概览
longbridge score TSLA.US

# JSON 输出（含完整 ratings 树）
longbridge score TSLA.US --format json
```

---

## 十八、财经日历（Finance Calendar）

> 来源：`stock-info` scope（scopes.json 确认）

### 功能说明

| 功能     | 说明                                             |
| -------- | ------------------------------------------------ |
| 财经日历 | 重要经济事件（财报发布日、央行决议、宏观数据等） |
| 新股日历 | IPO 上市时间线                                   |

### API Path

```
POST /stock_info/finance_calendar
POST /stock_info/finance_calendar_detail
GET  /stock_info/finance_calendar_date
```

### CLI 规划

```bash
longbridge calendar                          # 近期财经日历
longbridge calendar --date 2025-04-01        # 指定日期
longbridge calendar --type earnings          # 只看财报发布
longbridge calendar TSLA.US                  # 指定股票相关事件
longbridge calendar --format json
```

---

## 十九、待确认 / 不确定开放的功能

以下功能在 app 内存在，是否对外开放 API 需产品确认：

| 功能         | App Crate / Scope                       | 说明                                                 | 建议                           |
| ------------ | --------------------------------------- | ---------------------------------------------------- | ------------------------------ |
| 股票提醒     | `price-notify` scope                    | 指标提醒 API 完整，scope 已确认                      | 可直接对外开放                 |
| 技术指标数据 | `indicator-gateway` scope               | 获取/保存用户自定义指标配置                          | 更适合工具 API，非数据查询     |
| 榜单排行     | `rank-list` scope                       | 多指标排行榜（涨幅、换手率等）                       | 高价值，建议规划               |
