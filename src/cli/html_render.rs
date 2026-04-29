use anyhow::Result;
use serde_json::Value;

const ECHARTS_CDN: &str = "https://assets.lbkrs.com/libs/echarts@5.min.js";

// Colorful dark theme for ECharts. 10-color cycling palette tuned for dark backgrounds.
// Candlestick convention: green = price up, red = price down.
const THEME_JS: &str = r#"echarts.registerTheme('lb',{
  color:['#4D7FFF','#AAEE00','#7A7A8E','#FF9000','#00CCFF','#FFCC00','#FF3399','#AA33DD','#00BBAA','#7766BB'],
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
    format!(
        r#"var cur={cur};
var hist={hist};
(function(){{
  var descEl=document.getElementById('desc');
  if(descEl&&cur.description){{
    var t=+cur.temperature;
    descEl.textContent=cur.description;
    descEl.style.color=t<30?'#41b3a9':t<70?'#aa7900':'#d84a33';
  }}
  function mkGauge(el,val){{
    var c=echarts.init(document.getElementById(el),'lb',{{renderer:'canvas'}});
    c.setOption({{series:[{{
      type:'gauge',min:0,max:100,
      radius:'80%',center:['50%','58%'],
      startAngle:210,endAngle:-30,
      axisLine:{{lineStyle:{{width:16,color:[[0.3,'#41b3a9'],[0.7,'#aa7900'],[1,'#d84a33']]}}}},
      axisTick:{{show:false}},splitLine:{{show:false}},axisLabel:{{show:false}},
      pointer:{{width:5,length:'65%',itemStyle:{{color:'#feffff',opacity:0.9}}}},
      detail:{{formatter:'{{value}}',fontSize:28,fontWeight:'bold',color:'#feffff',offsetCenter:[0,'28%']}},
      title:{{show:false}},
      data:[{{value:+val}}]
    }}]}});
    window.addEventListener('resize',function(){{c.resize()}});
  }}
  mkGauge('g1',cur.temperature);
  mkGauge('g2',cur.valuation);
  mkGauge('g3',cur.sentiment);
  if(!hist||hist.length===0){{
    var s=document.getElementById('hist-section');
    if(s)s.style.display='none';
    return;
  }}
  var hc=echarts.init(document.getElementById('hist'),'lb',{{renderer:'canvas'}});
  var dates=hist.map(function(d){{return d.time}});
  var temps=hist.map(function(d){{return+d.temperature}});
  var vals=hist.map(function(d){{return+d.valuation}});
  var sents=hist.map(function(d){{return+d.sentiment}});
  hc.setOption({{
    animationDuration:1000,animationEasing:'cubicOut',
    tooltip:{{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
    legend:{{data:['Temperature','Valuation','Sentiment'],top:4,right:8}},
    grid:{{left:50,right:16,top:32,bottom:48}},
    xAxis:{{type:'category',data:dates,axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
    yAxis:{{scale:true,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
    dataZoom:[{{type:'inside'}},{{bottom:4,height:24,borderColor:'#282828'}}],
    series:[
      {{name:'Temperature',type:'line',data:temps,showSymbol:false,smooth:true,
        lineStyle:{{color:'#d84a33',width:2}},
        areaStyle:{{color:{{type:'linear',x:0,y:0,x2:0,y2:1,
          colorStops:[{{offset:0,color:'rgba(216,74,51,0.2)'}},{{offset:1,color:'rgba(216,74,51,0)'}}]
        }}}}}},
      {{name:'Valuation',type:'line',data:vals,showSymbol:false,smooth:true,
        lineStyle:{{color:'#41b3a9',width:2}}}},
      {{name:'Sentiment',type:'line',data:sents,showSymbol:false,smooth:true,
        lineStyle:{{color:'#ffb670',width:2}}}}
    ]
  }});
  window.addEventListener('resize',function(){{hc.resize()}});
  buildTable(
    ['Time','Temperature','Valuation','Sentiment'],
    hist.map(function(d){{return[d.time,d.temperature,d.valuation,d.sentiment]}}));
}})();"#
    )
}

// ── Kline ─────────────────────────────────────────────────────────────────────

fn kline_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    // Volume bar colors follow candlestick convention: red=up, green=down.
    format!(
        r#"var raw={json};
var dates=raw.map(function(d){{return d.time}});
var ohlc=raw.map(function(d){{return[+d.open,+d.close,+d.low,+d.high]}});
var vols=raw.map(function(d){{return{{value:+d.volume,itemStyle:{{color:+d.close>=+d.open?'#5da602':'#d84a33'}}}}}});
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',axisPointer:{{type:'cross'}},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:['K Line','Volume'],top:4,right:8}},
  grid:[
    {{left:60,right:16,top:32,bottom:52}},
    {{left:60,right:16,top:'74%',bottom:32}}
  ],
  xAxis:[
    {{type:'category',data:dates,scale:true,gridIndex:0,boundaryGap:true,
      axisLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}},
      splitLine:{{show:false}}}},
    {{type:'category',data:dates,scale:true,gridIndex:1,axisLabel:{{show:false}},
      axisLine:{{lineStyle:{{color:'#282828'}}}}}}
  ],
  yAxis:[
    {{scale:true,gridIndex:0,splitLine:{{lineStyle:{{color:'#282828'}}}},
      axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{show:false}}}},
    {{scale:true,gridIndex:1,splitLine:{{lineStyle:{{color:'#282828'}}}},
      axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{show:false}}}}
  ],
  dataZoom:[
    {{type:'inside',xAxisIndex:[0,1],start:0,end:100}},
    {{xAxisIndex:[0,1],bottom:4,height:24,borderColor:'#282828'}}
  ],
  series:[
    {{name:'K Line',type:'candlestick',xAxisIndex:0,yAxisIndex:0,data:ohlc,
      emphasis:{{itemStyle:{{shadowBlur:12,shadowColor:'rgba(0,0,0,0.8)'}}}} }},
    {{name:'Volume',type:'bar',xAxisIndex:1,yAxisIndex:1,data:vols,barMaxWidth:12}}
  ]
}});
buildTable(
  ['Time','Open','High','Low','Close','Volume'],
  raw.map(function(d){{return[d.time,d.open,d.high,d.low,d.close,Number(d.volume).toLocaleString()]}})
);"#
    )
}

// ── Intraday ──────────────────────────────────────────────────────────────────

fn intraday_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var dates=raw.map(function(d){{return d.time}});
var prices=raw.map(function(d){{return+d.price}});
var avgPrices=raw.map(function(d){{return+d.avg_price}});
var vols=raw.map(function(d){{return+d.volume}});
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',axisPointer:{{type:'cross'}},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:['Price','Avg Price','Volume'],top:4,right:8}},
  grid:[
    {{left:60,right:16,top:32,bottom:52}},
    {{left:60,right:16,top:'74%',bottom:32}}
  ],
  xAxis:[
    {{type:'category',data:dates,gridIndex:0,boundaryGap:false,
      axisLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
    {{type:'category',data:dates,gridIndex:1,axisLabel:{{show:false}},
      axisLine:{{lineStyle:{{color:'#282828'}}}}}}
  ],
  yAxis:[
    {{scale:true,gridIndex:0,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
    {{scale:true,gridIndex:1,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}}
  ],
  dataZoom:[{{type:'inside',xAxisIndex:[0,1]}}],
  series:[
    {{name:'Price',type:'line',xAxisIndex:0,yAxisIndex:0,data:prices,showSymbol:false,smooth:true,
      lineStyle:{{color:'#41b3a9',width:2}},
      areaStyle:{{color:{{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{{offset:0,color:'rgba(65,179,169,0.25)'}},{{offset:1,color:'rgba(65,179,169,0)'}}]
      }}}},
      emphasis:{{focus:'series'}} }},
    {{name:'Avg Price',type:'line',xAxisIndex:0,yAxisIndex:0,data:avgPrices,showSymbol:false,smooth:true,
      lineStyle:{{color:'#ffb670',width:1.5,type:'dashed'}}}},
    {{name:'Volume',type:'bar',xAxisIndex:1,yAxisIndex:1,data:vols,
      itemStyle:{{color:'rgba(65,179,169,0.4)'}},barMaxWidth:6}}
  ]
}});
buildTable(
  ['Time','Price','Avg Price','Volume'],
  raw.map(function(d){{return[d.time,d.price,d.avg_price,Number(d.volume).toLocaleString()]}})
);"#
    )
}

// ── Capital Flow ──────────────────────────────────────────────────────────────

fn capital_flow_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var dates=raw.map(function(d){{return d.time}});
var vals=raw.map(function(d){{return+d.inflow}});
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  grid:{{left:70,right:16,top:16,bottom:48}},
  xAxis:{{type:'category',data:dates,axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:{{scale:true,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
  dataZoom:[{{type:'inside'}},{{bottom:4,height:24,borderColor:'#282828'}}],
  series:[{{
    name:'Capital Inflow',type:'bar',
    data:vals.map(function(v){{return{{value:v,itemStyle:{{color:v>=0?'#41b3a9':'#d84a33'}}}}}}),
    barMaxWidth:16,
    emphasis:{{itemStyle:{{shadowBlur:8,shadowColor:'rgba(0,0,0,0.5)'}}}}
  }}]
}});
buildTable(
  ['Time','Inflow'],
  raw.map(function(d){{return[d.time,d.inflow]}})
);"#
    )
}

// ── Capital Distribution ──────────────────────────────────────────────────────

fn capital_dist_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var cats=['Large','Medium','Small'];
var inflow=[+raw.capital_in.large,+raw.capital_in.medium,+raw.capital_in.small];
var outflow=[+raw.capital_out.large,+raw.capital_out.medium,+raw.capital_out.small];
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',axisPointer:{{type:'shadow'}},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:['Inflow','Outflow'],top:4,right:8}},
  grid:{{left:70,right:16,top:36,bottom:16}},
  xAxis:{{type:'category',data:cats,axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:{{scale:true,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
  series:[
    {{name:'Inflow',type:'bar',data:inflow,itemStyle:{{color:'#41b3a9'}},barMaxWidth:40,
      emphasis:{{itemStyle:{{shadowBlur:8,shadowColor:'rgba(65,179,169,0.4)'}}}}}},
    {{name:'Outflow',type:'bar',data:outflow,itemStyle:{{color:'#d84a33'}},barMaxWidth:40,
      emphasis:{{itemStyle:{{shadowBlur:8,shadowColor:'rgba(216,74,51,0.4)'}}}}}}
  ]
}});
buildTable(
  ['Direction','Large','Medium','Small'],
  [
    ['Inflow',raw.capital_in.large,raw.capital_in.medium,raw.capital_in.small],
    ['Outflow',raw.capital_out.large,raw.capital_out.medium,raw.capital_out.small]
  ]
);"#
    )
}

// ── Depth ─────────────────────────────────────────────────────────────────────

fn depth_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var asks=(raw.asks||[]).slice().reverse();
var bids=raw.bids||[];
var askPrices=asks.map(function(d){{return d.price}});
var bidPrices=bids.map(function(d){{return d.price}});
var allPrices=askPrices.concat(bidPrices);
var askMap={{}};asks.forEach(function(d){{askMap[d.price]=+d.volume}});
var bidMap={{}};bids.forEach(function(d){{bidMap[d.price]=+d.volume}});
chart.setOption({{
  animationDuration:600,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',axisPointer:{{type:'shadow'}},backgroundColor:'#0e0e0e',borderColor:'#282828',
    textStyle:{{color:'#feffff',fontSize:11}},
    formatter:function(p){{return p.map(function(s){{return s.seriesName+': '+Math.abs(s.value).toLocaleString()}}).join('<br>')}}}},
  legend:{{data:['Ask','Bid'],top:4,right:8}},
  grid:{{left:70,right:16,top:36,bottom:16}},
  xAxis:{{type:'value',axisLabel:{{color:'#677179',fontSize:10,formatter:function(v){{return Math.abs(v).toLocaleString()}}}},
    splitLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:{{type:'category',data:allPrices,axisLabel:{{color:'#677179',fontSize:10}}}},
  series:[
    {{name:'Ask',type:'bar',data:allPrices.map(function(p){{return askMap[p]||null}}),
      itemStyle:{{color:'#d84a33'}},barMaxWidth:20,
      emphasis:{{itemStyle:{{shadowBlur:8,shadowColor:'rgba(216,74,51,0.5)'}}}}}},
    {{name:'Bid',type:'bar',data:allPrices.map(function(p){{return bidMap[p]?-bidMap[p]:null}}),
      itemStyle:{{color:'#41b3a9'}},barMaxWidth:20,
      emphasis:{{itemStyle:{{shadowBlur:8,shadowColor:'rgba(65,179,169,0.5)'}}}}}}
  ]
}});
var rows=[];
var maxLen=Math.max(asks.length,bids.length);
for(var i=0;i<maxLen;i++){{
  var a=asks[i]||{{}};var b=bids[i]||{{}};
  rows.push([a.price||'',a.volume?Number(a.volume).toLocaleString():'',
             b.price||'',b.volume?Number(b.volume).toLocaleString():'']);
}}
buildTable(['Ask Price','Ask Vol','Bid Price','Bid Vol'],rows);"#
    )
}

// ── Market Temperature History ─────────────────────────────────────────────────

fn market_temp_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var dates=raw.map(function(d){{return d.time}});
var temps=raw.map(function(d){{return+d.temperature}});
var vals=raw.map(function(d){{return+d.valuation}});
var sents=raw.map(function(d){{return+d.sentiment}});
chart.setOption({{
  animationDuration:1000,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:['Temperature','Valuation','Sentiment'],top:4,right:8}},
  grid:{{left:50,right:16,top:32,bottom:48}},
  xAxis:{{type:'category',data:dates,axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:{{scale:true,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
  dataZoom:[{{type:'inside'}},{{bottom:4,height:24,borderColor:'#282828'}}],
  series:[
    {{name:'Temperature',type:'line',data:temps,showSymbol:false,smooth:true,
      lineStyle:{{color:'#d84a33',width:2}},
      areaStyle:{{color:{{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{{offset:0,color:'rgba(216,74,51,0.2)'}},{{offset:1,color:'rgba(216,74,51,0)'}}]
      }}}}}},
    {{name:'Valuation',type:'line',data:vals,showSymbol:false,smooth:true,
      lineStyle:{{color:'#41b3a9',width:2}}}},
    {{name:'Sentiment',type:'line',data:sents,showSymbol:false,smooth:true,
      lineStyle:{{color:'#ffb670',width:2}}}}
  ]
}});
buildTable(
  ['Time','Temperature','Valuation','Sentiment'],
  raw.map(function(d){{return[d.time,d.temperature,d.valuation,d.sentiment]}})
);"#
    )
}

// ── Valuation History ─────────────────────────────────────────────────────────

fn valuation_history_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"(function(){{
var raw={json};
var metrics=raw.metrics||{{}};
var keys=Object.keys(metrics);
if(!keys.length){{chart.setOption({{title:{{text:'No data',left:'center',top:'center',textStyle:{{color:'#677179'}}}}}});return;}}
var dateSet={{}};
var seriesMap={{}};
keys.forEach(function(k){{
  seriesMap[k]={{}};
  (metrics[k].list||[]).forEach(function(pt){{
    var d=new Date(+pt.timestamp*1000).toISOString().slice(0,10);
    dateSet[d]=1;
    seriesMap[k][d]=pt.value;
  }});
}});
var dates=Object.keys(dateSet).sort();
var colors=['#4D7FFF','#AAEE00','#FF9000','#00CCFF','#FF3399','#FFCC00'];
var series=keys.map(function(k,i){{
  return{{name:k.toUpperCase(),type:'line',
    data:dates.map(function(d){{return seriesMap[k][d]!=null?+seriesMap[k][d]:null}}),
    showSymbol:false,smooth:false,connectNulls:false,
    lineStyle:{{color:colors[i%colors.length],width:2}}}};
}});
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:keys.map(function(k){{return k.toUpperCase()}}),top:4,right:8,textStyle:{{color:'#677179'}}}},
  grid:{{left:60,right:16,top:32,bottom:48}},
  xAxis:{{type:'category',data:dates,axisLabel:{{color:'#677179',fontSize:10}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:{{scale:true,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
  dataZoom:[{{type:'inside'}},{{bottom:4,height:24,borderColor:'#282828'}}],
  series:series
}});
buildTable(
  ['Date'].concat(keys.map(function(k){{return k.toUpperCase()}})),
  dates.map(function(d){{
    return[d].concat(keys.map(function(k){{
      var v=seriesMap[k][d];return v!=null?v:'';
    }}));
  }}));
}})();"#
    )
}

// ── Short Positions ───────────────────────────────────────────────────────────

fn short_positions_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var dates=raw.map(function(d){{return d.date}});
var rates=raw.map(function(d){{return+d.rate}});
var shares=raw.map(function(d){{return+d.short_shares}});
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',axisPointer:{{type:'cross'}},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:['Short Rate%','Short Shares'],top:4,right:8}},
  grid:{{left:60,right:70,top:32,bottom:48}},
  xAxis:{{type:'category',data:dates,axisLabel:{{color:'#677179',fontSize:10,rotate:30}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:[
    {{name:'Rate%',nameTextStyle:{{color:'#677179',fontSize:10}},scale:true,
      splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
    {{name:'Shares',nameTextStyle:{{color:'#677179',fontSize:10}},scale:true,position:'right',
      axisLabel:{{color:'#677179',fontSize:10}},splitLine:{{show:false}}}}
  ],
  dataZoom:[{{type:'inside'}},{{bottom:4,height:24,borderColor:'#282828'}}],
  series:[
    {{name:'Short Rate%',type:'line',yAxisIndex:0,data:rates,showSymbol:false,smooth:true,
      lineStyle:{{color:'#d84a33',width:2}},
      areaStyle:{{color:{{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{{offset:0,color:'rgba(216,74,51,0.2)'}},{{offset:1,color:'rgba(216,74,51,0)'}}]
      }}}}}},
    {{name:'Short Shares',type:'bar',yAxisIndex:1,data:shares,
      itemStyle:{{color:'rgba(65,179,169,0.45)'}},barMaxWidth:12}}
  ]
}});
buildTable(
  ['Date','Short Rate%','Short Shares'],
  raw.map(function(d){{return[d.date,d.rate.toFixed(2)+'%',Number(d.short_shares).toLocaleString()]}})
);"#
    )
}

// ── Option Volume Daily ───────────────────────────────────────────────────────

fn option_volume_daily_js(data: &Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    format!(
        r#"var raw={json};
var dates=raw.map(function(d){{return d.date}});
var callVols=raw.map(function(d){{return+d.call_vol}});
var putVols=raw.map(function(d){{return+d.put_vol}});
var pcRatio=raw.map(function(d){{return+d.pc_ratio}});
chart.setOption({{
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{{trigger:'axis',axisPointer:{{type:'shadow'}},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{{color:'#feffff',fontSize:11}}}},
  legend:{{data:['Call Vol','Put Vol','P/C Ratio'],top:4,right:8}},
  grid:{{left:70,right:70,top:32,bottom:48}},
  xAxis:{{type:'category',data:dates,axisLabel:{{color:'#677179',fontSize:10,rotate:30}},axisLine:{{lineStyle:{{color:'#282828'}}}}}},
  yAxis:[
    {{scale:true,splitLine:{{lineStyle:{{color:'#282828'}}}},axisLabel:{{color:'#677179',fontSize:10}}}},
    {{name:'P/C',nameTextStyle:{{color:'#677179',fontSize:10}},scale:true,position:'right',
      axisLabel:{{color:'#677179',fontSize:10}},splitLine:{{show:false}}}}
  ],
  dataZoom:[{{type:'inside'}},{{bottom:4,height:24,borderColor:'#282828'}}],
  series:[
    {{name:'Call Vol',type:'bar',stack:'vol',data:callVols,itemStyle:{{color:'#41b3a9'}},barMaxWidth:16}},
    {{name:'Put Vol',type:'bar',stack:'vol',data:putVols,itemStyle:{{color:'#d84a33'}},barMaxWidth:16}},
    {{name:'P/C Ratio',type:'line',yAxisIndex:1,data:pcRatio,showSymbol:false,smooth:true,
      lineStyle:{{color:'#ffb670',width:2}}}}
  ]
}});
buildTable(
  ['Date','Call Vol','Put Vol','P/C Ratio'],
  raw.map(function(d){{return[d.date,Number(d.call_vol).toLocaleString(),Number(d.put_vol).toLocaleString(),d.pc_ratio]}})
);"#
    )
}
