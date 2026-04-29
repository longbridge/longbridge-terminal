(function () {
  var raw = __JSON__;
  var ov = raw.overview || {};
  var holdings = raw.holdings || [];

  // ── Summary stats ─────────────────────────────────────────────────────────
  var riskLabel = ['Safe', 'Middle', 'Warning', 'Danger'][ov.risk_level] || 'Unknown';
  var totalPl = parseFloat(ov.total_pl) || 0;
  var todayPl = parseFloat(ov.total_today_pl) || 0;

  function fmtMoney(v) {
    var n = parseFloat(v) || 0;
    var abs = Math.abs(n);
    if (abs >= 1e9) return (n / 1e9).toFixed(2) + 'B';
    if (abs >= 1e6) return (n / 1e6).toFixed(2) + 'M';
    if (abs >= 1e3) return (n / 1e3).toFixed(2) + 'K';
    return n.toFixed(2);
  }

  function statCard(label, value, colorClass) {
    return '<div class="flex flex-col gap-0.5">'
      + '<span class="text-[10px] text-[#677179] uppercase tracking-wider">' + label + '</span>'
      + '<span class="font-mono text-sm ' + (colorClass || '') + '">' + value + '</span>'
      + '</div>';
  }

  var plClass = totalPl >= 0 ? 'text-[#5da602]' : 'text-[#d84a33]';
  var todayClass = todayPl >= 0 ? 'text-[#5da602]' : 'text-[#d84a33]';

  var statsEl = document.getElementById('pf-stats');
  if (statsEl) {
    statsEl.innerHTML = [
      statCard('Total Asset', fmtMoney(ov.total_asset) + ' ' + (ov.currency || '')),
      statCard('Market Cap', fmtMoney(ov.market_cap) + ' ' + (ov.currency || '')),
      statCard('Cash', fmtMoney(ov.total_cash) + ' ' + (ov.currency || '')),
      statCard('P/L', (totalPl >= 0 ? '+' : '') + fmtMoney(totalPl), plClass),
      statCard('Today P/L', (todayPl >= 0 ? '+' : '') + fmtMoney(todayPl), todayClass),
      statCard('Risk', riskLabel),
    ].join('');
  }

  // ── Asset distribution pie ────────────────────────────────────────────────
  var marketMap = {};
  holdings.forEach(function (h) {
    var mkt = 'HK';
    var dot = h.symbol.lastIndexOf('.');
    if (dot !== -1) {
      var suffix = h.symbol.slice(dot + 1);
      if (suffix === 'US') mkt = 'US';
      else if (suffix === 'SH' || suffix === 'SZ') mkt = 'CN';
      else if (suffix === 'SG') mkt = 'SG';
    }
    marketMap[mkt] = (marketMap[mkt] || 0) + (parseFloat(h.market_value_usd) || 0);
  });
  var cash = parseFloat(ov.total_cash) || 0;
  if (cash > 0) marketMap['Cash'] = cash;
  var fund = parseFloat(ov.fund_market_value) || 0;
  if (fund > 0) marketMap['Fund'] = fund;

  var pieData = Object.keys(marketMap).map(function (k) {
    return { name: k, value: +marketMap[k].toFixed(2) };
  }).sort(function (a, b) { return b.value - a.value; });

  var pie = echarts.init(document.getElementById('chart-pie'), 'lb', { renderer: 'canvas' });
  pie.setOption({
    animationDuration: 600, animationEasing: 'cubicOut',
    tooltip: {
      trigger: 'item', backgroundColor: '#0e0e0e', borderColor: '#282828',
      textStyle: { color: '#feffff', fontSize: 11 },
      formatter: function (p) { return p.name + ': ' + p.value.toFixed(2) + ' (' + p.percent + '%)'; }
    },
    legend: { orient: 'vertical', right: 8, top: 'center', textStyle: { color: '#677179', fontSize: 10 } },
    series: [{
      type: 'pie', radius: ['40%', '68%'], center: ['40%', '50%'],
      label: { color: '#677179', fontSize: 10 },
      data: pieData,
    }]
  });
  window.addEventListener('resize', function () { pie.resize(); });

  // ── Holdings bar chart ────────────────────────────────────────────────────
  var sorted = holdings.slice().sort(function (a, b) {
    return (parseFloat(b.market_value_usd) || 0) - (parseFloat(a.market_value_usd) || 0);
  });
  var totalMv = sorted.reduce(function (s, h) { return s + (parseFloat(h.market_value_usd) || 0); }, 0);
  var barSymbols = sorted.map(function (h) { return h.symbol.split('.')[0]; });
  var barVals = sorted.map(function (h) {
    var mv = parseFloat(h.market_value_usd) || 0;
    return +(totalMv > 0 ? mv / totalMv * 100 : 0).toFixed(2);
  });
  var barColors = sorted.map(function (h) {
    var pl = h.cost_price
      ? (parseFloat(h.market_price) - parseFloat(h.cost_price)) * parseFloat(h.quantity)
      : 0;
    return pl >= 0 ? '#00B89A' : '#D94400';
  });

  var bar = echarts.init(document.getElementById('chart-bar'), 'lb', { renderer: 'canvas' });
  bar.setOption({
    animationDuration: 600, animationEasing: 'cubicOut',
    tooltip: {
      trigger: 'axis', axisPointer: { type: 'shadow' },
      backgroundColor: '#0e0e0e', borderColor: '#282828',
      textStyle: { color: '#feffff', fontSize: 11 },
      formatter: function (params) {
        var p = params[0];
        return p.axisValue + ': ' + p.value.toFixed(2) + '%';
      }
    },
    grid: { left: 48, right: 16, top: 16, bottom: 48 },
    xAxis: {
      type: 'category', data: barSymbols,
      axisLabel: { color: '#677179', fontSize: 10, rotate: 30 },
      axisLine: { lineStyle: { color: '#282828' } },
    },
    yAxis: {
      scale: true, splitLine: { lineStyle: { color: '#282828' } },
      axisLabel: { color: '#677179', fontSize: 10, formatter: function (v) { return v + '%'; } }
    },
    series: [{
      type: 'bar', barMaxWidth: 28,
      label: { show: true, position: 'top', color: '#677179', fontSize: 10, formatter: function (p) { return p.value.toFixed(1) + '%'; } },
      data: barVals.map(function (v, i) { return { value: v, itemStyle: { color: barColors[i] } }; }),
    }]
  });
  window.addEventListener('resize', function () { bar.resize(); });

  // ── Holdings table ────────────────────────────────────────────────────────
  var headers = ['Symbol', 'Name', 'Qty', 'Price', 'Cost', 'Mkt Value (USD)', 'P/L', 'P/L %', 'Prev Close'];
  var rows = holdings.map(function (h) {
    var price = parseFloat(h.market_price) || 0;
    var cost = parseFloat(h.cost_price) || 0;
    var qty = parseFloat(h.quantity) || 0;
    var pl = cost > 0 ? (price - cost) * qty : null;
    var plPct = cost > 0 ? ((price - cost) / cost * 100) : null;
    return [
      h.symbol,
      h.name || '',
      qty.toFixed(0),
      price.toFixed(3),
      cost > 0 ? cost.toFixed(3) : '-',
      (parseFloat(h.market_value_usd) || 0).toFixed(2),
      pl != null ? pl.toFixed(2) : '-',
      plPct != null ? plPct.toFixed(2) + '%' : '-',
      (parseFloat(h.prev_close) || 0).toFixed(3),
    ];
  });
  buildTable(headers, rows);
})();
