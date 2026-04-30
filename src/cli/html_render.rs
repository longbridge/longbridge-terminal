use anyhow::Result;
use serde_json::Value;

const ECHARTS_CDN: &str = "https://assets.lbkrs.com/libs/echarts@5.min.js";

// Colorful dark theme for ECharts. 10-color cycling palette tuned for dark backgrounds.
// Candlestick convention: green = price up, red = price down.
const THEME_JS: &str = r#"echarts.registerTheme('lb',{
  color:['#00B89A','#D4BC00','#D94400','#4488CC','#44AA66','#CC4488','#8855BB','#0088AA','#DD7733','#7788AA'],
  backgroundColor:'rgba(0,0,0,0)',
  candlestick:{itemStyle:{color:'#5da602',color0:'#d84a33',borderColor:'#5da602',borderColor0:'#d84a33',borderWidth:1}},
  line:{lineStyle:{width:2},symbolSize:4,symbol:'emptyCircle'},
  bar:{itemStyle:{barBorderWidth:0}},
  legend:{textStyle:{color:'#5d6165'}},
  dataZoom:{
    backgroundColor:'rgba(0,0,0,0)',
    dataBackgroundColor:'rgba(255,255,255,0.06)',
    fillerColor:'rgba(65,179,169,0.1)',
    handleColor:'#41b3a9',
    handleStyle:{color:'#41b3a9'},
    textStyle:{color:'#677179'}
  }
});"#;

static TEMPLATE: &str = include_str!("html_render.html");

pub enum HtmlPayload {
    Kline {
        title: String,
        command: String,
        data: Value,
    },
    Intraday {
        title: String,
        command: String,
        data: Value,
    },
    HistoryIntraday {
        title: String,
        command: String,
        data: Value,
    },
    CapitalFlow {
        title: String,
        command: String,
        data: Value,
    },
    CapitalDist {
        title: String,
        command: String,
        data: Value,
    },
    Depth {
        title: String,
        command: String,
        data: Value,
    },
    MarketTempHistory {
        title: String,
        command: String,
        data: Value,
    },
    ValuationHistory {
        title: String,
        command: String,
        data: Value,
    },
    AhPremium {
        title: String,
        command: String,
        data: Value,
    },
    MarketTemp {
        title: String,
        command: String,
        current: Value,
        history: Value,
    },
    ShortPositions {
        title: String,
        command: String,
        data: Value,
    },
    OptionVolumeDaily {
        title: String,
        command: String,
        data: Value,
    },
    TradeStats {
        title: String,
        command: String,
        data: Value,
    },
    InstitutionRating {
        title: String,
        command: String,
        data: Value,
    },
    Table {
        title: String,
        command: String,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Renders arbitrary JSON data as an auto-detected table or key/value grid.
    RawJson {
        title: String,
        command: String,
        data: Value,
    },
    /// Renders a list of news items as HTML cards.
    News {
        title: String,
        command: String,
        items: Vec<Value>,
    },
    /// Renders financial statements (IS / BS / CF) as multi-section tables.
    FinancialReport {
        title: String,
        command: String,
        data: Value,
    },
    /// Renders industry valuation comparison as a radar chart.
    IndustryValuation {
        title: String,
        command: String,
        data: Value,
    },
    /// Renders industry valuation distribution as a grouped bar chart.
    IndustryValuationDist {
        title: String,
        command: String,
        data: Value,
    },
    /// Renders portfolio overview with pie chart + bar chart + holdings table.
    Portfolio {
        title: String,
        command: String,
        data: Value,
    },
    /// Renders profit analysis summary + category bar chart + per-stock table.
    ProfitAnalysis {
        title: String,
        command: String,
        data: Value,
    },
}

/// Convenience wrapper for rendering a plain table as HTML.
pub fn open_html_table(
    title: &str,
    command: &str,
    headers: &[&str],
    rows: Vec<Vec<String>>,
) -> Result<()> {
    open_html(HtmlPayload::Table {
        title: title.to_string(),
        command: command.to_string(),
        headers: headers.iter().map(|h| h.to_string()).collect(),
        rows,
    })
}

/// Convenience wrapper for rendering arbitrary JSON data as HTML.
pub fn open_html_raw(title: &str, command: &str, data: Value) -> Result<()> {
    open_html(HtmlPayload::RawJson {
        title: title.to_string(),
        command: command.to_string(),
        data,
    })
}

pub fn open_html(payload: HtmlPayload) -> Result<()> {
    let now = time::OffsetDateTime::now_utc();
    let fmt =
        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second] UTC");
    let generated_at = now.format(&fmt).unwrap_or_else(|_| now.to_string());
    let html = build_html(&payload, &generated_at);
    let mut file = tempfile::Builder::new()
        .prefix("longbridge-")
        .suffix(".html")
        .tempfile()?;
    std::io::Write::write_all(&mut file, html.as_bytes())?;
    let (_, path) = file.keep()?;
    open::that(&path)?;
    eprintln!("Opened: {}", path.display());
    Ok(())
}

fn build_html(payload: &HtmlPayload, generated_at: &str) -> String {
    match payload {
        HtmlPayload::Kline {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &kline_js(data)),
        HtmlPayload::Intraday {
            title,
            command,
            data,
        }
        | HtmlPayload::HistoryIntraday {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &intraday_js(data)),
        HtmlPayload::CapitalFlow {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &capital_flow_js(data)),
        HtmlPayload::CapitalDist {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &capital_dist_js(data)),
        HtmlPayload::Depth {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &depth_js(data)),
        HtmlPayload::MarketTempHistory {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &market_temp_js(data)),
        HtmlPayload::ValuationHistory {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &valuation_history_js(data)),
        HtmlPayload::AhPremium {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &ah_premium_js(data)),
        HtmlPayload::MarketTemp {
            title,
            command,
            current,
            history,
        } => render_gauge_page(title, command, generated_at, current, history),
        HtmlPayload::ShortPositions {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &short_positions_js(data)),
        HtmlPayload::OptionVolumeDaily {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &option_volume_daily_js(data)),
        HtmlPayload::TradeStats {
            title,
            command,
            data,
        } => render_trade_stats_page(title, command, generated_at, data),
        HtmlPayload::InstitutionRating {
            title,
            command,
            data,
        } => render_institution_rating_page(title, command, generated_at, data),
        HtmlPayload::Table {
            title,
            command,
            headers,
            rows,
        } => render_table_page(title, command, generated_at, headers, rows),
        HtmlPayload::RawJson {
            title,
            command,
            data,
        } => render_raw_json_page(title, command, generated_at, data),
        HtmlPayload::News {
            title,
            command,
            items,
        } => render_news_page(title, command, generated_at, items),
        HtmlPayload::FinancialReport {
            title,
            command,
            data,
        } => render_financial_report_page(title, command, generated_at, data),
        HtmlPayload::IndustryValuation {
            title,
            command,
            data,
        } => render_chart_page(title, command, generated_at, &industry_valuation_js(data)),
        HtmlPayload::IndustryValuationDist {
            title,
            command,
            data,
        } => render_chart_page(
            title,
            command,
            generated_at,
            &industry_valuation_dist_js(data),
        ),
        HtmlPayload::Portfolio {
            title,
            command,
            data,
        } => render_portfolio_page(title, command, generated_at, data),
        HtmlPayload::ProfitAnalysis {
            title,
            command,
            data,
        } => render_profit_analysis_page(title, command, generated_at, data),
    }
}

fn fill_template(
    title: &str,
    command: &str,
    generated_at: &str,
    cdn_scripts: &str,
    main_html: &str,
    data_js: &str,
) -> String {
    TEMPLATE
        .replace("__TITLE__", title)
        .replace("__COMMAND__", command)
        .replace("__GENERATED_AT__", generated_at)
        .replace("__CDN_SCRIPTS__", cdn_scripts)
        .replace("__MAIN__", main_html)
        .replace("__DATA_JS__", data_js)
}

// ── Chart page ────────────────────────────────────────────────────────────────

fn render_chart_page(title: &str, command: &str, generated_at: &str, body_js: &str) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let main = r#"<div class="w-full h-[480px] mb-7 border border-[#282828] bg-[#040404]" id="chart"></div>
<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Data</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div>
</div>"#;

    // Theme + chart init + body_js, concatenated to avoid format! brace issues.
    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push_str(
        "\nvar chart=echarts.init(document.getElementById('chart'),'lb',{renderer:'canvas'});\n",
    );
    data_js.push_str(body_js);
    data_js.push_str("\nwindow.addEventListener('resize',function(){chart.resize()});");

    fill_template(title, command, generated_at, &cdn, main, &data_js)
}

// ── Table page ────────────────────────────────────────────────────────────────

fn render_table_page(
    title: &str,
    command: &str,
    generated_at: &str,
    headers: &[String],
    rows: &[Vec<String>],
) -> String {
    let thead: String = headers
        .iter()
        .map(|h| format!("<th>{h}</th>"))
        .collect::<Vec<_>>()
        .join("");
    let tbody: String = rows
        .iter()
        .map(|row| {
            let cells: String = row
                .iter()
                .map(|cell| format!("<td>{cell}</td>"))
                .collect::<Vec<_>>()
                .join("");
            format!("<tr>{cells}</tr>")
        })
        .collect::<Vec<_>>()
        .join("");
    let main = format!(
        r#"<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Data</div>
<div class="tbl-wrap"><table><thead><tr>{thead}</tr></thead><tbody>{tbody}</tbody></table></div>
</div>"#
    );
    fill_template(title, command, generated_at, "", &main, "")
}

// ── Raw JSON auto-table ───────────────────────────────────────────────────────

fn render_raw_json_page(title: &str, command: &str, generated_at: &str, data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    let main = r#"<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Data</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div>
</div>"#;
    // Auto-render: array of objects → table columns from keys; object → key/value pairs.
    let data_js = format!(
        r#"(function(){{
  var raw={json};
  if(Array.isArray(raw)&&raw.length>0&&typeof raw[0]==='object'){{
    var cols=[...new Set(raw.flatMap(Object.keys))];
    var rows=raw.map(function(r){{return cols.map(function(c){{return r[c]!=null?String(r[c]):''}});}});
    buildTable(cols,rows);
  }}else if(raw!==null&&typeof raw==='object'&&!Array.isArray(raw)){{
    var rows=Object.entries(raw).map(function([k,v]){{return[k,typeof v==='object'?JSON.stringify(v):String(v)]}});
    buildTable(['field','value'],rows);
  }}
}})();"#
    );
    fill_template(title, command, generated_at, "", main, &data_js)
}

// ── Market Temperature Gauge ──────────────────────────────────────────────────

fn render_gauge_page(
    title: &str,
    command: &str,
    generated_at: &str,
    current: &Value,
    history: &Value,
) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let main = r#"<div class="mb-6 flex items-center gap-3"><span id="desc" class="text-[18px] font-bold tracking-wide"></span></div>
<div class="grid grid-cols-3 gap-4 mb-7">
<div class="border border-[#282828] bg-[#040404]"><div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Temperature</div><div class="h-[200px]" id="g1"></div></div>
<div class="border border-[#282828] bg-[#040404]"><div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Valuation</div><div class="h-[200px]" id="g2"></div></div>
<div class="border border-[#282828] bg-[#040404]"><div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Sentiment</div><div class="h-[200px]" id="g3"></div></div>
</div>
<div id="hist-section" class="border border-[#282828] bg-[#040404] mb-7"><div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">History (90 days)</div><div class="w-full h-[320px]" id="hist"></div></div>
<div class="border border-[#282828] mb-7"><div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Data</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div></div>"#;

    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push('\n');
    data_js.push_str(&market_temp_current_js(current, history));
    fill_template(title, command, generated_at, &cdn, main, &data_js)
}

fn market_temp_current_js(current: &Value, history: &Value) -> String {
    let cur = serde_json::to_string(current).unwrap_or_default();
    let hist = serde_json::to_string(history).unwrap_or_default();
    include_str!("html_render/market_temp_current.js")
        .replace("__CURRENT_JSON__", &cur)
        .replace("__HISTORY_JSON__", &hist)
}

// ── Kline ─────────────────────────────────────────────────────────────────────

fn kline_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    // Volume bar colors follow candlestick convention: red=up, green=down.
    include_str!("html_render/kline.js").replace("__JSON__", &json)
}

// ── Intraday ──────────────────────────────────────────────────────────────────

fn intraday_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/intraday.js").replace("__JSON__", &json)
}

// ── Capital Flow ──────────────────────────────────────────────────────────────

fn capital_flow_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/capital_flow.js").replace("__JSON__", &json)
}

// ── Capital Distribution ──────────────────────────────────────────────────────

fn capital_dist_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/capital_dist.js").replace("__JSON__", &json)
}

// ── Depth ─────────────────────────────────────────────────────────────────────

fn depth_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/depth.js").replace("__JSON__", &json)
}

// ── Market Temperature History ─────────────────────────────────────────────────

fn market_temp_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/market_temp.js").replace("__JSON__", &json)
}

// ── Valuation History ─────────────────────────────────────────────────────────

fn valuation_history_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/valuation_history.js").replace("__JSON__", &json)
}

// ── A/H Premium ───────────────────────────────────────────────────────────────

fn ah_premium_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/ah_premium.js").replace("__JSON__", &json)
}

// ── Short Positions ───────────────────────────────────────────────────────────

fn short_positions_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/short_positions.js").replace("__JSON__", &json)
}

// ── Trade Stats ───────────────────────────────────────────────────────────────

fn render_trade_stats_page(title: &str, command: &str, generated_at: &str, data: &Value) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let main = r#"<div class="bg-[#171717] border border-[#282828] px-[18px] py-[14px] mb-7">
<div id="ts-stats" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-x-6 gap-y-4"></div>
</div>
<div class="w-full h-[480px] mb-7 border border-[#282828] bg-[#040404]" id="chart"></div>
<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Price Distribution</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div>
</div>"#;

    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push_str(
        "\nvar chart=echarts.init(document.getElementById('chart'),'lb',{renderer:'canvas'});\n",
    );
    let json = serde_json::to_string(data).unwrap_or_default();
    data_js.push_str(&include_str!("html_render/trade_stats.js").replace("__JSON__", &json));
    data_js.push_str("\nwindow.addEventListener('resize',function(){chart.resize()});");
    fill_template(title, command, generated_at, &cdn, main, &data_js)
}

// ── Institution Rating ────────────────────────────────────────────────────────

fn render_institution_rating_page(
    title: &str,
    command: &str,
    generated_at: &str,
    data: &Value,
) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let main = r#"<div class="bg-[#171717] border border-[#282828] px-[18px] py-[14px] mb-7">
<div id="ir-consensus" class="grid grid-cols-2 sm:grid-cols-4 gap-x-6 gap-y-4"></div>
</div>
<div class="w-full h-[280px] mb-7 border border-[#282828] bg-[#040404]" id="chart"></div>
<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Rating Breakdown</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div>
</div>"#;

    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push_str(
        "\nvar chart=echarts.init(document.getElementById('chart'),'lb',{renderer:'canvas'});\n",
    );
    let json = serde_json::to_string(data).unwrap_or_default();
    data_js.push_str(&include_str!("html_render/institution_rating.js").replace("__JSON__", &json));
    data_js.push_str("\nwindow.addEventListener('resize',function(){chart.resize()});");
    fill_template(title, command, generated_at, &cdn, main, &data_js)
}

// ── Option Volume Daily ───────────────────────────────────────────────────────

fn option_volume_daily_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/option_volume_daily.js").replace("__JSON__", &json)
}

// ── News card list ────────────────────────────────────────────────────────────

fn render_news_page(title: &str, command: &str, generated_at: &str, items: &[Value]) -> String {
    let date_fmt = time::macros::format_description!("[year]-[month]-[day]");
    let cards: String = items
        .iter()
        .map(|item| {
            let item_title = item["title"].as_str().unwrap_or("").trim().to_string();
            let url = item["url"].as_str().unwrap_or("#");
            let ts = item["published_at"].as_i64().unwrap_or(0);
            let date = time::OffsetDateTime::from_unix_timestamp(ts)
                .ok()
                .and_then(|dt| dt.format(&date_fmt).ok())
                .unwrap_or_default();
            let likes = item["likes_count"].as_i64().unwrap_or(0);
            let comments = item["comments_count"].as_i64().unwrap_or(0);
            // Escape any HTML special chars in title
            let safe_title = item_title
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;");
            format!(
                r#"<div class="border border-[#282828] mb-3 px-5 py-4 hover:bg-[#0e0e0e]"><a href="{url}" target="_blank" class="text-[#feffff] no-underline hover:text-[#41b3a9] text-sm font-semibold leading-snug">{safe_title}</a><div class="text-[11px] text-[#5d6165] mt-1.5">{date} · {likes} likes · {comments} comments</div></div>"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let main = format!(r#"<div class="mb-2">{cards}</div>"#);
    fill_template(title, command, generated_at, "", &main, "")
}

// ── Financial Report ──────────────────────────────────────────────────────────

fn render_financial_report_page(
    title: &str,
    command: &str,
    generated_at: &str,
    data: &Value,
) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let sections = [
        ("IS", "Income Statement"),
        ("BS", "Balance Sheet"),
        ("CF", "Cash Flow"),
    ];
    let mut main = String::new();
    for (idx, (key, label)) in sections.iter().enumerate() {
        main.push_str(&format!(
            r#"<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">{label}</div>
<div class="w-full h-[300px] bg-[#040404]" id="chart-{key}"></div>
<div class="tbl-wrap"><table><thead id="thead-{idx}"></thead><tbody id="tbody-{idx}"></tbody></table></div>
</div>"#
        ));
    }

    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push('\n');
    data_js.push_str(&financial_report_js(data));
    fill_template(title, command, generated_at, &cdn, &main, &data_js)
}

fn financial_report_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/financial_report.js").replace("__JSON__", &json)
}

// ── Industry Valuation ────────────────────────────────────────────────────────

fn industry_valuation_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/industry_valuation.js").replace("__JSON__", &json)
}

// ── Industry Valuation Distribution ──────────────────────────────────────────

fn industry_valuation_dist_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    include_str!("html_render/industry_valuation_dist.js").replace("__JSON__", &json)
}

// ── Portfolio ─────────────────────────────────────────────────────────────────

fn render_portfolio_page(title: &str, command: &str, generated_at: &str, data: &Value) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let main = r#"<div class="bg-[#171717] border border-[#282828] px-[18px] py-[14px] mb-7">
<div id="pf-stats" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-x-6 gap-y-4"></div>
</div>
<div class="grid grid-cols-2 gap-4 mb-7">
<div class="border border-[#282828] bg-[#040404]">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Asset Distribution</div>
<div class="h-[300px]" id="chart-pie"></div>
</div>
<div class="border border-[#282828] bg-[#040404]">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Holdings by Market Value</div>
<div class="h-[300px]" id="chart-bar"></div>
</div>
</div>
<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">Holdings</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div>
</div>"#;

    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push('\n');
    let json = serde_json::to_string(data).unwrap_or_default();
    data_js.push_str(&include_str!("html_render/portfolio.js").replace("__JSON__", &json));
    fill_template(title, command, generated_at, &cdn, main, &data_js)
}

// ── Profit Analysis ───────────────────────────────────────────────────────────

fn render_profit_analysis_page(
    title: &str,
    command: &str,
    generated_at: &str,
    data: &Value,
) -> String {
    let cdn = format!(r#"<script src="{ECHARTS_CDN}"></script>"#);
    let main = r#"<div class="bg-[#171717] border border-[#282828] px-[18px] py-[14px] mb-7">
<div id="pa-stats" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-x-6 gap-y-4"></div>
</div>
<div class="w-full h-[300px] mb-7 border border-[#282828] bg-[#040404]" id="chart"></div>
<div class="border border-[#282828] mb-7">
<div class="bg-[#171717] px-3.5 py-2 text-[10px] text-[#677179] uppercase tracking-[.08em] border-b border-[#282828]">P&amp;L Breakdown</div>
<div class="tbl-wrap"><table><thead id="thead"></thead><tbody id="tbody"></tbody></table></div>
</div>"#;

    let mut data_js = String::new();
    data_js.push_str(THEME_JS);
    data_js.push_str(
        "\nvar chart=echarts.init(document.getElementById('chart'),'lb',{renderer:'canvas'});\n",
    );
    let json = serde_json::to_string(data).unwrap_or_default();
    data_js.push_str(&include_str!("html_render/profit_analysis.js").replace("__JSON__", &json));
    data_js.push_str("\nwindow.addEventListener('resize',function(){chart.resize()});");
    fill_template(title, command, generated_at, &cdn, main, &data_js)
}
