(function () {
  var raw = __JSON__;
  var profits = raw.profits || {};
  var sublist = raw.sublist || {};
  var items = sublist.items || [];

  function fmtPct(v) {
    var n = parseFloat(v);
    if (isNaN(n)) return '-';
    return (n >= 0 ? '+' : '') + (n * 100).toFixed(2) + '%';
  }
  function fmtMoney(v) {
    var n = parseFloat(v);
    if (isNaN(n) || n === 0) return '-';
    var abs = Math.abs(n);
    var s = abs >= 1e9 ? (n / 1e9).toFixed(2) + 'B'
           : abs >= 1e6 ? (n / 1e6).toFixed(2) + 'M'
           : abs >= 1e3 ? (n / 1e3).toFixed(2) + 'K'
           : n.toFixed(2);
    return (n >= 0 ? '+' : '') + s;
  }
  function plClass(v) {
    var n = parseFloat(v);
    return isNaN(n) ? '' : n >= 0 ? 'text-[#5da602]' : 'text-[#d84a33]';
  }
  function statCard(label, value, cls) {
    return '<div class="flex flex-col gap-0.5">'
      + '<span class="text-[10px] text-[#677179] uppercase tracking-wider">' + label + '</span>'
      + '<span class="font-mono text-sm ' + (cls || '') + '">' + value + '</span>'
      + '</div>';
  }

  // ── Summary stats ─────────────────────────────────────────────────────────
  var statsEl = document.getElementById('pa-stats');
  if (statsEl) {
    var totalPl = raw.sum_profit || '0';
    var simpleYield = raw.total_simple_earning_yield || '0';
    var twr = raw.total_time_earning_yield || '0';
    var period = (raw.start_date || '') + ' ~ ' + (raw.end_date || '');
    statsEl.innerHTML = [
      statCard('Period', period),
      statCard('Total Asset', raw.current_total_asset || '-'),
      statCard('Invest Amount', raw.invest_amount || '-'),
      statCard('Total P&L', fmtMoney(totalPl), plClass(totalPl)),
      statCard('Simple Yield', fmtPct(simpleYield), plClass(simpleYield)),
      statCard('TWR', fmtPct(twr), plClass(twr)),
    ].join('');
  }

  // ── Top gainers + top losers bar chart (absolute P&L, 8+8) ───────────────
  var stockItems = items
    .filter(function (item) { return item.security_code; })
    .map(function (item) {
      var code = item.security_code || '';
      var market = item.market || '';
      return {
        label: market ? code + '.' + market : code,
        pl: parseFloat(item.profit) || 0,
        rate: (!isNaN(parseFloat(item.profit_rate)) && item.profit_rate !== '')
          ? (parseFloat(item.profit_rate) * 100).toFixed(2) + '%' : '-',
      };
    });

  var TOP_N = 8;
  var byPl = stockItems.slice().sort(function (a, b) { return b.pl - a.pl; });
  var gainers = byPl.filter(function (x) { return x.pl > 0; }).slice(0, TOP_N);
  var losers  = byPl.filter(function (x) { return x.pl < 0; }).slice(-TOP_N).reverse();
  var chartItems = losers.concat(gainers);

  if (chartItems.length) {
    chart.setOption({
      animationDuration: 600, animationEasing: 'cubicOut',
      tooltip: {
        trigger: 'axis', axisPointer: { type: 'shadow' },
        backgroundColor: '#0e0e0e', borderColor: '#282828',
        textStyle: { color: '#feffff', fontSize: 11 },
        formatter: function (params) {
          var p = params[0];
          var item = chartItems[p.dataIndex];
          return p.axisValue + ': ' + p.value.toFixed(2) + '  (' + item.rate + ')';
        }
      },
      grid: { left: 64, right: 16, top: 16, bottom: 60 },
      xAxis: {
        type: 'category',
        data: chartItems.map(function (x) { return x.label; }),
        axisLabel: { color: '#677179', fontSize: 9, rotate: 35 },
        axisLine: { lineStyle: { color: '#282828' } },
      },
      yAxis: {
        scale: true, splitLine: { lineStyle: { color: '#282828' } },
        axisLabel: {
          color: '#677179', fontSize: 10,
          formatter: function (v) {
            var a = Math.abs(v);
            if (a >= 1e6) return (v / 1e6).toFixed(1) + 'M';
            if (a >= 1e3) return (v / 1e3).toFixed(1) + 'K';
            return v;
          }
        }
      },
      series: [{
        type: 'bar', barMaxWidth: 28,
        data: chartItems.map(function (x) {
          return { value: x.pl, itemStyle: { color: x.pl >= 0 ? '#00B89A' : '#D94400' } };
        }),
      }]
    });
  }

  // ── Per-stock table (sorted by P&L desc) ─────────────────────────────────
  var sortedItems = items.slice().sort(function (a, b) {
    return (parseFloat(b.profit) || 0) - (parseFloat(a.profit) || 0);
  });
  if (sortedItems.length) {
    var headers = ['Symbol', 'Name', 'P&L', 'P&L %', 'Invest Cost', 'Holding Value'];
    var rows = sortedItems.map(function (item) {
      var code = item.security_code || '';
      var market = item.market || '';
      var symbol = code ? (market ? code + '.' + market : code) : (item.isin || item.name || '-');
      var _pr = parseFloat(item.profit_rate);
      var plPct = (!isNaN(_pr) && item.profit_rate !== '') ? (_pr * 100).toFixed(2) + '%' : '-';
      return [
        symbol,
        item.name || '-',
        item.profit || '-',
        plPct,
        item.invest_cost || '-',
        item.holding_value || '-',
      ];
    });
    buildTable(headers, rows);
  }
})();
